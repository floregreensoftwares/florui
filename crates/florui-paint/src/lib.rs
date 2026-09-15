//! Paints a styled, laid-out `florui_style::Arena` into pixels — the
//! "painting" stage at the end of the pipeline (components → elements →
//! style → boxes/layout → text → **painting**).
//!
//! # Scope
//!
//! Flat background-color rectangles, solid borders, and hard-edged
//! box-shadows, painted in real document order: a node before its
//! children, children in source order, so an overlap always resolves to
//! whichever box comes later in the tree — the same rule real CSS
//! painting follows for normal-flow boxes with no stacking contexts. A
//! leaf's own text (see [`florui_style::Arena::text_content`]) paints
//! inside its padding box: glyph outlines come from
//! [`florui_text::Font::shape`] and are rasterized with
//! [skrifa](https://github.com/googlefonts/fontations)'s outline extractor
//! feeding a [tiny-skia](https://github.com/RazrFalcon/tiny-skia) path,
//! the same rasterizer backgrounds/borders/shadows use — one painting
//! backend, not several.
//!
//! `box-shadow`'s `blur-radius` parses and cascades all the way through
//! [`florui_style::BoxShadow`], but is not painted: tiny-skia 0.11 (this
//! crate's whole rasterizer) has no Gaussian blur or mask-filter
//! primitive at all, so there's nothing to rasterize a blur *with* short
//! of hand-rolling a box-blur pass over a mask, out of scope for this
//! crate's first slice of the property. Every shadow paints with a hard
//! edge regardless of its declared blur — a "supported syntax, simplified
//! rendering" gap, the same shape as this crate's own solid-only borders
//! (see [`paint_border`]'s own doc). There is no border-radius, opacity,
//! transform, or clipping yet — `florui_style` has no properties for any
//! of those either.

use std::collections::HashMap;
use std::path::Path;

use florui_layout::{BoxLayout, absolute_position};
use florui_style::{Arena, ComputedStyle, NodeId, Rgba};
use florui_text::Font;
use skrifa::instance::{LocationRef, NormalizedCoord, Size as GlyphSize};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

pub type Canvas = Pixmap;

#[derive(Debug)]
pub struct PaintError {
    path: std::path::PathBuf,
    source: png::EncodingError,
}

impl std::fmt::Display for PaintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not write {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for PaintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Paints `width` x `height` physical pixels — `canvas` everywhere, then
/// every node's background and text, in document order — and writes the
/// result as a PNG.
///
/// # Panics
///
/// Panics if `width` or `height` is zero: there is no meaningful canvas to
/// paint into.
#[allow(clippy::too_many_arguments)]
pub fn paint_to_png(
    font: &mut Font,
    path: &Path,
    width: u32,
    height: u32,
    canvas: Rgba,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    scale_factor: f32,
) -> Result<(), PaintError> {
    paint_to_buffer(
        font,
        width,
        height,
        canvas,
        arena,
        styles,
        layouts,
        scale_factor,
    )
    .save_png(path)
    .map_err(|source| PaintError {
        path: path.to_owned(),
        source,
    })
}

/// Same painting as [`paint_to_png`], returning the pixel buffer directly
/// instead of writing it to disk. `font` is the caller's own long-lived
/// instance — see [`florui_layout::compute_layout`]'s own doc for why, and
/// pass it the exact same instance that computed `layouts`, since this
/// paints the identical glyphs that font already shaped.
///
/// `layouts` and `width`/`height` are in the *painted* canvas's own units —
/// a caller painting a HiDPI window passes boxes already scaled up to
/// physical pixels (its own `scale_layouts`, mirrored in
/// `florui_conformance::engine`), not the logical pixels layout itself ran
/// against. `scale_factor` is how much that scaling multiplied every box
/// by (`1.0` for an unscaled/logical canvas). Text painting splits the
/// difference: it shapes at the *logical* font size and wrap width — the
/// same ones layout itself measured with, recovered by dividing the
/// already-scaled box width back down — so wrapping matches the committed
/// layout exactly, then scales the resulting glyph outlines and positions
/// back up by `scale_factor` only at rasterization time, so the ink
/// itself is painted at the canvas's own (possibly physical) size. Get
/// either half wrong and it shows up differently: skip the wrap-width
/// divide and text re-wraps wider than the box was sized for; skip the
/// rasterization scale-up and text paints correctly wrapped but roughly
/// `scale_factor` times too small.
///
/// # Panics
///
/// Panics if `width` or `height` is zero.
#[allow(clippy::too_many_arguments)]
pub fn paint_to_buffer(
    font: &mut Font,
    width: u32,
    height: u32,
    canvas: Rgba,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    scale_factor: f32,
) -> Canvas {
    let mut buffer =
        Pixmap::new(width, height).expect("paint_to_buffer requires a nonzero-sized canvas");
    buffer.fill(to_tiny_skia_color(canvas));

    let mut stack: Vec<NodeId> = arena.roots().iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        paint_node(
            &mut buffer,
            arena,
            styles,
            layouts,
            font,
            node,
            scale_factor,
        );
        stack.extend(arena.children(node).iter().rev());
    }
    buffer
}

/// Paints `node`'s own background and text — document-order painting
/// (an overlapping later node always wins) comes from the caller's own
/// pre-order walk over the whole tree, not from this function recursing.
#[allow(clippy::too_many_arguments)]
fn paint_node(
    buffer: &mut Canvas,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    font: &mut Font,
    node: NodeId,
    scale_factor: f32,
) {
    if let Some(&layout) = layouts.get(&node) {
        let style = styles.get(&node);
        let (x, y) = absolute_position(arena, layouts, node);

        let no_border_side = florui_style::BorderSide {
            width: 0.0,
            color: Rgba::TRANSPARENT,
        };
        let no_border = florui_style::Edges {
            top: no_border_side,
            right: no_border_side,
            bottom: no_border_side,
            left: no_border_side,
        };
        let border = style.map_or(no_border, |s| s.border);
        let box_shadow: &[florui_style::BoxShadow] = style.map_or(&[][..], |s| &s.box_shadow[..]);

        // Real CSS's own painting order for a normal-flow box with no
        // stacking context: outer (non-inset) shadows sit behind
        // everything else — background, then border, then content — while
        // inset shadows sit on top of the background but still behind the
        // border and content. See [`paint_box_shadows`]'s own doc for the
        // painted shape.
        paint_box_shadows(
            buffer,
            x,
            y,
            layout.width,
            layout.height,
            border,
            box_shadow,
            false,
        );

        let background = style.map_or(Rgba::TRANSPARENT, |s| s.background_color);
        if background.a != 0 {
            fill_rect(buffer, x, y, layout.width, layout.height, background);
        }

        paint_box_shadows(
            buffer,
            x,
            y,
            layout.width,
            layout.height,
            border,
            box_shadow,
            true,
        );

        paint_border(buffer, x, y, layout.width, layout.height, border);

        let color = style.map_or(Rgba::opaque(0, 0, 0), |s| s.color);
        let no_padding = florui_style::Edges {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        };
        let padding = style.map_or(no_padding, |s| s.padding);
        // Content starts inside both the border and the padding — the same
        // content-box math `florui_layout`'s own `to_taffy_style` doc
        // describes for sizing applies to painting's own offset too.
        let content_x = x + border.left.width + padding.left;
        let content_y = y + border.top.width + padding.top;
        // The box's own final content width, whatever layout resolved it
        // to (wrapped or not) — shaping at exactly this width always
        // reproduces what layout already measured: an intrinsically
        // unwrapped box is already exactly as wide as its one line, so
        // wrapping "at" that width changes nothing, and a box layout
        // wrapped to fit stays wrapped identically here.
        let content_width =
            (layout.width - border.left.width - border.right.width - padding.left - padding.right)
                .max(0.0);
        // `content_width` is in the painted canvas's own (possibly scaled)
        // units, but shaping below runs at the *logical* `font_size`
        // `styles` itself carries — text must reshape at the same logical
        // width layout itself wrapped against, or a HiDPI canvas
        // (`scale_factor` > 1) wraps text wider than the committed layout,
        // overflowing past where the box was sized to fit it. See
        // `paint_to_buffer`'s own doc. `paint_shaped_runs`/`paint_text`
        // scale the resulting logical-sized glyphs back up to
        // `scale_factor` at rasterization time, the other half of the same
        // split: shape at the size layout actually measured, paint at the
        // size the canvas actually needs.
        let wrap_width = content_width / scale_factor;

        if florui_layout::is_inline_formatting_context(arena, styles, node) {
            // A real mixed text/inline-element node: rebuilt and
            // reshaped fresh here, since this crate doesn't share layout's
            // own internal Taffy tree — deterministic at the same final
            // width, the same reasoning `paint_text`'s own doc gives for
            // the plain single-style case below. Every run paints in this
            // node's own inherited `color` uniformly — a per-span `color`
            // override isn't painted differently yet, a documented bound
            // matching `florui_layout`'s own module doc.
            if let Some(shaped) = florui_layout::shape_inline_formatting_context(
                font, arena, styles, node, wrap_width,
            ) {
                paint_shaped_runs(
                    buffer,
                    &shaped.runs,
                    content_x,
                    content_y,
                    color,
                    scale_factor,
                );
            }
        } else {
            let text = arena.text_content(node);
            if !text.is_empty() {
                let font_size = style.map_or(16.0, |s| s.font_size);
                let font_weight = style.map_or(400.0, |s| s.font_weight);
                let font_family = style.map_or(florui_text::FontFamily::SansSerif, |s| {
                    to_text_font_family(s.font_family)
                });
                paint_text(
                    buffer,
                    font,
                    TextPaint {
                        text,
                        font_size,
                        font_weight,
                        font_family,
                        color,
                        x: content_x,
                        y: content_y,
                        wrap_width,
                        scale_factor,
                    },
                );
            }
        }
    }
}

fn fill_rect(buffer: &mut Canvas, x: f32, y: f32, width: f32, height: f32, color: Rgba) {
    let x0 = x.max(0.0);
    let y0 = y.max(0.0);
    let x1 = (x + width).max(0.0).min(buffer.width() as f32);
    let y1 = (y + height).max(0.0).min(buffer.height() as f32);
    let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = false;
    buffer.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Paints `border`'s four sides as flat rectangles at the box's own outer
/// edges — `(x, y, width, height)` is the whole border-box, matching
/// `fill_rect`'s own background call in [`paint_node`]. A zero-width side
/// (real CSS's own invisible default; see [`florui_style::BorderSide`]'s
/// doc) paints nothing. No border-radius or per-corner miter join yet —
/// `florui_style::BorderSide` has neither — so adjacent sides simply
/// overlap by their own width at each corner, which a flat single color
/// per side (this crate's only supported case, real CSS's own `solid`)
/// paints identically to a mitered corner anyway.
fn paint_border(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    border: florui_style::Edges<florui_style::BorderSide>,
) {
    if border.top.width > 0.0 {
        fill_rect(buffer, x, y, width, border.top.width, border.top.color);
    }
    if border.bottom.width > 0.0 {
        fill_rect(
            buffer,
            x,
            y + height - border.bottom.width,
            width,
            border.bottom.width,
            border.bottom.color,
        );
    }
    if border.left.width > 0.0 {
        fill_rect(buffer, x, y, border.left.width, height, border.left.color);
    }
    if border.right.width > 0.0 {
        fill_rect(
            buffer,
            x + width - border.right.width,
            y,
            border.right.width,
            height,
            border.right.color,
        );
    }
}

/// Paints every layer of `shadows` whose own `inset` matches `inset` — the
/// caller makes two passes, one per value, so it can interleave outer
/// shadows behind the background and inset shadows in front of it (see
/// [`paint_node`]'s own doc for why). Layers paint back-to-front in list
/// order: the *last*-listed layer paints first, so the first-listed one
/// ends up on top — real CSS's own layering rule for `box-shadow`'s
/// comma-separated list.
#[allow(clippy::too_many_arguments)]
fn paint_box_shadows(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    border: florui_style::Edges<florui_style::BorderSide>,
    shadows: &[florui_style::BoxShadow],
    inset: bool,
) {
    for shadow in shadows.iter().filter(|s| s.inset == inset).rev() {
        if shadow.color.a == 0 {
            continue;
        }
        if inset {
            paint_inset_shadow(buffer, x, y, width, height, border, shadow);
        } else {
            paint_outset_shadow(buffer, x, y, width, height, shadow);
        }
    }
}

/// An outer (drop) shadow: a copy of the border box, grown by
/// `spread_radius` on every side and offset by `(offset_x, offset_y)`,
/// filled with `shadow.color` — except wherever it's covered by the
/// box's own border box, which the box's own subsequent background/border
/// painting would cover anyway, but real CSS clips it there regardless
/// (visible through a *transparent* background otherwise, which a real
/// browser never shows).
fn paint_outset_shadow(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    shadow: &florui_style::BoxShadow,
) {
    let outer_x = x + shadow.offset_x - shadow.spread_radius;
    let outer_y = y + shadow.offset_y - shadow.spread_radius;
    let outer_width = width + 2.0 * shadow.spread_radius;
    let outer_height = height + 2.0 * shadow.spread_radius;
    fill_rect_minus_hole(
        buffer,
        outer_x,
        outer_y,
        outer_width,
        outer_height,
        x,
        y,
        width,
        height,
        shadow.color,
    );
}

/// An inset shadow: real CSS clips it to the box's own padding box (its
/// interior, inside the border), which is filled with `shadow.color`
/// except for a "hole" — the padding box shrunk by `spread_radius` on
/// every side, then offset by `(offset_x, offset_y)`. Growing
/// `spread_radius` *shrinks* the hole here — the opposite sign from
/// [`paint_outset_shadow`]'s own shape, matching real CSS's own "spread
/// makes an outer shadow bigger, an inner one's hole smaller" rule.
fn paint_inset_shadow(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    border: florui_style::Edges<florui_style::BorderSide>,
    shadow: &florui_style::BoxShadow,
) {
    let padding_x = x + border.left.width;
    let padding_y = y + border.top.width;
    let padding_width = (width - border.left.width - border.right.width).max(0.0);
    let padding_height = (height - border.top.width - border.bottom.width).max(0.0);

    let hole_x = padding_x + shadow.offset_x + shadow.spread_radius;
    let hole_y = padding_y + shadow.offset_y + shadow.spread_radius;
    let hole_width = padding_width - 2.0 * shadow.spread_radius;
    let hole_height = padding_height - 2.0 * shadow.spread_radius;

    fill_rect_minus_hole(
        buffer,
        padding_x,
        padding_y,
        padding_width,
        padding_height,
        hole_x,
        hole_y,
        hole_width,
        hole_height,
        shadow.color,
    );
}

/// Fills the `outer` rectangle with `color`, minus whatever area it
/// shares with `hole` — the shape both an outer shadow (`hole` = the
/// box's own border box) and an inset shadow (`hole` = the offset/spread
/// "shadow-free" interior) need. Decomposed into up to four
/// non-overlapping rectangle strips — the top and bottom spanning
/// `outer`'s full width (covering its corners), left and right filling
/// only the vertical band between them — rather than an even-odd path
/// fill: even-odd computes a symmetric difference (an XOR) of the two
/// shapes, not a true subtraction, so it paints the wrong thing whenever
/// `hole` only partially overlaps `outer` or fully contains it (a
/// shadow entirely hidden behind its own box would wrongly show a ring).
/// A degenerate (non-positive width/height) strip is left to
/// [`fill_rect`]'s own no-op handling.
#[allow(clippy::too_many_arguments)]
fn fill_rect_minus_hole(
    buffer: &mut Canvas,
    outer_x: f32,
    outer_y: f32,
    outer_width: f32,
    outer_height: f32,
    hole_x: f32,
    hole_y: f32,
    hole_width: f32,
    hole_height: f32,
    color: Rgba,
) {
    if outer_width <= 0.0 || outer_height <= 0.0 {
        return;
    }
    let outer_x1 = outer_x + outer_width;
    let outer_y1 = outer_y + outer_height;

    // `hole` clipped to `outer`'s own bounds: the only part of it that can
    // actually remove anything.
    let clip_x0 = hole_x.max(outer_x);
    let clip_y0 = hole_y.max(outer_y);
    let clip_x1 = (hole_x + hole_width).min(outer_x1);
    let clip_y1 = (hole_y + hole_height).min(outer_y1);

    if clip_x0 >= clip_x1 || clip_y0 >= clip_y1 {
        // No overlap (including a hole with a non-positive width/height,
        // e.g. an inset shadow whose spread erased it entirely): nothing
        // to subtract.
        fill_rect(buffer, outer_x, outer_y, outer_width, outer_height, color);
        return;
    }

    fill_rect(
        buffer,
        outer_x,
        outer_y,
        outer_width,
        clip_y0 - outer_y,
        color,
    );
    fill_rect(
        buffer,
        outer_x,
        clip_y1,
        outer_width,
        outer_y1 - clip_y1,
        color,
    );
    fill_rect(
        buffer,
        outer_x,
        clip_y0,
        clip_x0 - outer_x,
        clip_y1 - clip_y0,
        color,
    );
    fill_rect(
        buffer,
        clip_x1,
        clip_y0,
        outer_x1 - clip_x1,
        clip_y1 - clip_y0,
        color,
    );
}

/// Shapes `text` (wrapped at `wrap_width`, matching whatever content width
/// layout already resolved this box to) and fills each glyph's outline at
/// `(x, y)` — the top-left of the whole shaped block, in the same
/// coordinate space as [`fill_rect`] — with `color`. Skips a run whose font
/// data doesn't parse, or an individual glyph with no outline (e.g.
/// genuinely missing from the font); a partial render beats aborting the
/// whole paint over one bad glyph.
struct TextPaint<'a> {
    text: &'a str,
    font_size: f32,
    font_weight: f32,
    font_family: florui_text::FontFamily,
    color: Rgba,
    x: f32,
    y: f32,
    wrap_width: f32,
    scale_factor: f32,
}

fn to_text_font_family(value: florui_style::FontFamily) -> florui_text::FontFamily {
    match value {
        florui_style::FontFamily::SansSerif => florui_text::FontFamily::SansSerif,
        florui_style::FontFamily::Monospace => florui_text::FontFamily::Monospace,
    }
}

fn paint_text(buffer: &mut Canvas, font: &mut Font, params: TextPaint<'_>) {
    let TextPaint {
        text,
        font_size,
        font_weight,
        font_family,
        color,
        x,
        y,
        wrap_width,
        scale_factor,
    } = params;
    let shaped = font.shape_wrapped(font_family, text, font_size, font_weight, wrap_width);
    paint_shaped_runs(buffer, &shaped.runs, x, y, color, scale_factor);
}

/// Fills every glyph across `runs` at `(x, y)` — the top-left of the whole
/// shaped block, in the same coordinate space as [`fill_rect`] — with
/// `color`. Shared by [`paint_text`] (single-style text) and a real inline
/// formatting context's own mixed-style runs, since both end up
/// with the same `&[florui_text::ShapedRun]` shape to paint, just built
/// via a different `florui_text::Font` call. Skips a run whose font data
/// doesn't parse, or an individual glyph with no outline (e.g. genuinely
/// missing from the font); a partial render beats aborting the whole paint
/// over one bad glyph.
///
/// `runs` was shaped at the *logical* font size (matching what layout
/// itself measured — see [`paint_node`]'s own doc on `wrap_width`), so
/// every glyph's own outline scale and relative position is still in
/// logical units here; `scale_factor` blows both back up to the painted
/// canvas's own (possibly physical) units, the other half of that split.
fn paint_shaped_runs(
    buffer: &mut Canvas,
    runs: &[florui_text::ShapedRun],
    x: f32,
    y: f32,
    color: Rgba,
    scale_factor: f32,
) {
    let mut builder = PathBuilder::new();

    for run in runs {
        let Ok(font_ref) = FontRef::from_index(run.font.data.data(), run.font.index) else {
            continue;
        };
        let outlines = font_ref.outline_glyphs();
        let size = GlyphSize::new(run.font_size * scale_factor);
        // Without this, every glyph draws at the font's default variable
        // instance regardless of what was actually shaped — a bold run
        // would measure wider (Parley resolves the wght axis correctly
        // for layout) but paint no bolder at all.
        let coords: Vec<NormalizedCoord> = run
            .normalized_coords
            .iter()
            .map(|&bits| NormalizedCoord::from_bits(bits))
            .collect();
        let location = LocationRef::new(&coords);

        for glyph in &run.glyphs {
            let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                continue;
            };
            let mut pen = GlyphPen {
                builder: &mut builder,
                origin_x: x + glyph.x * scale_factor,
                origin_y: y + glyph.y * scale_factor,
            };
            let settings = DrawSettings::unhinted(size, location);
            let _ = outline.draw(settings, &mut pen);
        }
    }

    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;
    buffer.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Feeds a glyph's outline (font units, Y-up from its own baseline origin)
/// into a [`PathBuilder`] (screen space, Y-down), offsetting by the
/// glyph's pen position and flipping the Y axis for every emitted point.
struct GlyphPen<'a> {
    builder: &'a mut PathBuilder,
    origin_x: f32,
    origin_y: f32,
}

impl OutlinePen for GlyphPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(self.origin_x + x, self.origin_y - y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(self.origin_x + x, self.origin_y - y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(
            self.origin_x + cx0,
            self.origin_y - cy0,
            self.origin_x + x,
            self.origin_y - y,
        );
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            self.origin_x + cx0,
            self.origin_y - cy0,
            self.origin_x + cx1,
            self.origin_y - cy1,
            self.origin_x + x,
            self.origin_y - y,
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

fn to_tiny_skia_color(color: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(color.r, color.g, color.b, color.a)
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;
    use florui_style::InteractionState;
    use taffy::prelude::*;

    use super::*;

    fn pixel_rgb(buffer: &Canvas, x: u32, y: u32) -> [u8; 3] {
        let pixel = buffer.pixel(x, y).expect("pixel is within the canvas");
        [pixel.red(), pixel.green(), pixel.blue()]
    }

    #[test]
    fn paints_a_parent_and_a_smaller_positioned_child() {
        let tree: Element = view! {
            <div class="card">
                <div class="button" />
            </div>
        };
        let css = "
            .card { width: 100px; height: 60px; background-color: #1e1e22; padding-top: 10px; padding-left: 10px; }
            .button { width: 40px; height: 20px; background-color: #42734f; }
        ";

        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            100,
            60,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // Inside the card, outside the button: the card's own color.
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0x1e, 0x1e, 0x22]);
        // Inside the button (offset by the card's padding).
        assert_eq!(pixel_rgb(&buffer, 15, 15), [0x42, 0x73, 0x4f]);
    }

    #[test]
    fn a_border_paints_its_own_color_at_the_boxs_outer_edge() {
        let tree: Element = view! { <div class="box" /> };
        let css = "
            .box {
                width: 20px; height: 20px; background-color: #1e1e22;
                border-top-width: 4px; border-top-style: solid; border-top-color: #ff0000;
                border-left-width: 4px; border-left-style: solid; border-left-color: #ff0000;
            }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let node = arena.roots()[0];
        let width = layouts[&node].width.ceil() as u32;
        let height = layouts[&node].height.ceil() as u32;
        let buffer = paint_to_buffer(
            &mut font,
            width,
            height,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // Inside the 4px border strip, on both the top and left edges.
        assert_eq!(pixel_rgb(&buffer, 10, 1), [0xff, 0, 0], "top border");
        assert_eq!(pixel_rgb(&buffer, 1, 10), [0xff, 0, 0], "left border");
        // Past the border, into the content-box background — not still
        // border color, and not the canvas's own clear color either.
        assert_eq!(pixel_rgb(&buffer, 10, 10), [0x1e, 0x1e, 0x22]);
    }

    #[test]
    fn a_border_style_of_none_paints_no_border_despite_an_explicit_width() {
        let tree: Element = view! { <div class="box" /> };
        let css = "
            .box {
                width: 20px; height: 20px; background-color: #1e1e22;
                border-top-width: 4px; border-top-color: #ff0000;
            }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            20,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );
        // No border-style declared means border-style: none, real CSS's
        // own initial value — every pixel is the flat background color,
        // including the strip a rendered border would have occupied.
        for py in 0..20 {
            for px in 0..20 {
                assert_eq!(pixel_rgb(&buffer, px, py), [0x1e, 0x1e, 0x22]);
            }
        }
    }

    #[test]
    fn an_outset_box_shadow_paints_a_hard_edged_offset_copy_outside_the_box() {
        let tree: Element = view! {
            <div class="wrapper">
                <div class="box" />
            </div>
        };
        let css = "
            .wrapper {
                width: 40px; height: 40px; background-color: #000000;
                padding-top: 15px; padding-right: 15px; padding-bottom: 15px; padding-left: 15px;
            }
            .box { width: 10px; height: 10px; background-color: #1e1e22; box-shadow: 0px 8px 0px 0px #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            40,
            40,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // The box sits at (15,15)-(25,25); an 8px downward offset with no
        // blur/spread paints a hard-edged red copy at (15,25)-(25,33).
        assert_eq!(
            pixel_rgb(&buffer, 20, 28),
            [0xff, 0, 0],
            "the offset shadow band below the box"
        );
        assert_eq!(
            pixel_rgb(&buffer, 20, 20),
            [0x1e, 0x1e, 0x22],
            "inside the box's own footprint the shadow must not show through \
             (real CSS clips a shadow to outside its own border box)"
        );
        assert_eq!(
            pixel_rgb(&buffer, 20, 10),
            [0, 0, 0],
            "above the box, where the downward-only offset never reaches, \
             just the wrapper's own background"
        );
    }

    #[test]
    fn an_outset_box_shadow_with_only_spread_frames_the_box_without_a_ring_inside_it() {
        // Zero offset and a positive spread means the shadow's own
        // (grown) shape fully contains the box's own border box — the
        // "hole" this crate subtracts is then nested entirely inside the
        // shadow's outer rect, exactly the case an even-odd/XOR fill
        // would get wrong (it would paint a visible ring bleeding into
        // the box's own interior instead of leaving it alone).
        let tree: Element = view! {
            <div class="wrapper">
                <div class="box" />
            </div>
        };
        let css = "
            .wrapper {
                width: 40px; height: 40px; background-color: #000000;
                padding-top: 15px; padding-right: 15px; padding-bottom: 15px; padding-left: 15px;
            }
            .box { width: 10px; height: 10px; background-color: #1e1e22; box-shadow: 0px 0px 0px 4px #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            40,
            40,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // The 4px spread frames the box's (15,15)-(25,25) footprint with a
        // red ring from (11,11) to (29,29).
        assert_eq!(
            pixel_rgb(&buffer, 20, 13),
            [0xff, 0, 0],
            "inside the spread ring, just above the box"
        );
        assert_eq!(
            pixel_rgb(&buffer, 20, 20),
            [0x1e, 0x1e, 0x22],
            "the box's own interior must stay its own background, with no \
             shadow ring bleeding in from the nested hole"
        );
    }

    #[test]
    fn an_inset_box_shadow_paints_only_on_the_side_opposite_its_offset() {
        let tree: Element = view! {
            <div class="wrapper">
                <div class="box" />
            </div>
        };
        let css = "
            .wrapper {
                width: 40px; height: 40px; background-color: #000000;
                padding-top: 15px; padding-right: 15px; padding-bottom: 15px; padding-left: 15px;
            }
            .box { width: 10px; height: 10px; background-color: #1e1e22; box-shadow: inset 3px 3px 0px 0px #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            40,
            40,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // A positive (right/down) offset pushes the shadow-free hole
        // toward the bottom-right, so the visible inset shadow band shows
        // up along the box's own top and left interior edges instead.
        assert_eq!(
            pixel_rgb(&buffer, 20, 16),
            [0xff, 0, 0],
            "the inset shadow band along the box's own top interior edge"
        );
        assert_eq!(
            pixel_rgb(&buffer, 23, 23),
            [0x1e, 0x1e, 0x22],
            "the box's own bottom-right interior, inside the shifted hole, \
             must stay its own background"
        );
    }

    #[test]
    fn multiple_box_shadow_layers_paint_the_first_listed_one_on_top() {
        let tree: Element = view! { <div class="box" /> };
        let css = "
            .box {
                width: 10px; height: 10px; background-color: #1e1e22;
                box-shadow: 0px 4px 0px 0px #ff0000, 0px 4px 0px 0px #00ff00;
            }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            10,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // Both shadows land on the identical offset rect; the first
        // listed (red) must win, not whichever painted last by list
        // order alone.
        assert_eq!(
            pixel_rgb(&buffer, 5, 12),
            [0xff, 0, 0],
            "the first-listed shadow layer must paint on top of later ones"
        );
    }

    #[test]
    fn a_declared_blur_radius_is_carried_through_but_does_not_soften_the_painted_edge() {
        // florui-paint's own module doc: tiny-skia has no blur primitive,
        // so `blur-radius` parses and cascades but never softens what
        // gets painted — this shadow must paint identically to the same
        // shadow with `blur-radius: 0`, not partially transparent at its
        // edge the way a real blurred shadow would.
        let tree: Element = view! {
            <div class="wrapper">
                <div class="box" />
            </div>
        };
        let css = "
            .wrapper {
                width: 40px; height: 40px; background-color: #000000;
                padding-top: 15px; padding-right: 15px; padding-bottom: 15px; padding-left: 15px;
            }
            .box { width: 10px; height: 10px; background-color: #1e1e22; box-shadow: 0px 8px 20px 0px #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            40,
            40,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // Just inside the shadow's hard-edged shape: fully opaque red, not
        // a blurred/blended intermediate color.
        assert_eq!(pixel_rgb(&buffer, 20, 28), [0xff, 0, 0]);
        // Just past that edge, one pixel further from the box: back to
        // the plain unblurred background, with no soft blurred fringe.
        assert_eq!(pixel_rgb(&buffer, 20, 33), [0, 0, 0]);
    }

    #[test]
    fn writes_a_real_png_file() {
        let tree: Element = view! { <div class="card" /> };
        let css = ".card { width: 20px; height: 20px; background-color: #ff0000; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let dir = std::env::temp_dir().join(format!("florui-paint-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.png");

        paint_to_png(
            &mut font,
            &path,
            20,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        )
        .unwrap();

        let decoded = Pixmap::load_png(&path).unwrap();
        assert_eq!(pixel_rgb(&decoded, 10, 10), [0xff, 0, 0]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Real block layout has no absolute positioning yet, so it can never
    /// actually produce overlapping siblings — but the painter's contract
    /// (later source order wins an overlap) should hold for whatever
    /// layout it's handed, including a synthetic one. This is also the
    /// case the old area-sorted approach got wrong: two equal-area,
    /// fully-overlapping siblings tie on area, so which one ends up on top
    /// depended on `HashMap` iteration order — unspecified, and observed
    /// to vary from run to run.
    #[test]
    fn overlapping_siblings_paint_in_document_order_not_by_area() {
        let tree: Element = view! {
            <div>
                <div class="back" />
                <div class="front" />
            </div>
        };
        let css = ".back { background-color: #ff0000; } .front { background-color: #00ff00; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());

        let root = arena.roots()[0];
        let back = arena.children(root)[0];
        let front = arena.children(root)[1];
        let mut layouts = HashMap::new();
        layouts.insert(
            back,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        layouts.insert(
            front,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );

        let mut font = Font::load_embedded();
        let buffer = paint_to_buffer(
            &mut font,
            20,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );
        assert_eq!(
            pixel_rgb(&buffer, 10, 10),
            [0x00, 0xff, 0x00],
            "front is later in source order, so it should paint on top of back"
        );
    }

    #[test]
    fn a_fully_transparent_node_does_not_overwrite_what_is_beneath_it() {
        let tree: Element = view! {
            <div class="card">
                <div class="ghost" />
            </div>
        };
        let css = ".card { width: 20px; height: 20px; background-color: #1e1e22; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            20,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );
        assert_eq!(pixel_rgb(&buffer, 10, 10), [0x1e, 0x1e, 0x22]);
    }

    #[test]
    fn text_paints_visible_ink_in_its_declared_color() {
        let tree: Element = view! { <h2>{"H"}</h2> };
        let css = "h2 { color: #ff0000; font-size: 40px; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let node = arena.roots()[0];
        let width = layouts[&node].width.ceil() as u32;
        let height = layouts[&node].height.ceil() as u32;
        let buffer = paint_to_buffer(
            &mut font,
            width,
            height,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // A large "H" fills a good portion of its own tight box; scanning
        // for any red-ish pixel proves real glyph ink landed somewhere,
        // without pinning the test to one exact anti-aliased pixel.
        let mut found_ink = false;
        for py in 0..height {
            for px in 0..width {
                let pixel = pixel_rgb(&buffer, px, py);
                if pixel[0] > 0x80 && pixel[1] < 0x40 && pixel[2] < 0x40 {
                    found_ink = true;
                }
            }
        }
        assert!(found_ink, "expected at least one red glyph pixel");
    }

    #[test]
    fn bold_paints_thicker_strokes_than_regular_at_the_same_size() {
        // Real variation-coordinate rendering, not just wider layout
        // metrics: bold's own glyph outlines must cover more ink pixels
        // than regular's at the identical size — this is exactly the bug
        // a size-only measurement test would miss (the width changed, but
        // outlines still drew at the font's default instance).
        fn ink_pixel_count(font_weight: &str) -> u32 {
            let tree: Element = view! { <h2>{"H"}</h2> };
            let css =
                format!("h2 {{ color: #ff0000; font-size: 60px; font-weight: {font_weight}; }}");
            let arena = Arena::build(&tree);
            let rules = florui_style::parse_stylesheet(&css).unwrap();
            let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
            let mut font = Font::load_embedded();
            let layouts =
                florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT)
                    .unwrap();

            let node = arena.roots()[0];
            let width = layouts[&node].width.ceil() as u32;
            let height = layouts[&node].height.ceil() as u32;
            let buffer = paint_to_buffer(
                &mut font,
                width,
                height,
                Rgba::opaque(0, 0, 0),
                &arena,
                &styles,
                &layouts,
                1.0,
            );

            let mut count = 0;
            for py in 0..height {
                for px in 0..width {
                    if pixel_rgb(&buffer, px, py)[0] > 0x20 {
                        count += 1;
                    }
                }
            }
            count
        }

        let regular = ink_pixel_count("400");
        let bold = ink_pixel_count("700");
        assert!(
            bold > regular,
            "bold ({bold} ink pixels) must cover more area than regular ({regular})"
        );
    }

    #[test]
    fn a_real_inline_formatting_context_paints_ink_from_both_the_text_and_the_inline_element() {
        // `Element::node`/`Element::text` directly — real mixed inline
        // content, which `view!` has no ergonomic syntax for. Before
        // real inline formatting context support, `florui-paint` painted
        // a node with element children using only `arena.text_content`,
        // which flattens through nested elements and would have silently
        // dropped "B" — this test is exactly what would have caught that:
        // ink must appear on *both* sides of the box, not just the left.
        let tree = Element::node(
            "p",
            vec![],
            vec![
                Element::text("A"),
                Element::node("span", vec![], vec![Element::text("B")]),
            ],
        );
        let css = "p { color: #ff0000; font-size: 40px; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let node = arena.roots()[0];
        let width = layouts[&node].width.ceil() as u32;
        let height = layouts[&node].height.ceil() as u32;
        let buffer = paint_to_buffer(
            &mut font,
            width,
            height,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // A lower bar than "fully-opaque red" (the single-glyph "H" test's
        // own threshold): two adjacent, mostly-curved glyphs like "A"/"B"
        // at this size can anti-alias every pixel of their outline without
        // any one pixel reaching full coverage, unlike "H"'s thick straight
        // strokes — any non-trivial red channel value still distinguishes
        // real ink from the plain black background.
        let is_ink = |px: u32, py: u32| pixel_rgb(&buffer, px, py)[0] > 0x20;
        let half = width / 2;
        let mut ink_in_left_half = false;
        let mut ink_in_right_half = false;
        for py in 0..height {
            for px in 0..width {
                if is_ink(px, py) {
                    if px < half {
                        ink_in_left_half = true;
                    } else {
                        ink_in_right_half = true;
                    }
                }
            }
        }
        assert!(ink_in_left_half, "expected ink from \"A\" on the left");
        assert!(
            ink_in_right_half,
            "expected ink from the inline <span>'s own \"B\" on the right — \
             text_content-only painting would have dropped it entirely"
        );
    }

    #[test]
    fn empty_text_content_paints_no_ink() {
        let tree: Element = view! { <div class="card" /> };
        let css = ".card { width: 20px; height: 20px; background-color: #1e1e22; color: #ff0000; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            20,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );
        // No text content, so every pixel is exactly the flat background —
        // no stray glyph ink from a leaf with nothing to shape.
        for py in 0..20 {
            for px in 0..20 {
                assert_eq!(pixel_rgb(&buffer, px, py), [0x1e, 0x1e, 0x22]);
            }
        }
    }

    /// `paint_node` used to recurse once per tree level; iterative now.
    /// 1,200, matching florui-layout's own equivalent test: past the
    /// original 1,000-deep crash report, but not far past it.
    #[test]
    fn paint_to_buffer_survives_a_tree_far_deeper_than_the_old_recursion_limit() {
        let depth = 1_200;
        let mut tree = Element::node("div", vec![("class".into(), "leaf".into())], vec![]);
        for _ in 0..depth {
            tree = Element::node("div", vec![], vec![tree]);
        }
        let css = ".leaf { width: 5px; height: 5px; background-color: #ff0000; }";

        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            5,
            5,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );
        assert_eq!(pixel_rgb(&buffer, 0, 0), [0xff, 0x00, 0x00]);
    }

    #[test]
    fn text_still_wraps_at_the_logical_width_when_painted_onto_a_scaled_hidpi_canvas() {
        // A HiDPI caller (a real window, or `florui_conformance::engine`'s
        // own DPR fixtures) passes `layouts` already scaled up to physical
        // pixels — if painting reshaped text at that scaled box width
        // instead of dividing back to the logical width layout wrapped
        // against, it would wrap wider than the committed layout and
        // overflow the second line straight into empty canvas.
        let text = "Hello world this line is long enough to wrap";
        let css = "p { width: 100px; font-size: 16px; color: #ff0000; }";
        let tree = Element::node("p", vec![], vec![Element::text(text)]);

        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let mut font = Font::load_embedded();
        let layouts = florui_layout::compute_layout(
            &mut font,
            &arena,
            &styles,
            Size {
                width: AvailableSpace::Definite(200.0),
                height: AvailableSpace::MaxContent,
            },
        )
        .unwrap();

        let node = arena.roots()[0];
        let logical_width = layouts[&node].width;
        let logical_height = layouts[&node].height;

        // Proves the chosen text really does wrap to fewer lines at double
        // the width — otherwise the scenario below wouldn't exercise the
        // bug this test guards against at all.
        let unwrapped_at_double_width = font
            .measure_wrapped(
                florui_text::FontFamily::SansSerif,
                text,
                16.0,
                400.0,
                logical_width * 2.0,
            )
            .height;
        assert!(
            unwrapped_at_double_width < logical_height,
            "the chosen text must wrap to fewer lines at double the width, or this test proves \
             nothing"
        );

        let scale_factor = 2.0;
        let physical_layouts = layouts
            .iter()
            .map(|(&id, l)| {
                (
                    id,
                    BoxLayout {
                        x: l.x * scale_factor,
                        y: l.y * scale_factor,
                        width: l.width * scale_factor,
                        height: l.height * scale_factor,
                    },
                )
            })
            .collect();

        let width = (logical_width * scale_factor).ceil() as u32;
        let height = (logical_height * scale_factor).ceil() as u32;
        let buffer = paint_to_buffer(
            &mut font,
            width,
            height,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &physical_layouts,
            scale_factor,
        );

        // If painting had wrapped at the scaled (physical) width instead
        // of the logical one, the whole text would fit on its first line
        // and the canvas's bottom half — reserved by layout for the
        // second wrapped line — would be pure background, no ink.
        let is_ink = |px: u32, py: u32| pixel_rgb(&buffer, px, py)[0] > 0x20;
        let bottom_half_has_ink =
            (height / 2..height).any(|py| (0..width).any(|px| is_ink(px, py)));
        assert!(
            bottom_half_has_ink,
            "text painted onto a scaled canvas must still wrap at the logical width, filling \
             the second line layout reserved room for — not re-wrap wider and collapse onto \
             one line"
        );
    }

    #[test]
    fn a_glyph_painted_onto_a_scaled_hidpi_canvas_is_rasterized_at_the_scaled_size_not_just_repositioned()
     {
        // The bug this guards against: positioning glyphs at physical
        // coordinates (correct) while still rasterizing their outlines at
        // the *logical* font size paints text roughly `scale_factor` times
        // smaller than a real HiDPI render — box geometry still matches
        // (nothing here depends on glyph size), so a percent-different
        // pixel tolerance can pass even though the rendered ink itself is
        // visibly wrong, since thin glyph strokes cover little of a mostly
        // blank canvas.
        fn ink_height(scale_factor: f32) -> u32 {
            let tree: Element = view! { <h2>{"H"}</h2> };
            let css = "h2 { color: #ff0000; font-size: 40px; }";
            let arena = Arena::build(&tree);
            let rules = florui_style::parse_stylesheet(css).unwrap();
            let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
            let mut font = Font::load_embedded();
            let layouts =
                florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT)
                    .unwrap();

            let node = arena.roots()[0];
            let logical_width = layouts[&node].width;
            let logical_height = layouts[&node].height;
            let physical_layouts = layouts
                .iter()
                .map(|(&id, l)| {
                    (
                        id,
                        BoxLayout {
                            x: l.x * scale_factor,
                            y: l.y * scale_factor,
                            width: l.width * scale_factor,
                            height: l.height * scale_factor,
                        },
                    )
                })
                .collect();
            let width = (logical_width * scale_factor).ceil() as u32;
            let height = (logical_height * scale_factor).ceil() as u32;
            let buffer = paint_to_buffer(
                &mut font,
                width,
                height,
                Rgba::opaque(0, 0, 0),
                &arena,
                &styles,
                &physical_layouts,
                scale_factor,
            );

            let is_ink = |px: u32, py: u32| pixel_rgb(&buffer, px, py)[0] > 0x20;
            let mut min_y = None;
            let mut max_y = None;
            for py in 0..height {
                if (0..width).any(|px| is_ink(px, py)) {
                    min_y.get_or_insert(py);
                    max_y = Some(py);
                }
            }
            let min_y = min_y.expect("the glyph must paint some ink");
            let max_y = max_y.expect("the glyph must paint some ink");
            max_y - min_y + 1
        }

        let height_at_1x = ink_height(1.0);
        let height_at_2x = ink_height(2.0);
        let ratio = f64::from(height_at_2x) / f64::from(height_at_1x);
        assert!(
            (1.8..=2.2).contains(&ratio),
            "a glyph on a 2x-scaled canvas must paint roughly twice as tall as on an unscaled \
             one (ink height {height_at_1x}px -> {height_at_2x}px, ratio {ratio:.2}), not stay \
             at the logical size just repositioned"
        );
    }
}

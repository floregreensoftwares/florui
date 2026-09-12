//! Paints a styled, laid-out `florui_style::Arena` into pixels — the
//! "painting" stage at the end of the pipeline (components → elements →
//! style → boxes/layout → text → **painting**).
//!
//! # Scope
//!
//! Flat background-color rectangles, painted in real document order: a
//! node before its children, children in source order, so an overlap
//! always resolves to whichever box comes later in the tree — the same
//! rule real CSS painting follows for normal-flow boxes with no stacking
//! contexts. A leaf's own text (see [`florui_style::Arena::text_content`])
//! paints inside its padding box: glyph outlines come from
//! [`florui_text::Font::shape`] and are rasterized with
//! [skrifa](https://github.com/googlefonts/fontations)'s outline extractor
//! feeding a [tiny-skia](https://github.com/RazrFalcon/tiny-skia) path,
//! the same rasterizer backgrounds use — one painting backend, not two.
//! There are no borders, shadows, opacity, transforms, or clipping yet —
//! `florui_style` has no properties for any of those either.

use std::collections::HashMap;
use std::path::Path;

use florui_layout::{BoxLayout, absolute_position};
use florui_style::{Arena, ComputedStyle, NodeId, Rgba};
use florui_text::Font;
use skrifa::instance::{LocationRef, Size as GlyphSize};
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
pub fn paint_to_png(
    path: &Path,
    width: u32,
    height: u32,
    canvas: Rgba,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
) -> Result<(), PaintError> {
    paint_to_buffer(width, height, canvas, arena, styles, layouts)
        .save_png(path)
        .map_err(|source| PaintError {
            path: path.to_owned(),
            source,
        })
}

/// Same painting as [`paint_to_png`], returning the pixel buffer directly
/// instead of writing it to disk.
///
/// # Panics
///
/// Panics if `width` or `height` is zero.
pub fn paint_to_buffer(
    width: u32,
    height: u32,
    canvas: Rgba,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
) -> Canvas {
    let mut buffer =
        Pixmap::new(width, height).expect("paint_to_buffer requires a nonzero-sized canvas");
    buffer.fill(to_tiny_skia_color(canvas));

    // One embedded font for the whole tree, and its scratch shaping state
    // reused across every text-bearing node — matching florui_layout's own
    // per-pass `Font::load_embedded` convention.
    let mut font = Font::load_embedded();
    for &root in arena.roots() {
        paint_node(&mut buffer, arena, styles, layouts, &mut font, root);
    }
    buffer
}

/// Paints `node`'s own background and text, then its children in source
/// order, so an overlapping later node always wins over an earlier one —
/// real document-order painting rather than a heuristic (e.g. sorting by
/// box area) that only happens to agree with it for pure containment.
fn paint_node(
    buffer: &mut Canvas,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    font: &mut Font,
    node: NodeId,
) {
    if let Some(&layout) = layouts.get(&node) {
        let style = styles.get(&node);
        let (x, y) = absolute_position(arena, layouts, node);

        let background = style.map_or(Rgba::TRANSPARENT, |s| s.background_color);
        if background.a != 0 {
            fill_rect(buffer, x, y, layout.width, layout.height, background);
        }

        let text = arena.text_content(node);
        if !text.is_empty() {
            let color = style.map_or(Rgba::opaque(0, 0, 0), |s| s.color);
            let font_size = style.map_or(16.0, |s| s.font_size);
            let no_padding = florui_style::Edges {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            };
            let padding = style.map_or(no_padding, |s| s.padding);
            paint_text(
                buffer,
                font,
                text,
                font_size,
                color,
                x + padding.left,
                y + padding.top,
            );
        }
    }
    for &child in arena.children(node) {
        paint_node(buffer, arena, styles, layouts, font, child);
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

/// Shapes `text` and fills each glyph's outline at `(x, y)` — the top-left
/// of the line, in the same coordinate space as [`fill_rect`] — with
/// `color`. Skips a run whose font data doesn't parse, or an individual
/// glyph with no outline (e.g. genuinely missing from the font); a partial
/// render beats aborting the whole paint over one bad glyph.
fn paint_text(
    buffer: &mut Canvas,
    font: &mut Font,
    text: &str,
    font_size: f32,
    color: Rgba,
    x: f32,
    y: f32,
) {
    let shaped = font.shape(text, font_size);
    let mut builder = PathBuilder::new();

    for run in &shaped.runs {
        let Ok(font_ref) = FontRef::from_index(run.font.data.data(), run.font.index) else {
            continue;
        };
        let outlines = font_ref.outline_glyphs();
        let size = GlyphSize::new(run.font_size);

        for glyph in &run.glyphs {
            let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                continue;
            };
            let mut pen = GlyphPen {
                builder: &mut builder,
                origin_x: x + glyph.x,
                origin_y: y + glyph.y,
            };
            let settings = DrawSettings::unhinted(size, LocationRef::default());
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
        let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(100, 60, Rgba::opaque(0, 0, 0), &arena, &styles, &layouts);

        // Inside the card, outside the button: the card's own color.
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0x1e, 0x1e, 0x22]);
        // Inside the button (offset by the card's padding).
        assert_eq!(pixel_rgb(&buffer, 15, 15), [0x42, 0x73, 0x4f]);
    }

    #[test]
    fn writes_a_real_png_file() {
        let tree: Element = view! { <div class="card" /> };
        let css = ".card { width: 20px; height: 20px; background-color: #ff0000; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT).unwrap();

        let dir = std::env::temp_dir().join(format!("florui-paint-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.png");

        paint_to_png(
            &path,
            20,
            20,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
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

        let buffer = paint_to_buffer(20, 20, Rgba::opaque(0, 0, 0), &arena, &styles, &layouts);
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
        let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(20, 20, Rgba::opaque(0, 0, 0), &arena, &styles, &layouts);
        assert_eq!(pixel_rgb(&buffer, 10, 10), [0x1e, 0x1e, 0x22]);
    }

    #[test]
    fn text_paints_visible_ink_in_its_declared_color() {
        let tree: Element = view! { <h2>{"H"}</h2> };
        let css = "h2 { color: #ff0000; font-size: 40px; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT).unwrap();

        let node = arena.roots()[0];
        let width = layouts[&node].width.ceil() as u32;
        let height = layouts[&node].height.ceil() as u32;
        let buffer = paint_to_buffer(
            width,
            height,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
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
    fn empty_text_content_paints_no_ink() {
        let tree: Element = view! { <div class="card" /> };
        let css = ".card { width: 20px; height: 20px; background-color: #1e1e22; color: #ff0000; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(20, 20, Rgba::opaque(0, 0, 0), &arena, &styles, &layouts);
        // No text content, so every pixel is exactly the flat background —
        // no stray glyph ink from a leaf with nothing to shape.
        for py in 0..20 {
            for px in 0..20 {
                assert_eq!(pixel_rgb(&buffer, px, py), [0x1e, 0x1e, 0x22]);
            }
        }
    }
}

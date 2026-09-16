//! Paints a styled, laid-out `florui_style::Arena` into pixels — the
//! "painting" stage at the end of the pipeline (components → elements →
//! style → boxes/layout → text → **painting**).
//!
//! # Scope
//!
//! Flat background-color rectangles, solid borders, and hard-edged
//! box-shadows, painted in real paint order (see [`paint_order`]): a
//! node before its children, and within one node's own children,
//! document order — except a flex or grid child with an explicit
//! `z-index` reorders among its own siblings the same way real CSS
//! does, the only case florui can express `z-index` for yet (there is
//! no `position` property, so nothing here establishes a positioned
//! element's own stacking context). An overlap always resolves to
//! whichever box paints later. A leaf's own text (see
//! [`florui_style::Arena::text_content`]) paints inside its padding
//! box: glyph outlines come from [`florui_text::Font::shape`] and are
//! rasterized with [skrifa](https://github.com/googlefonts/fontations)'s
//! outline extractor feeding a
//! [tiny-skia](https://github.com/RazrFalcon/tiny-skia) path, the same
//! rasterizer backgrounds/borders/shadows use — one painting backend,
//! not several. A node with `opacity` below `1.0`, a `transform`, a
//! `filter`, or any combination paints itself and its whole subtree into
//! an offscreen buffer first, composited back as one group (see
//! [`paint_group`]) — real CSS's own group-opacity semantics (not a
//! per-primitive alpha multiply) and its own "transform moves the whole
//! rendered result, clip included" rule for transforms, both falling out
//! of the same offscreen-buffer mechanism for free: the buffer already
//! holds the group's content at its normal, untransformed position, so
//! compositing it back with a real matrix instead of the identity one
//! moves everything — background, border, already-clipped descendants —
//! as a single unit, pivoting around [`ComputedStyle::transform_origin`].
//! See [`resolve_transform`] for the supported `transform` subset and its
//! own matrix composition, and [`apply_filters`] for the documented
//! `filter` subset (`blur`/`brightness`/`contrast`/`saturate`), applied to
//! that same buffer before it's composited — real CSS's own
//! filter-then-composite order. `backdrop-filter` (same subset) works the
//! other way: [`apply_backdrop_filter`] samples what's already painted
//! behind a node, filters it, and writes it back before that node's own
//! background/border/content paint on top.
//!
//! `box-shadow`'s `blur-radius` is painted too, via a real Gaussian blur
//! — see [`blur`]'s own module doc: tiny-skia 0.11 (this crate's whole
//! rasterizer) has no blur or mask-filter primitive of its own, so this
//! crate rasterizes the shadow's own shape into a scratch alpha buffer,
//! blurs *that* by hand, and composites the result back onto the canvas
//! pixel by pixel — the one place in this crate that blends manually
//! instead of going through a `tiny_skia::Paint` fill (`filter: blur()`
//! reuses the same hand-rolled Gaussian, run over the group's own already-
//! rasterized pixels instead of a shape rasterized just for it). A node
//! whose `overflow` clips content (see [`clip_for_children`]) restricts
//! its own descendants — never its own border/background — to its
//! padding box; nested clips intersect. There is no border-radius yet —
//! `florui_style` has no property for it.

mod blur;

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use florui_layout::{BoxLayout, absolute_position};
use florui_style::{
    Arena, ComputedStyle, Display, FilterFunction, NodeId, Rgba, TransformFunction,
};
use florui_text::Font;
use skrifa::instance::{LocationRef, NormalizedCoord, Size as GlyphSize};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{
    BlendMode, FillRule, IntRect, Mask, Paint, PathBuilder, Pixmap, PixmapPaint,
    PremultipliedColorU8, Rect, Transform,
};

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
    paint_nodes(
        &mut buffer,
        arena,
        styles,
        layouts,
        font,
        None,
        arena.roots(),
        scale_factor,
        None,
    );
    buffer
}

/// Paints `nodes` (a set of siblings — document roots when
/// `parent_display` is `None`, one node's own children otherwise) and
/// their descendants onto `buffer`, in real paint order (see
/// [`paint_order`]).
///
/// Iterative for the common case — every node fully opaque and
/// untransformed, the same walk `paint_to_buffer` always did — but
/// recurses once per ancestor whose own [`ComputedStyle::opacity`] is
/// below `1.0`, whose own [`ComputedStyle::transform`] isn't empty, or
/// both, to render that ancestor's whole subtree into its own offscreen
/// buffer before compositing it back as a single group (see this
/// module's own doc on why that's not the same as multiplying each
/// descendant's own paint individually, or transforming each one on its
/// own). Recursion depth tracks the *nesting depth of these groups
/// specifically*, not overall tree depth — a subtree with neither still
/// walks iteratively, the same guarantee this module's own deep-tree test
/// already covers. A pathologically deep chain of nested groups could
/// still overflow the stack; unguarded for now, a smaller and far less
/// likely bound than the plain-tree-depth case that motivated this
/// function's own iterative design in the first place.
///
/// `clip`, if given, restricts every fill in this call (and everything it
/// recurses into) to the pixels it marks visible — inherited from an
/// ancestor whose own `overflow` clips its content; see
/// [`clip_for_children`] for how a node's own `overflow` narrows it
/// further for its own children. Shared via [`Rc`] rather than cloned:
/// most of a tree has no `overflow: hidden` ancestor at all (`clip` stays
/// `None` all the way down, the same zero-cost path as before this
/// existed), and even under one, only a node that *itself* clips ever
/// allocates a new mask — every sibling and descendant that doesn't
/// clip shares the same one.
#[allow(clippy::too_many_arguments)]
fn paint_nodes(
    buffer: &mut Canvas,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    font: &mut Font,
    parent_display: Option<Display>,
    nodes: &[NodeId],
    scale_factor: f32,
    clip: Option<Rc<Mask>>,
) {
    let mut stack: Vec<(NodeId, Option<Rc<Mask>>)> = paint_order(styles, parent_display, nodes)
        .into_iter()
        .rev()
        .map(|id| (id, clip.clone()))
        .collect();
    while let Some((node, node_clip)) = stack.pop() {
        let opacity = styles.get(&node).map_or(1.0, |s| s.opacity);
        if opacity <= 0.0 {
            // Real CSS: a fully transparent subtree still occupies its
            // own layout box and stays hit-testable, but paints nothing
            // at all — not "paint it and let zero alpha erase it," which
            // would still cost the same work for no visible result.
            continue;
        }
        let transform = match (styles.get(&node), layouts.get(&node)) {
            (Some(style), Some(&layout)) => {
                let (x, y) = absolute_position(arena, layouts, node);
                resolve_transform(style, &layout, x, y, scale_factor)
            }
            _ => Transform::identity(),
        };
        let has_filter = styles.get(&node).is_some_and(|s| !s.filter.is_empty());
        if opacity < 1.0 || !transform.is_identity() || has_filter {
            paint_group(
                buffer,
                arena,
                styles,
                layouts,
                font,
                node,
                scale_factor,
                node_clip,
                opacity,
                transform,
            );
            continue;
        }
        paint_node(
            buffer,
            arena,
            styles,
            layouts,
            font,
            node,
            scale_factor,
            node_clip.as_deref(),
        );
        let child_clip = clip_for_children(
            arena,
            styles,
            layouts,
            node,
            &node_clip,
            buffer.width(),
            buffer.height(),
        );
        let child_display = styles.get(&node).map(|s| s.display);
        stack.extend(
            paint_order(styles, child_display, arena.children(node))
                .into_iter()
                .rev()
                .map(|id| (id, child_clip.clone())),
        );
    }
}

/// The clip a node's own children paint under: `incoming` unchanged
/// unless `node` itself has [`ComputedStyle::overflow_clips`] set, in
/// which case its own padding box is intersected into it (or becomes the
/// whole clip, if there was no `incoming` one yet). Doesn't affect `node`
/// itself — real CSS's own `overflow` clips a box's *content*, not the
/// box's own border/background, which is why this only ever changes what
/// gets passed down, never what [`paint_node`] used for `node` itself.
///
/// A node's own text is a documented exception: it's painted by
/// [`paint_node`] using the *incoming* clip, not this function's result,
/// so a leaf whose own unbreakable text overflows its own
/// `overflow: hidden` box isn't self-clipped yet — only a *descendant*
/// extending past this box is. Narrower than real CSS, but the common
/// and far more visible case (clipping other elements, e.g. an image
/// container) works correctly today.
fn clip_for_children(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    node: NodeId,
    incoming: &Option<Rc<Mask>>,
    canvas_width: u32,
    canvas_height: u32,
) -> Option<Rc<Mask>> {
    let clips = styles.get(&node).is_some_and(|s| s.overflow_clips);
    if !clips {
        return incoming.clone();
    }
    let Some(&layout) = layouts.get(&node) else {
        return incoming.clone();
    };
    let (x, y) = absolute_position(arena, layouts, node);
    let border = styles.get(&node).map_or(NO_BORDER, |s| s.border);
    let padding_box = Rect::from_xywh(
        x + border.left.width,
        y + border.top.width,
        (layout.width - border.left.width - border.right.width).max(0.0),
        (layout.height - border.top.width - border.bottom.width).max(0.0),
    );
    let Some(padding_box) = padding_box else {
        return incoming.clone();
    };
    let mut path_builder = PathBuilder::new();
    path_builder.push_rect(padding_box);
    let Some(path) = path_builder.finish() else {
        return incoming.clone();
    };

    let mask = match incoming {
        Some(parent_mask) => {
            let mut mask = (**parent_mask).clone();
            mask.intersect_path(&path, FillRule::Winding, true, Transform::identity());
            mask
        }
        None => {
            let Some(mut mask) = Mask::new(canvas_width, canvas_height) else {
                return incoming.clone();
            };
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
            mask
        }
    };
    Some(Rc::new(mask))
}

const NO_BORDER: florui_style::Edges<florui_style::BorderSide> = florui_style::Edges {
    top: NO_BORDER_SIDE,
    right: NO_BORDER_SIDE,
    bottom: NO_BORDER_SIDE,
    left: NO_BORDER_SIDE,
};
const NO_BORDER_SIDE: florui_style::BorderSide = florui_style::BorderSide {
    width: 0.0,
    color: Rgba::TRANSPARENT,
};

/// Renders `node`'s own box and its whole subtree into a fresh,
/// transparent buffer the same size as `buffer`, applies `node`'s own
/// [`ComputedStyle::filter`] chain to that buffer's pixels (see
/// [`apply_filters`]), then composites the result onto `buffer` at
/// `opacity` through `transform` — real CSS's own "group opacity" (every
/// overlap *inside* the group still resolves at full strength against its
/// own siblings, later paints over earlier exactly as usual, and only the
/// group's own combined result is faded as one flat image — painting each
/// descendant at the reduced opacity individually instead would show
/// every overlap inside the group as a visibly different, doubled-up
/// alpha), real CSS's own `transform` semantics (the buffer already holds
/// `node` at its normal, transform-free position, so compositing it with a
/// real matrix instead of the identity one moves the *entire rendered
/// group* — background, border, and every already-clipped descendant
/// together — as a single rigid image, exactly what "transform
/// establishes its own coordinate system" means), and real CSS's own
/// filter-then-composite order (a filter transforms what the element
/// itself looks like *before* opacity fades it or transform repositions
/// it, not after). Called whenever any of the three conditions holds; a
/// node with more than one applies them together in this one function,
/// matching real CSS's own single stacking-context-establishing group.
///
/// A full canvas-sized buffer per group, uncached and unpooled — correct,
/// but not yet the bounded/pooled temporary surfaces real compositing
/// work eventually needs for many overlapping translucent or transformed
/// panels at once.
///
/// `clip` is an ancestor's clip, exactly as [`paint_node`] takes it —
/// applied both to what's painted *inside* the group (so a descendant
/// still respects an ancestor's `overflow: hidden` even though it's
/// rendered into a separate buffer first) and to the final composite
/// (redundant with the first once the inner paint already respected it,
/// but cheap and keeps this function correct even if that ever changes).
/// Left in the ancestor's own untransformed coordinate space in both
/// places — real CSS clips a transformed box's *rendered result* against
/// an ancestor's clip, not the other way around.
#[allow(clippy::too_many_arguments)]
fn paint_group(
    buffer: &mut Canvas,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    font: &mut Font,
    node: NodeId,
    scale_factor: f32,
    clip: Option<Rc<Mask>>,
    opacity: f32,
    transform: Transform,
) {
    let mut group = Pixmap::new(buffer.width(), buffer.height())
        .expect("paint_to_buffer requires a nonzero-sized canvas");
    paint_node(
        &mut group,
        arena,
        styles,
        layouts,
        font,
        node,
        scale_factor,
        clip.as_deref(),
    );
    let content_clip = clip_for_children(
        arena,
        styles,
        layouts,
        node,
        &clip,
        buffer.width(),
        buffer.height(),
    );
    let child_display = styles.get(&node).map(|s| s.display);
    paint_nodes(
        &mut group,
        arena,
        styles,
        layouts,
        font,
        child_display,
        arena.children(node),
        scale_factor,
        content_clip,
    );

    let filter = styles.get(&node).map_or(&[][..], |s| &s.filter[..]);
    apply_filters(&mut group, filter);

    let paint = PixmapPaint {
        opacity,
        ..Default::default()
    };
    buffer.draw_pixmap(0, 0, group.as_ref(), &paint, transform, clip.as_deref());
}

/// The real 2D affine matrix `style`'s own `transform` describes, pivoted
/// around its own `transform_origin` — [`Transform::identity()`] (a cheap
/// no-op composite, taken by [`paint_nodes`]'s own fast path) when
/// `transform` is empty, real CSS's own `none`.
///
/// `x`/`y` are `style`'s node's own absolute border-box position in the
/// canvas's own (possibly HiDPI-scaled) physical pixels — the same
/// position [`paint_node`] paints that box at. `scale_factor` converts a
/// `transform` value's own CSS-authored lengths (`translate(10px)`,
/// `matrix()`'s `e`/`f`) from the logical pixels they're specified in up
/// to the canvas's physical ones, exactly the split [`paint_shaped_runs`]
/// already needs for text and for the same reason: `layout` (and this
/// whole crate's coordinate space) already accounts for HiDPI, but a raw
/// CSS length read off [`ComputedStyle`] hasn't yet. A `<percentage>`
/// component needs no such correction — it's already relative to
/// `layout`'s own (already-scaled) box, so the same ratio holds
/// regardless of DPR.
///
/// Each function in [`ComputedStyle::transform`]'s own list folds into
/// one running matrix via [`Transform::pre_concat`], in authored order —
/// real CSS's own composition rule: `transform: A B` maps a point as
/// `A(B(point))`, so the *last*-listed function acts on the original
/// point first and the *first*-listed one acts last, in the coordinate
/// system every earlier function already established. `pre_concat`
/// builds exactly that: starting from the identity, concatenating `A`
/// then `B` leaves `A * B`, which maps a point as `A(B(point))` — the
/// same left-to-right accumulation, not the other order.
fn resolve_transform(
    style: &ComputedStyle,
    layout: &BoxLayout,
    x: f32,
    y: f32,
    scale_factor: f32,
) -> Transform {
    if style.transform.is_empty() {
        return Transform::identity();
    }
    let mut list = Transform::identity();
    for function in &style.transform {
        let next = match *function {
            TransformFunction::Translate(tx, ty) => Transform::from_translate(
                tx.length * scale_factor + tx.percentage * layout.width,
                ty.length * scale_factor + ty.percentage * layout.height,
            ),
            TransformFunction::Scale(sx, sy) => Transform::from_scale(sx, sy),
            TransformFunction::Rotate(degrees) => Transform::from_rotate(degrees),
            TransformFunction::Matrix { a, b, c, d, e, f } => {
                Transform::from_row(a, b, c, d, e * scale_factor, f * scale_factor)
            }
        };
        list = list.pre_concat(next);
    }

    let (origin_x_lp, origin_y_lp) = style.transform_origin;
    let origin_x = x + origin_x_lp.length * scale_factor + origin_x_lp.percentage * layout.width;
    let origin_y = y + origin_y_lp.length * scale_factor + origin_y_lp.percentage * layout.height;
    Transform::from_translate(origin_x, origin_y)
        .pre_concat(list)
        .pre_concat(Transform::from_translate(-origin_x, -origin_y))
}

/// `node`'s own border box at `(x, y, width, height)`, after applying
/// [`resolve_transform`] to its four corners and taking the axis-aligned
/// bounding box of the result — real CSS's own `getBoundingClientRect()`
/// semantics for a transformed element (CSSOM View's spec: the
/// *transformed* border box, not the pre-transform layout one). Painting
/// itself never needs this — [`resolve_transform`]'s own [`Transform`]
/// composites the whole rendered group directly instead of remeasuring a
/// box — but anything comparing this crate's own geometry against a real
/// browser's (`florui-conformance`'s own harness) needs to compare on the
/// same terms, or every transformed fixture would show a geometry
/// mismatch that isn't a real bug.
pub fn transformed_bounding_box(
    style: &ComputedStyle,
    layout: &BoxLayout,
    x: f32,
    y: f32,
    scale_factor: f32,
) -> (f32, f32, f32, f32) {
    let transform = resolve_transform(style, layout, x, y, scale_factor);
    let corners = [
        (x, y),
        (x + layout.width, y),
        (x, y + layout.height),
        (x + layout.width, y + layout.height),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &(px, py) in &corners {
        let mut point = tiny_skia::Point { x: px, y: py };
        transform.map_point(&mut point);
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

/// Samples `buffer` within `node`'s own border box, filters that sample
/// (see [`apply_filters`]), then writes it back with `BlendMode::Source`
/// (replace, not blend) — the filtered sample *is* the new backdrop.
/// Region is clamped to `buffer`'s own bounds first, since
/// [`Pixmap::clone_rect`] refuses a rect that isn't fully contained.
/// Sampling ignores `clip` (an ancestor's clip stops painting, it doesn't
/// erase what's already there); only the write-back respects it.
fn apply_backdrop_filter(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    functions: &[FilterFunction],
    clip: Option<&Mask>,
) {
    if functions.is_empty() {
        return;
    }
    let x0 = x.max(0.0).floor() as i32;
    let y0 = y.max(0.0).floor() as i32;
    let x1 = (x + width).min(buffer.width() as f32).ceil() as i32;
    let y1 = (y + height).min(buffer.height() as f32).ceil() as i32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let Some(region) = IntRect::from_xywh(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32) else {
        return;
    };
    let Some(mut backdrop) = buffer.clone_rect(region) else {
        return;
    };
    apply_filters(&mut backdrop, functions);
    let paint = PixmapPaint {
        blend_mode: BlendMode::Source,
        ..Default::default()
    };
    buffer.draw_pixmap(
        x0,
        y0,
        backdrop.as_ref(),
        &paint,
        Transform::identity(),
        clip,
    );
}

/// Applies `functions`' own chain to `pixmap`'s premultiplied pixels in
/// place, each function processing the *previous* one's own output in
/// authored order — real CSS's own filter-chain semantics (the
/// first-listed function reads the node's own unfiltered content).
fn apply_filters(pixmap: &mut Pixmap, functions: &[FilterFunction]) {
    let (width, height) = (pixmap.width(), pixmap.height());
    for function in functions {
        match *function {
            FilterFunction::Blur(radius) => blur_pixmap_in_place(pixmap, width, height, radius),
            FilterFunction::Brightness(factor) => {
                for pixel in pixmap.pixels_mut() {
                    *pixel = scale_premultiplied(*pixel, factor);
                }
            }
            FilterFunction::Contrast(factor) => {
                for pixel in pixmap.pixels_mut() {
                    // Real CSS's own formula operates in normalized
                    // [0, 1] space as `(color - 0.5) * amount + 0.5`;
                    // `127.5` is that same midpoint scaled to this
                    // buffer's own 0..255 range, not the nearby `128`.
                    *pixel = map_unpremultiplied(*pixel, |c| (c - 127.5) * factor + 127.5);
                }
            }
            FilterFunction::Saturate(factor) => {
                for pixel in pixmap.pixels_mut() {
                    *pixel = saturate_premultiplied(*pixel, factor);
                }
            }
        }
    }
}

/// `blur()`'s own Gaussian, run independently over each of `pixmap`'s own
/// four premultiplied channels (including alpha) rather than un-
/// premultiplying to straight color first: blurring straight color would
/// let a fully transparent pixel's own arbitrary RGB bleed into a blurred
/// edge — the dark/bright fringe this crate's own module doc on
/// premultiplied handling warns against — while blurring every
/// premultiplied channel identically keeps `rgb <= a` everywhere the
/// source did, the same invariant [`blur::gaussian_blur_in_place`]'s own
/// per-channel treatment preserves for any linear operation.
///
/// `radius_px` is real CSS's own `<length>` argument to `blur()`, already
/// the Gaussian's own standard deviation per the Filter Effects spec — a
/// *different* correspondence than `box-shadow`'s own `blur-radius`
/// (`radius / 2`, see [`rasterize_and_blur`]'s own doc), since the two
/// properties define the relationship differently, not a shared radius
/// convention this crate could factor into one constant.
fn blur_pixmap_in_place(pixmap: &mut Pixmap, width: u32, height: u32, radius_px: f32) {
    if radius_px <= 0.0 {
        return;
    }
    let pixel_count = (width as usize) * (height as usize);
    let mut channels = [
        vec![0u8; pixel_count],
        vec![0u8; pixel_count],
        vec![0u8; pixel_count],
        vec![0u8; pixel_count],
    ];
    for (i, pixel) in pixmap.pixels().iter().enumerate() {
        channels[0][i] = pixel.red();
        channels[1][i] = pixel.green();
        channels[2][i] = pixel.blue();
        channels[3][i] = pixel.alpha();
    }
    for channel in &mut channels {
        blur::gaussian_blur_in_place(channel, width, height, radius_px);
    }
    for (i, pixel) in pixmap.pixels_mut().iter_mut().enumerate() {
        let alpha = channels[3][i];
        *pixel = PremultipliedColorU8::from_rgba(
            channels[0][i].min(alpha),
            channels[1][i].min(alpha),
            channels[2][i].min(alpha),
            alpha,
        )
        .unwrap_or(PremultipliedColorU8::TRANSPARENT);
    }
}

/// Scales `pixel`'s own color by `factor` (real CSS's own `brightness()`),
/// clamping each channel to `pixel`'s own alpha rather than `255` —
/// scaling a premultiplied channel by a non-negative factor and clamping
/// to alpha is exactly equivalent to un-premultiplying, scaling and
/// clamping to `255`, then re-premultiplying, without the intermediate
/// division. `factor` is never negative — real CSS's own grammar already
/// forbids it (see [`crate::cascade::FilterFunction::Brightness`]'s own
/// doc via [`crate::cascade`]).
fn scale_premultiplied(pixel: PremultipliedColorU8, factor: f32) -> PremultipliedColorU8 {
    let alpha = pixel.alpha();
    let scale = |channel: u8| ((channel as f32) * factor).round().clamp(0.0, alpha as f32) as u8;
    PremultipliedColorU8::from_rgba(
        scale(pixel.red()),
        scale(pixel.green()),
        scale(pixel.blue()),
        alpha,
    )
    .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// Un-premultiplies `pixel`, applies `f` to each of its own R/G/B channels
/// independently (`contrast()`'s own affine remap doesn't commute with
/// premultiplication the way [`scale_premultiplied`]'s plain scale does,
/// so this un-premultiplies first), then re-premultiplies — a fully
/// transparent pixel passes through unchanged rather than dividing by a
/// zero alpha.
fn map_unpremultiplied(
    pixel: PremultipliedColorU8,
    mut f: impl FnMut(f32) -> f32,
) -> PremultipliedColorU8 {
    let alpha = pixel.alpha();
    if alpha == 0 {
        return pixel;
    }
    let unpremultiply = |channel: u8| (channel as f32) * 255.0 / (alpha as f32);
    let repremultiply = |channel: f32| {
        ((channel.clamp(0.0, 255.0) * (alpha as f32) / 255.0).round() as u8).min(alpha)
    };
    PremultipliedColorU8::from_rgba(
        repremultiply(f(unpremultiply(pixel.red()))),
        repremultiply(f(unpremultiply(pixel.green()))),
        repremultiply(f(unpremultiply(pixel.blue()))),
        alpha,
    )
    .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// `saturate()`'s own luminance-preserving mix: real CSS's own saturate
/// matrix (CSS Filter Effects' own reference to SVG's `feColorMatrix
/// type="saturate"`) is algebraically identical to mixing each channel
/// toward its own Rec.-601-ish luma at `1 - factor` and keeping the rest
/// at `factor` — `factor` `1.0` is a no-op, `0.0` is grayscale, and above
/// `1.0` oversaturates.
fn saturate_premultiplied(pixel: PremultipliedColorU8, factor: f32) -> PremultipliedColorU8 {
    let alpha = pixel.alpha();
    if alpha == 0 {
        return pixel;
    }
    let unpremultiply = |channel: u8| (channel as f32) * 255.0 / (alpha as f32);
    let r = unpremultiply(pixel.red());
    let g = unpremultiply(pixel.green());
    let b = unpremultiply(pixel.blue());
    let luma = 0.213 * r + 0.715 * g + 0.072 * b;
    let mix = |channel: f32| luma + (channel - luma) * factor;
    let repremultiply = |channel: f32| {
        ((channel.clamp(0.0, 255.0) * (alpha as f32) / 255.0).round() as u8).min(alpha)
    };
    PremultipliedColorU8::from_rgba(
        repremultiply(mix(r)),
        repremultiply(mix(g)),
        repremultiply(mix(b)),
        alpha,
    )
    .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// `nodes` (a set of siblings — document roots when `parent_display` is
/// `None`, one node's own children otherwise) sorted into real paint
/// order — back to front, so a caller walking this list in order and
/// painting each one gets later entries drawn on top of earlier ones, the
/// same "later wins" rule [`paint_node`]'s own doc already establishes
/// for document order.
///
/// Real CSS only gives `z-index` an effect on a positioned element, a
/// flex item, or a grid item — this crate has no `position` property yet
/// (see [`ComputedStyle::z_index`]'s own doc), so `parent_display` is
/// what decides whether `z_index` applies at all: `None` (no parent, i.e.
/// a set of document roots) or anything but [`Display::Flex`]/
/// [`Display::Grid`] leaves `nodes` in plain document order untouched,
/// matching a plain block/inline child's `z-index` having no real effect.
/// Only inside a flex or grid container are siblings actually reordered,
/// by [`ComputedStyle::z_index`] ascending — `auto` (`None`) sorts as `0`
/// for comparison purposes only, its own semantic meaning otherwise
/// unaffected. Ties (including every sibling at the default `auto`, the
/// common case even inside a flex/grid container) keep their original
/// document order: [`Vec::sort_by_key`] is a stable sort, so a flex/grid
/// container with no `z-index` anywhere paints in plain document order,
/// unchanged from before this function existed.
pub fn paint_order(
    styles: &HashMap<NodeId, ComputedStyle>,
    parent_display: Option<Display>,
    nodes: &[NodeId],
) -> Vec<NodeId> {
    if !matches!(parent_display, Some(Display::Flex) | Some(Display::Grid)) {
        return nodes.to_vec();
    }
    let mut ordered = nodes.to_vec();
    ordered.sort_by_key(|id| styles.get(id).and_then(|s| s.z_index).unwrap_or(0));
    ordered
}

/// Paints `node`'s own background and text — document-order painting
/// (an overlapping later node always wins) comes from the caller's own
/// pre-order walk over the whole tree, not from this function recursing.
/// A `backdrop-filter` runs first (see [`apply_backdrop_filter`]), so
/// `node`'s own background/border/content paint on top of the already-
/// filtered backdrop, not the other way around.
///
/// `clip`, if given, is an *ancestor's* clip (see [`clip_for_children`]):
/// it restricts everything painted here, but `node`'s own `overflow`
/// never does — real CSS never clips a box's own border/background
/// against its own content-clip, only a descendant's.
#[allow(clippy::too_many_arguments)]
fn paint_node(
    buffer: &mut Canvas,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    font: &mut Font,
    node: NodeId,
    scale_factor: f32,
    clip: Option<&Mask>,
) {
    if let Some(&layout) = layouts.get(&node) {
        let style = styles.get(&node);
        let (x, y) = absolute_position(arena, layouts, node);

        let backdrop_filter: &[FilterFunction] = style.map_or(&[][..], |s| &s.backdrop_filter[..]);
        apply_backdrop_filter(
            buffer,
            x,
            y,
            layout.width,
            layout.height,
            backdrop_filter,
            clip,
        );

        let border = style.map_or(NO_BORDER, |s| s.border);
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
            fill_rect(buffer, x, y, layout.width, layout.height, background, clip);
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

        paint_border(buffer, x, y, layout.width, layout.height, border, clip);

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
                    clip,
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
                        clip,
                    },
                );
            }
        }
    }
}

fn fill_rect(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Rgba,
    clip: Option<&Mask>,
) {
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
    buffer.fill_rect(rect, &paint, Transform::identity(), clip);
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
#[allow(clippy::too_many_arguments)]
fn paint_border(
    buffer: &mut Canvas,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    border: florui_style::Edges<florui_style::BorderSide>,
    clip: Option<&Mask>,
) {
    if border.top.width > 0.0 {
        fill_rect(
            buffer,
            x,
            y,
            width,
            border.top.width,
            border.top.color,
            clip,
        );
    }
    if border.bottom.width > 0.0 {
        fill_rect(
            buffer,
            x,
            y + height - border.bottom.width,
            width,
            border.bottom.width,
            border.bottom.color,
            clip,
        );
    }
    if border.left.width > 0.0 {
        fill_rect(
            buffer,
            x,
            y,
            border.left.width,
            height,
            border.left.color,
            clip,
        );
    }
    if border.right.width > 0.0 {
        fill_rect(
            buffer,
            x + width - border.right.width,
            y,
            border.right.width,
            height,
            border.right.color,
            clip,
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
        match (inset, shadow.blur_radius > 0.0) {
            (false, false) => paint_outset_shadow(buffer, x, y, width, height, shadow),
            (false, true) => paint_outset_shadow_blurred(buffer, x, y, width, height, shadow),
            (true, false) => paint_inset_shadow(buffer, x, y, width, height, border, shadow),
            (true, true) => paint_inset_shadow_blurred(buffer, x, y, width, height, border, shadow),
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
        fill_rect(
            buffer,
            outer_x,
            outer_y,
            outer_width,
            outer_height,
            color,
            None,
        );
        return;
    }

    fill_rect(
        buffer,
        outer_x,
        outer_y,
        outer_width,
        clip_y0 - outer_y,
        color,
        None,
    );
    fill_rect(
        buffer,
        outer_x,
        clip_y1,
        outer_width,
        outer_y1 - clip_y1,
        color,
        None,
    );
    fill_rect(
        buffer,
        outer_x,
        clip_y0,
        clip_x0 - outer_x,
        clip_y1 - clip_y0,
        color,
        None,
    );
    fill_rect(
        buffer,
        clip_x1,
        clip_y0,
        outer_x1 - clip_x1,
        clip_y1 - clip_y0,
        color,
        None,
    );
}

/// A blurred coverage buffer, plus the canvas pixel its own `(0, 0)`
/// lands on — [`rasterize_and_blur`]'s own output, read back by
/// [`composite_blurred_shadow`].
struct BlurredShape {
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
    coverage: Vec<u8>,
}

/// Stamps a `shape_width x shape_height` rect (at `shape_x`/`shape_y`,
/// canvas coordinates) into a fresh buffer covering `region` padded by
/// [`blur::kernel_radius`] on every side, then blurs the whole buffer with
/// a real Gaussian of standard deviation `blur_radius / 2.0` (see
/// `blur`'s own module doc for that CSS-spec correspondence). `region` is
/// the caller's own region of interest — the only part of the result it
/// actually reads back — which may differ from the shape itself: an
/// outset shadow's shape *is* its region of interest, but an inset
/// shadow's region of interest is the padding box, not the (differently
/// sized/positioned) hole shape blurred within it. Padding the buffer by
/// the kernel's own reach beyond `region`, not just beyond the shape,
/// keeps every value the caller will actually read genuinely unaffected
/// by this buffer's own edges — see [`blur::gaussian_blur_in_place`]'s
/// own doc on why that padding has to be real, not merely clamped.
///
/// Returns `None` for a degenerate (non-positive) region — nothing to
/// rasterize or read back.
#[allow(clippy::too_many_arguments)]
fn rasterize_and_blur(
    region_x: f32,
    region_y: f32,
    region_width: f32,
    region_height: f32,
    shape_x: f32,
    shape_y: f32,
    shape_width: f32,
    shape_height: f32,
    blur_radius: f32,
) -> Option<BlurredShape> {
    if region_width <= 0.0 || region_height <= 0.0 {
        return None;
    }
    let sigma = blur_radius / 2.0;
    let pad = blur::kernel_radius(sigma) as f32;

    // Snapped to a whole pixel so every later coordinate in this buffer's
    // own local space is an exact integer offset from a real canvas pixel
    // — composite_blurred_shadow then never needs to re-round a float.
    let origin_x = (region_x - pad).round() as i32;
    let origin_y = (region_y - pad).round() as i32;
    let width = (region_width + 2.0 * pad).ceil().max(1.0) as u32;
    let height = (region_height + 2.0 * pad).ceil().max(1.0) as u32;

    let mut coverage = vec![0u8; (width as usize) * (height as usize)];
    stamp_rect(
        &mut coverage,
        width,
        height,
        shape_x - origin_x as f32,
        shape_y - origin_y as f32,
        shape_width,
        shape_height,
        255,
    );

    blur::gaussian_blur_in_place(&mut coverage, width, height, sigma);

    Some(BlurredShape {
        origin_x,
        origin_y,
        width,
        height,
        coverage,
    })
}

/// Fills the (clamped-to-buffer) rectangle at `local_x`/`local_y` with
/// `value` — [`rasterize_and_blur`]'s own way of stamping a shape's
/// unblurred, hard-edged silhouette before blurring it. A rect that
/// doesn't overlap the buffer at all (including a non-positive width or
/// height — an inset shadow's hole erased entirely by its own spread, the
/// same case [`fill_rect_minus_hole`] already handles for the unblurred
/// path) simply stamps nothing.
#[allow(clippy::too_many_arguments)]
fn stamp_rect(
    buf: &mut [u8],
    buf_width: u32,
    buf_height: u32,
    local_x: f32,
    local_y: f32,
    width: f32,
    height: f32,
    value: u8,
) {
    let x0 = local_x.max(0.0).round() as i64;
    let y0 = local_y.max(0.0).round() as i64;
    let x1 = (local_x + width).min(buf_width as f32).round() as i64;
    let y1 = (local_y + height).min(buf_height as f32).round() as i64;
    for row in y0.max(0)..y1.max(0) {
        for col in x0.max(0)..x1.max(0) {
            buf[(row as u32 * buf_width + col as u32) as usize] = value;
        }
    }
}

/// An outer (drop) shadow with a real declared blur: the same outer
/// shape [`paint_outset_shadow`] paints, but blurred (real CSS blurs the
/// shadow shape itself, then clips the *already-blurred* result away
/// from the box's own border box — the clip itself stays a hard edge,
/// only the shape's own outer boundary softens).
fn paint_outset_shadow_blurred(
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

    let Some(blurred) = rasterize_and_blur(
        outer_x,
        outer_y,
        outer_width,
        outer_height,
        outer_x,
        outer_y,
        outer_width,
        outer_height,
        shadow.blur_radius,
    ) else {
        return;
    };

    composite_blurred_shadow(
        buffer,
        &blurred,
        shadow.color,
        false,
        |canvas_x, canvas_y| {
            // Excluded from the box's own border box — the hard clip real
            // CSS applies to an outer shadow regardless of its own blur.
            canvas_x < x || canvas_x >= x + width || canvas_y < y || canvas_y >= y + height
        },
    );
}

/// An inset shadow with a real declared blur. Blurring the *hole* shape
/// directly (rather than "padding box minus hole", inverted before
/// blurring) and inverting the coverage only at composite time relies on
/// the Gaussian blur's own linearity — `blur(255 - hole) == 255 -
/// blur(hole)` wherever the buffer's padding is real (see
/// [`rasterize_and_blur`]'s own doc) — so the same rasterize-then-blur
/// routine [`paint_outset_shadow_blurred`] uses works here too, with the
/// hole as the shape and the padding box as the region of interest.
fn paint_inset_shadow_blurred(
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

    let Some(blurred) = rasterize_and_blur(
        padding_x,
        padding_y,
        padding_width,
        padding_height,
        hole_x,
        hole_y,
        hole_width,
        hole_height,
        shadow.blur_radius,
    ) else {
        return;
    };

    composite_blurred_shadow(
        buffer,
        &blurred,
        shadow.color,
        true,
        |canvas_x, canvas_y| {
            // The hard clip real CSS applies to an inset shadow: never past
            // its own padding box, regardless of its own blur.
            canvas_x >= padding_x
                && canvas_x < padding_x + padding_width
                && canvas_y >= padding_y
                && canvas_y < padding_y + padding_height
        },
    );
}

/// Blends `blurred`'s own coverage (or, with `invert`, `255 -` it — see
/// [`paint_inset_shadow_blurred`]'s own doc for why that's correct) onto
/// `buffer` in `color`, skipping any pixel `allowed` rejects or that
/// falls outside `buffer`'s own bounds.
fn composite_blurred_shadow(
    buffer: &mut Canvas,
    blurred: &BlurredShape,
    color: Rgba,
    invert: bool,
    allowed: impl Fn(f32, f32) -> bool,
) {
    let canvas_width = buffer.width() as i64;
    let canvas_height = buffer.height() as i64;

    for local_y in 0..blurred.height {
        let canvas_y = blurred.origin_y as i64 + local_y as i64;
        if canvas_y < 0 || canvas_y >= canvas_height {
            continue;
        }
        for local_x in 0..blurred.width {
            let canvas_x = blurred.origin_x as i64 + local_x as i64;
            if canvas_x < 0 || canvas_x >= canvas_width {
                continue;
            }
            if !allowed(canvas_x as f32, canvas_y as f32) {
                continue;
            }

            let raw = blurred.coverage[(local_y * blurred.width + local_x) as usize];
            let coverage = if invert { 255 - raw } else { raw };
            if coverage == 0 {
                continue;
            }

            let index = (canvas_y as u32 * buffer.width() + canvas_x as u32) as usize;
            let dst = buffer.pixels()[index];
            buffer.pixels_mut()[index] = blend_source_over(dst, color, coverage);
        }
    }
}

/// `u16` fixed-point `a * b / 255`, rounded to nearest — the one
/// multiply-divide every channel below needs, pulled out once so the
/// rounding stays consistent across all of them.
fn mul_div_255(a: u8, b: u8) -> u8 {
    ((a as u16 * b as u16 + 127) / 255) as u8
}

/// Standard Porter-Duff "`A` over `B`", both operands already
/// premultiplied — the real per-pixel compositing math
/// [`composite_blurred_shadow`] needs, that [`fill_rect`]'s own calls
/// into tiny-skia get for free from the library instead: this path
/// writes into an already-painted `Canvas` pixel by pixel, not through a
/// fresh `tiny_skia::Paint` fill, so there's no library call already
/// doing this blend to reuse. `src_color`/`src_coverage` are straight
/// (non-premultiplied) — the same shape [`florui_style::Rgba`] and this
/// crate's own alpha masks already use — converted to a premultiplied
/// source here before blending.
fn blend_source_over(
    dst: PremultipliedColorU8,
    src_color: Rgba,
    src_coverage: u8,
) -> PremultipliedColorU8 {
    let src_a = mul_div_255(src_color.a, src_coverage);
    let src_r = mul_div_255(src_color.r, src_a);
    let src_g = mul_div_255(src_color.g, src_a);
    let src_b = mul_div_255(src_color.b, src_a);

    let inv = 255 - src_a;
    let out_a = (src_a as u16 + mul_div_255(dst.alpha(), inv) as u16).min(255) as u8;
    let clamp_channel = |src: u8, dst: u8| -> u8 {
        let value = src as u16 + mul_div_255(dst, inv) as u16;
        value.min(out_a as u16) as u8
    };
    let out_r = clamp_channel(src_r, dst.red());
    let out_g = clamp_channel(src_g, dst.green());
    let out_b = clamp_channel(src_b, dst.blue());

    PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a).unwrap_or(dst)
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
    clip: Option<&'a Mask>,
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
        clip,
    } = params;
    let shaped = font.shape_wrapped(font_family, text, font_size, font_weight, wrap_width);
    paint_shaped_runs(buffer, &shaped.runs, x, y, color, scale_factor, clip);
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
#[allow(clippy::too_many_arguments)]
fn paint_shaped_runs(
    buffer: &mut Canvas,
    runs: &[florui_text::ShapedRun],
    x: f32,
    y: f32,
    color: Rgba,
    scale_factor: f32,
    clip: Option<&Mask>,
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
        clip,
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
    fn a_declared_blur_radius_softens_the_shadows_own_edge_into_a_gradient() {
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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

        // The box's own interior never shows the shadow at all, blurred
        // or not — real CSS clips the *already-blurred* shape away from
        // the border box, the clip itself staying a hard edge.
        assert_eq!(
            pixel_rgb(&buffer, 20, 20),
            [0x1e, 0x1e, 0x22],
            "blur must not bleed into the box's own footprint"
        );

        // Scanning straight down from the box's own bottom edge, at least
        // one pixel must land strictly between fully red and fully
        // background — proof of a real gradient, not a hard 0/255 step.
        let mut found_partial = false;
        for py in 25..39u32 {
            let [r, g, b] = pixel_rgb(&buffer, 20, py);
            if (1..0xff).contains(&r) && g == 0 && b == 0 {
                found_partial = true;
            }
        }
        assert!(
            found_partial,
            "expected a genuine red gradient below the box, found only hard 0/255 steps"
        );
    }

    #[test]
    fn an_inset_shadow_with_blur_softens_near_its_own_edges_but_stays_hard_at_the_padding_box() {
        let tree: Element = view! {
            <div class="wrapper">
                <div class="box" />
            </div>
        };
        let css = "
            .wrapper {
                width: 50px; height: 50px; background-color: #000000;
                padding-top: 15px; padding-right: 15px; padding-bottom: 15px; padding-left: 15px;
            }
            .box { width: 20px; height: 20px; background-color: #1e1e22; box-shadow: inset 0px 0px 6px 0px #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
        let mut font = Font::load_embedded();
        let layouts =
            florui_layout::compute_layout(&mut font, &arena, &styles, Size::MAX_CONTENT).unwrap();

        let buffer = paint_to_buffer(
            &mut font,
            50,
            50,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        );

        // Deep in the box's own center (well beyond the blur's own
        // reach from any edge): completely untouched by the shadow.
        assert_eq!(
            pixel_rgb(&buffer, 25, 25),
            [0x1e, 0x1e, 0x22],
            "a zero-offset inset shadow with a small blur must not reach the box's own center"
        );
        // One pixel in from the box's own left edge: inside the blur's
        // reach, so a genuine partial red tint, not the flat background.
        let [near_edge_r, _, near_edge_b] = pixel_rgb(&buffer, 16, 25);
        assert!(
            near_edge_r > 0x1e && near_edge_b < 0x22,
            "expected a visible red tint blended in near the box's own edge, got \
             rgb component readings r={near_edge_r:#x} b={near_edge_b:#x}"
        );
        // Outside the box (and so outside the padding box) entirely: the
        // hard clip at the padding box still holds despite the blur —
        // no soft fringe leaking past it.
        assert_eq!(
            pixel_rgb(&buffer, 10, 25),
            [0, 0, 0],
            "an inset shadow, blurred or not, must never paint past its own padding box"
        );
    }

    #[test]
    fn writes_a_real_png_file() {
        let tree: Element = view! { <div class="card" /> };
        let css = ".card { width: 20px; height: 20px; background-color: #ff0000; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

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
    fn a_flex_items_positive_z_index_paints_it_above_a_later_sibling() {
        // "back" is later in source order (would win a plain document-order
        // tie), but "front"'s own explicit z-index outranks it — real CSS's
        // own rule for a flex item, which this is: z-index applies without
        // needing `position` at all.
        let tree: Element = view! {
            <div class="row">
                <div class="front" />
                <div class="back" />
            </div>
        };
        let css = "
            .row { display: flex; }
            .front { background-color: #00ff00; z-index: 1; }
            .back { background-color: #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let row = arena.roots()[0];
        let front = arena.children(row)[0];
        let back = arena.children(row)[1];
        let mut layouts = HashMap::new();
        layouts.insert(
            row,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        for child in [front, back] {
            layouts.insert(
                child,
                BoxLayout {
                    x: 0.0,
                    y: 0.0,
                    width: 20.0,
                    height: 20.0,
                },
            );
        }

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
            "front's positive z-index must win over back's later source position"
        );
    }

    #[test]
    fn z_index_has_no_effect_outside_a_flex_or_grid_container() {
        // Same shape as the flex test above (later source order beaten by
        // an earlier sibling's positive z-index), but the parent is a
        // plain block — real CSS gives z-index no effect there at all, so
        // document order (later wins) must still hold.
        let tree: Element = view! {
            <div class="column">
                <div class="front" />
                <div class="back" />
            </div>
        };
        let css = "
            .front { background-color: #00ff00; z-index: 1; }
            .back { background-color: #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let column = arena.roots()[0];
        let front = arena.children(column)[0];
        let back = arena.children(column)[1];
        let mut layouts = HashMap::new();
        layouts.insert(
            column,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        for child in [front, back] {
            layouts.insert(
                child,
                BoxLayout {
                    x: 0.0,
                    y: 0.0,
                    width: 20.0,
                    height: 20.0,
                },
            );
        }

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
            [0xff, 0x00, 0x00],
            "z-index on a plain block child has no real CSS effect; back is later in \
             source order and must still win"
        );
    }

    #[test]
    fn paint_order_sorts_flex_children_by_z_index_ascending_with_stable_ties() {
        let tree: Element = view! {
            <div class="row">
                <div class="a" />
                <div class="b" />
                <div class="c" />
                <div class="d" />
            </div>
        };
        // `a` and `c` tie at 2; `d` is left at the default `auto`.
        let css =
            ".row { display: flex; } .a { z-index: 2; } .b { z-index: -1; } .c { z-index: 2; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let row = arena.roots()[0];
        let children = arena.children(row).to_vec();
        let (a, b, c, d) = (children[0], children[1], children[2], children[3]);

        let ordered = paint_order(&styles, Some(florui_style::Display::Flex), &children);
        assert_eq!(
            ordered,
            vec![b, d, a, c],
            "ascending by z-index (-1, then auto treated as 0, then the 2/2 tie kept in its \
             own original a-before-c order)"
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
    fn an_opacity_below_1_fades_a_solid_box_toward_whatever_is_behind_it() {
        let tree: Element = view! { <div class="ghost" /> };
        let css = ".ghost { opacity: 0.5; background-color: #ff0000; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
        let node = arena.roots()[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            node,
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
        let pixel = pixel_rgb(&buffer, 10, 10);
        assert!(
            (100..160).contains(&pixel[0]) && pixel[1] == 0 && pixel[2] == 0,
            "an opaque red box at 50% group opacity over a black canvas should read as \
             roughly half-strength red, got {pixel:?}"
        );
    }

    #[test]
    fn an_opacity_of_zero_paints_nothing_at_all() {
        let tree: Element = view! {
            <div class="card">
                <div class="ghost" />
            </div>
        };
        let css = "
            .card { width: 20px; height: 20px; background-color: #1e1e22; }
            .ghost { opacity: 0; background-color: #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        assert_eq!(
            pixel_rgb(&buffer, 10, 10),
            [0x1e, 0x1e, 0x22],
            "an opacity: 0 child must paint nothing at all, leaving its parent's own \
             background untouched"
        );
    }

    #[test]
    fn group_opacity_does_not_double_blend_children_that_overlap_inside_the_group() {
        // The whole reason this is "group" opacity and not a per-node
        // alpha multiply: back and front are both fully opaque relative
        // to *each other* (front, painted later, fully occludes back
        // wherever they overlap), and only the group's own combined
        // result fades against whatever is behind it. Painting each
        // child at the group's reduced opacity individually instead
        // would show the overlap as a visibly different red/green blend
        // (back's red bleeding through front) rather than pure green
        // faded once.
        let tree: Element = view! {
            <div class="group">
                <div class="back" />
                <div class="front" />
            </div>
        };
        let css = "
            .group { opacity: 0.5; }
            .back { background-color: #ff0000; }
            .front { background-color: #00ff00; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let group = arena.roots()[0];
        let back = arena.children(group)[0];
        let front = arena.children(group)[1];
        let mut layouts = HashMap::new();
        layouts.insert(
            group,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        );
        layouts.insert(
            back,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 30.0,
                height: 30.0,
            },
        );
        layouts.insert(
            front,
            BoxLayout {
                x: 10.0,
                y: 10.0,
                width: 30.0,
                height: 30.0,
            },
        );

        let mut font = Font::load_embedded();
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

        let overlap = pixel_rgb(&buffer, 20, 20);
        assert!(
            overlap[0] < 20,
            "the overlap must read as green faded by the group's own opacity (red channel \
             near zero), not a red/green blend from fading each child individually — got \
             {overlap:?}"
        );
        assert!(
            (100..160).contains(&overlap[1]),
            "green should be roughly half-strength after the group's 50% opacity, got \
             {overlap:?}"
        );

        let red_only = pixel_rgb(&buffer, 5, 20);
        assert!(
            (100..160).contains(&red_only[0]) && red_only[1] == 0,
            "outside the overlap, back's own red must still fade correctly, got {red_only:?}"
        );
    }

    #[test]
    fn overflow_hidden_clips_a_child_that_extends_past_the_parents_padding_box() {
        let tree: Element = view! {
            <div class="frame">
                <div class="content" />
            </div>
        };
        let css = "
            .frame { width: 20px; height: 20px; overflow: hidden; }
            .content { background-color: #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let frame = arena.roots()[0];
        let content = arena.children(frame)[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            frame,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        // Deliberately much larger than, and positioned to spill past,
        // the frame's own 20x20 box on every side.
        layouts.insert(
            content,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        );

        let mut font = Font::load_embedded();
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

        assert_eq!(
            pixel_rgb(&buffer, 10, 10),
            [0xff, 0, 0],
            "inside the frame's own bounds, the child must still paint normally"
        );
        assert_eq!(
            pixel_rgb(&buffer, 30, 30),
            [0, 0, 0],
            "past the frame's own padding box, the child's overflow must be clipped away, \
             leaving the plain canvas background"
        );
    }

    #[test]
    fn overflow_visible_the_default_does_not_clip_an_overflowing_child() {
        // The exact same shape as the `overflow: hidden` test above, minus
        // that one declaration — proves the clip in that test comes from
        // `overflow: hidden` specifically, not from some other limit (a
        // canvas edge, a coincidentally-unpainted region) that would make
        // that test pass for the wrong reason.
        let tree: Element = view! {
            <div class="frame">
                <div class="content" />
            </div>
        };
        let css = "
            .frame { width: 20px; height: 20px; }
            .content { background-color: #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let frame = arena.roots()[0];
        let content = arena.children(frame)[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            frame,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        layouts.insert(
            content,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        );

        let mut font = Font::load_embedded();
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

        assert_eq!(
            pixel_rgb(&buffer, 30, 30),
            [0xff, 0, 0],
            "overflow: visible (the default) must not clip the child at all"
        );
    }

    #[test]
    fn nested_overflow_hidden_clips_to_the_intersection_of_both_ancestors() {
        // The outer frame alone would let the child show through at
        // (25, 5) (inside the outer's 30x30 box, outside the inner's own
        // narrower 15x15 one) -- only the *intersection* of both clips
        // correctly hides it there too.
        let tree: Element = view! {
            <div class="outer">
                <div class="inner">
                    <div class="content" />
                </div>
            </div>
        };
        let css = "
            .outer { width: 30px; height: 30px; overflow: hidden; }
            .inner { width: 15px; height: 15px; overflow: hidden; }
            .content { background-color: #ff0000; }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );

        let outer = arena.roots()[0];
        let inner = arena.children(outer)[0];
        let content = arena.children(inner)[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            outer,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 30.0,
                height: 30.0,
            },
        );
        layouts.insert(
            inner,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 15.0,
                height: 15.0,
            },
        );
        layouts.insert(
            content,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        );

        let mut font = Font::load_embedded();
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

        assert_eq!(
            pixel_rgb(&buffer, 5, 5),
            [0xff, 0, 0],
            "inside both the inner and outer bounds, the content must still paint"
        );
        assert_eq!(
            pixel_rgb(&buffer, 25, 5),
            [0, 0, 0],
            "inside the outer's own 30x30 box but past the inner's own narrower 15x15 one, \
             the intersection of both clips must still hide it"
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 25),
            [0, 0, 0],
            "past the outer's own 30x30 box entirely, the intersection must hide it here too"
        );
    }

    #[test]
    fn overflow_hidden_does_not_clip_its_own_background_or_border() {
        // Real CSS: `overflow` clips a box's *content*, never the box's
        // own border/background against its own clip -- only a
        // descendant can be clipped by it.
        let tree: Element = view! { <div class="frame" /> };
        let css = "
            .frame {
                width: 20px;
                height: 20px;
                overflow: hidden;
                background-color: #ff0000;
                border-top-width: 4px;
                border-top-style: solid;
                border-top-color: #00ff00;
            }
        ";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
        let node = arena.roots()[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            node,
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
            pixel_rgb(&buffer, 2, 2),
            [0, 0xff, 0],
            "the frame's own border must still paint in full"
        );
        assert_eq!(
            pixel_rgb(&buffer, 10, 10),
            [0xff, 0, 0],
            "the frame's own background must still paint in full"
        );
    }

    #[test]
    fn text_paints_visible_ink_in_its_declared_color() {
        let tree: Element = view! { <h2>{"H"}</h2> };
        let css = "h2 { color: #ff0000; font-size: 40px; }";
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
            let styles = florui_style::compute(
                &arena,
                &rules,
                &InteractionState::new(),
                florui_style::Viewport::default(),
            );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
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
            let styles = florui_style::compute(
                &arena,
                &rules,
                &InteractionState::new(),
                florui_style::Viewport::default(),
            );
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

    fn single_box_buffer(css: &str, canvas: u32, scale_factor: f32) -> Canvas {
        single_box_buffer_at(css, canvas, scale_factor, 0.0, 0.0)
    }

    fn single_box_buffer_at(css: &str, canvas: u32, scale_factor: f32, x: f32, y: f32) -> Canvas {
        let tree: Element = view! { <div class="box" /> };
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
        let node = arena.roots()[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            node,
            BoxLayout {
                x,
                y,
                width: 10.0 * scale_factor,
                height: 10.0 * scale_factor,
            },
        );
        let mut font = Font::load_embedded();
        paint_to_buffer(
            &mut font,
            canvas,
            canvas,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            scale_factor,
        )
    }

    #[test]
    fn translate_moves_a_solid_box_to_its_own_new_position() {
        let buffer = single_box_buffer(
            ".box { background-color: #ff0000; transform: translate(15px, 5px); }",
            30,
            1.0,
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 5),
            [0, 0, 0],
            "the box's own untransformed spot must go back to the canvas's clear color"
        );
        assert_eq!(
            pixel_rgb(&buffer, 20, 10),
            [0xff, 0, 0],
            "the box must actually paint at translate()'s own new position"
        );
    }

    #[test]
    fn translate_percentage_resolves_against_the_nodes_own_box() {
        // A 10px-wide/tall box: translate(50%, 0) must move it by 5px on
        // the x axis alone, the percentage resolving against *this
        // node's own* box (real CSS's own reference box for `translate`),
        // not some other size.
        let buffer = single_box_buffer(
            ".box { background-color: #ff0000; transform: translate(50%, 0); }",
            20,
            1.0,
        );
        assert_eq!(pixel_rgb(&buffer, 2, 5), [0, 0, 0], "left of the moved box");
        assert_eq!(
            pixel_rgb(&buffer, 7, 5),
            [0xff, 0, 0],
            "inside the moved box"
        );
    }

    #[test]
    fn translates_own_length_scales_with_a_hidpi_canvas_but_its_percentage_does_not_need_to() {
        // Same logical move (`translate(10px, 0)` on a box already
        // scaled to physical pixels by the caller) at two different
        // scale factors: the *physical* distance the box moves must
        // scale right along with the canvas, the same correction real
        // text painting already needs for its own glyph offsets.
        let buffer_1x = single_box_buffer(
            ".box { background-color: #ff0000; transform: translate(10px, 0); }",
            30,
            1.0,
        );
        let buffer_2x = single_box_buffer(
            ".box { background-color: #ff0000; transform: translate(10px, 0); }",
            60,
            2.0,
        );
        assert_eq!(
            pixel_rgb(&buffer_1x, 15, 5),
            [0xff, 0, 0],
            "moved 10 logical px at 1x"
        );
        assert_eq!(
            pixel_rgb(&buffer_2x, 30, 10),
            [0xff, 0, 0],
            "the same 10 logical px must land 20 physical px over on a 2x canvas"
        );
    }

    #[test]
    fn rotate_pivots_around_the_boxs_own_default_center_origin() {
        // A box with a red top border only: rotating 180 degrees around
        // the default center origin must flip that border to the
        // opposite (bottom) edge — proof both that rotation actually
        // rotates and that the default `transform-origin` (the box's own
        // center, not its top-left corner) is right, since pivoting
        // around the corner would move the whole box off-canvas instead
        // of flipping it in place.
        let buffer = single_box_buffer(
            ".box {
                background-color: #1e1e22;
                border-top-width: 3px; border-top-style: solid; border-top-color: #ff0000;
                transform: rotate(180deg);
            }",
            10,
            1.0,
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 1),
            [0x1e, 0x1e, 0x22],
            "the top edge no longer shows the border — it flipped away from here"
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 8),
            [0xff, 0, 0],
            "bottom edge now shows the red border"
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 5),
            [0x1e, 0x1e, 0x22],
            "the middle stays the box's own background"
        );
    }

    #[test]
    fn scale_grows_the_box_around_its_own_default_center_origin() {
        // A 10x10 box at (10, 10) — centered at (15, 15) — scaled 2x
        // around its own default center must span 20x20 still centered
        // on (15, 15), i.e. (5, 5)-(25, 25): 5px past each of its
        // original edges on *every* side, not 10px past only the
        // bottom-right ones (which a top-left-pivoted scale would
        // produce instead).
        let buffer = single_box_buffer_at(
            ".box { background-color: #ff0000; transform: scale(2); }",
            30,
            1.0,
            10.0,
            10.0,
        );
        assert_eq!(
            pixel_rgb(&buffer, 3, 15),
            [0, 0, 0],
            "still outside the scaled box's new left edge"
        );
        assert_eq!(
            pixel_rgb(&buffer, 7, 15),
            [0xff, 0, 0],
            "just inside the scaled box's new left edge"
        );
        assert_eq!(
            pixel_rgb(&buffer, 23, 15),
            [0xff, 0, 0],
            "just inside the scaled box's new right edge"
        );
        assert_eq!(
            pixel_rgb(&buffer, 27, 15),
            [0, 0, 0],
            "still outside the scaled box's new right edge"
        );
    }

    #[test]
    fn matrix_produces_the_same_result_as_the_equivalent_translate() {
        let via_translate = single_box_buffer(
            ".box { background-color: #ff0000; transform: translate(15px, 5px); }",
            30,
            1.0,
        );
        let via_matrix = single_box_buffer(
            ".box { background-color: #ff0000; transform: matrix(1, 0, 0, 1, 15, 5); }",
            30,
            1.0,
        );
        for y in 0..30 {
            for x in 0..30 {
                assert_eq!(
                    pixel_rgb(&via_matrix, x, y),
                    pixel_rgb(&via_translate, x, y),
                    "matrix(1,0,0,1,15,5) must paint identically to translate(15px, 5px) at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn transform_functions_compose_left_to_right_not_commutatively() {
        // Real CSS composes a transform list so the *last*-listed
        // function acts on the original point first and the
        // *first*-listed one acts last, in the coordinate system every
        // earlier function already established — so `translate(10px, 0)
        // rotate(90deg)` (rotate first, in the box's own local frame,
        // then translate in the outer/world frame) must not paint the
        // same as `rotate(90deg) translate(10px, 0)` (translate first,
        // then that already-shifted result gets rotated around the
        // box's own original center). Any reversed composition order
        // would make these two indistinguishable or swapped.
        let translate_then_rotate = single_box_buffer(
            ".box { background-color: #ff0000; transform: translate(10px, 0) rotate(90deg); }",
            30,
            1.0,
        );
        let rotate_then_translate = single_box_buffer(
            ".box { background-color: #ff0000; transform: rotate(90deg) translate(10px, 0); }",
            30,
            1.0,
        );
        let mut any_pixel_differs = false;
        for y in 0..30 {
            for x in 0..30 {
                if pixel_rgb(&translate_then_rotate, x, y)
                    != pixel_rgb(&rotate_then_translate, x, y)
                {
                    any_pixel_differs = true;
                }
            }
        }
        assert!(
            any_pixel_differs,
            "translate(10px,0) rotate(90deg) and rotate(90deg) translate(10px,0) must not paint \
             identically — composition order matters in real CSS"
        );
    }

    #[test]
    fn opacity_and_transform_apply_together_in_the_same_group() {
        let buffer = single_box_buffer(
            ".box { background-color: #ff0000; opacity: 0.5; transform: translate(15px, 0); }",
            30,
            1.0,
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 5),
            [0, 0, 0],
            "the box's own untransformed spot must be untouched — opacity alone must not leave \
             it painted in place"
        );
        assert_eq!(
            pixel_rgb(&buffer, 20, 5),
            [0x80, 0, 0],
            "the moved box must still be faded to half opacity against the canvas's own black"
        );
    }

    #[test]
    fn an_unsupported_skew_function_is_treated_as_identity() {
        // `skew()` isn't in this crate's documented 2D subset (see
        // `TransformFunction`'s own doc) and drops out of the resolved
        // list entirely — so a box with only a skew must paint exactly
        // where it would with no `transform` at all, not distorted and
        // not silently discarded along with the rest of its own styles.
        let with_skew = single_box_buffer(
            ".box { background-color: #ff0000; transform: skewX(30deg); }",
            30,
            1.0,
        );
        let without_transform = single_box_buffer(".box { background-color: #ff0000; }", 30, 1.0);
        for y in 0..30 {
            for x in 0..30 {
                assert_eq!(
                    pixel_rgb(&with_skew, x, y),
                    pixel_rgb(&without_transform, x, y)
                );
            }
        }
    }

    #[test]
    fn blur_spreads_a_solid_boxs_own_color_past_its_own_edge() {
        let filtered = single_box_buffer_at(
            ".box { background-color: #ff0000; filter: blur(6px); }",
            30,
            1.0,
            10.0,
            10.0,
        );
        let unfiltered =
            single_box_buffer_at(".box { background-color: #ff0000; }", 30, 1.0, 10.0, 10.0);
        assert_eq!(
            pixel_rgb(&unfiltered, 8, 15),
            [0, 0, 0],
            "sanity check: 2px outside the unblurred box is pure background"
        );
        let blurred_pixel = pixel_rgb(&filtered, 8, 15);
        assert_ne!(
            blurred_pixel,
            [0, 0, 0],
            "a blurred box's own color must bleed past its own original edge"
        );
        assert_ne!(
            blurred_pixel,
            [0xff, 0, 0],
            "but not at full strength right at the blur's own reach"
        );
    }

    #[test]
    fn brightness_of_zero_paints_the_box_fully_black_without_touching_its_own_alpha() {
        let buffer = single_box_buffer(
            ".box { background-color: #ff8040; filter: brightness(0); }",
            20,
            1.0,
        );
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0, 0, 0]);
    }

    #[test]
    fn brightness_above_one_brightens_and_clamps_at_full_strength() {
        let buffer = single_box_buffer(
            ".box { background-color: #804020; filter: brightness(4); }",
            20,
            1.0,
        );
        // 0x80*4 and 0x40*4 both saturate to 0xff; 0x20*4 = 0x80 exactly.
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0xff, 0xff, 0x80]);
    }

    #[test]
    fn contrast_pushes_a_light_color_further_from_mid_gray() {
        let buffer = single_box_buffer(
            ".box { background-color: #c0c0c0; filter: contrast(2); }",
            20,
            1.0,
        );
        // (0xc0 - 127.5) * 2 + 127.5 = 256.5, clamped to 255.
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0xff, 0xff, 0xff]);
    }

    #[test]
    fn contrast_of_zero_flattens_every_color_to_mid_gray() {
        let buffer = single_box_buffer(
            ".box { background-color: #ff0000; filter: contrast(0); }",
            20,
            1.0,
        );
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0x80, 0x80, 0x80]);
    }

    #[test]
    fn saturate_of_zero_desaturates_to_the_colors_own_luma() {
        let buffer = single_box_buffer(
            ".box { background-color: #ff0000; filter: saturate(0); }",
            20,
            1.0,
        );
        // Luma of pure red at this crate's own saturate coefficients:
        // 0.213 * 255 ~= 54.
        let [r, g, b] = pixel_rgb(&buffer, 5, 5);
        assert_eq!(r, g, "a fully desaturated pixel has equal channels");
        assert_eq!(g, b, "a fully desaturated pixel has equal channels");
        assert!(
            (50..=58).contains(&r),
            "expected red's own luma (~54), got {r}"
        );
    }

    #[test]
    fn filter_functions_apply_in_authored_order_not_commutatively() {
        // brightness(2) then contrast(0) must land on mid-gray (contrast
        // ignores whatever brightness already did); contrast(0) then
        // brightness(2) must double that same mid-gray instead. If this
        // crate applied the chain in the wrong order, both would produce
        // the identical result.
        let brighten_then_flatten = single_box_buffer(
            ".box { background-color: #300000; filter: brightness(2) contrast(0); }",
            20,
            1.0,
        );
        let flatten_then_brighten = single_box_buffer(
            ".box { background-color: #300000; filter: contrast(0) brightness(2); }",
            20,
            1.0,
        );
        assert_eq!(pixel_rgb(&brighten_then_flatten, 5, 5), [0x80, 0x80, 0x80]);
        assert_eq!(pixel_rgb(&flatten_then_brighten, 5, 5), [0xff, 0xff, 0xff]);
    }

    #[test]
    fn an_unsupported_grayscale_function_is_dropped_from_the_filter_chain() {
        let with_grayscale = single_box_buffer(
            ".box { background-color: #ff0000; filter: grayscale(1); }",
            20,
            1.0,
        );
        let without_filter = single_box_buffer(".box { background-color: #ff0000; }", 20, 1.0);
        assert_eq!(
            pixel_rgb(&with_grayscale, 5, 5),
            pixel_rgb(&without_filter, 5, 5)
        );
    }

    #[test]
    fn filter_opacity_and_transform_all_apply_together() {
        // filter must act on the box's own unfaded, untransformed
        // content: contrast(0) flattens it to mid-gray, *then* opacity
        // fades that gray, *then* translate moves the whole faded result.
        let buffer = single_box_buffer(
            ".box {
                background-color: #ff0000;
                filter: contrast(0);
                opacity: 0.5;
                transform: translate(15px, 0);
            }",
            30,
            1.0,
        );
        assert_eq!(
            pixel_rgb(&buffer, 5, 5),
            [0, 0, 0],
            "the box's own untransformed spot must be untouched"
        );
        assert_eq!(
            pixel_rgb(&buffer, 20, 5),
            [0x40, 0x40, 0x40],
            "mid-gray (0x80) faded to half opacity against black"
        );
    }

    fn backdrop_over_red_buffer(backdrop_css: &str) -> Canvas {
        let tree: Element = view! {
            <div class="back">
                <div class="glass"></div>
            </div>
        };
        let css = format!(".back {{ background-color: #ff0000; }} .glass {{ {backdrop_css} }}");
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(&css).unwrap();
        let styles = florui_style::compute(
            &arena,
            &rules,
            &InteractionState::new(),
            florui_style::Viewport::default(),
        );
        let back = arena.roots()[0];
        let glass = arena.children(back)[0];
        let mut layouts = HashMap::new();
        layouts.insert(
            back,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 40.0,
            },
        );
        layouts.insert(
            glass,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        let mut font = Font::load_embedded();
        paint_to_buffer(
            &mut font,
            40,
            40,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
            1.0,
        )
    }

    #[test]
    fn backdrop_filter_darkens_what_is_already_painted_behind_it() {
        let buffer = backdrop_over_red_buffer("backdrop-filter: brightness(0.5);");
        assert_eq!(
            pixel_rgb(&buffer, 5, 5),
            [0x80, 0, 0],
            "inside the glass, the red behind it must be dimmed"
        );
        assert_eq!(
            pixel_rgb(&buffer, 30, 30),
            [0xff, 0, 0],
            "outside the glass, the background is untouched"
        );
    }

    #[test]
    fn an_empty_backdrop_filter_paints_nothing_extra() {
        let buffer = backdrop_over_red_buffer("");
        assert_eq!(pixel_rgb(&buffer, 5, 5), [0xff, 0, 0]);
    }
}

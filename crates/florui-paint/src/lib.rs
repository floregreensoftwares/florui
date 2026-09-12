//! Paints a styled, laid-out `florui_style::Arena` into pixels — the
//! "painting" stage at the end of the pipeline (components → elements →
//! style → boxes/layout → text → **painting**).
//!
//! # Scope
//!
//! Flat background-color rectangles only, painted in real document order:
//! a node before its children, children in source order, so an overlap
//! always resolves to whichever box comes later in the tree — the same
//! rule real CSS painting follows for normal-flow boxes with no stacking
//! contexts. There is no text rendering yet (`florui-text` only measures,
//! it does not rasterize glyphs), and no borders, shadows, opacity,
//! transforms, or clipping — `florui-style` has no properties for any of
//! those yet either.

use std::collections::HashMap;
use std::path::Path;

use florui_layout::{BoxLayout, absolute_position};
use florui_style::{Arena, ComputedStyle, NodeId, Rgba};
use image::{ImageBuffer, Rgba as ImageRgba};

pub type Canvas = ImageBuffer<ImageRgba<u8>, Vec<u8>>;

#[derive(Debug)]
pub struct PaintError {
    path: std::path::PathBuf,
    source: image::ImageError,
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
/// every node's background rectangle at its real computed position and
/// size, in document order — and writes the result as a PNG.
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
        .save(path)
        .map_err(|source| PaintError {
            path: path.to_owned(),
            source,
        })
}

/// Same painting as [`paint_to_png`], returning the pixel buffer directly
/// instead of writing it to disk.
pub fn paint_to_buffer(
    width: u32,
    height: u32,
    canvas: Rgba,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
) -> Canvas {
    let mut buffer: Canvas = ImageBuffer::from_pixel(
        width,
        height,
        ImageRgba([canvas.r, canvas.g, canvas.b, canvas.a]),
    );
    for &root in arena.roots() {
        paint_node(&mut buffer, arena, styles, layouts, root);
    }
    buffer
}

/// Paints `node`'s own background, then its children in source order, so
/// an overlapping later node always wins over an earlier one — real
/// document-order painting rather than a heuristic (e.g. sorting by box
/// area) that only happens to agree with it for pure containment.
fn paint_node(
    buffer: &mut Canvas,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    node: NodeId,
) {
    if let Some(&layout) = layouts.get(&node) {
        let color = styles
            .get(&node)
            .map_or(Rgba::TRANSPARENT, |s| s.background_color);
        if color.a != 0 {
            let (x, y) = absolute_position(arena, layouts, node);
            fill_rect(buffer, x, y, layout.width, layout.height, color);
        }
    }
    for &child in arena.children(node) {
        paint_node(buffer, arena, styles, layouts, child);
    }
}

fn fill_rect(buffer: &mut Canvas, x: f32, y: f32, width: f32, height: f32, color: Rgba) {
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + width).max(0.0) as u32).min(buffer.width());
    let y1 = ((y + height).max(0.0) as u32).min(buffer.height());
    for py in y0..y1 {
        for px in x0..x1 {
            buffer.put_pixel(px, py, ImageRgba([color.r, color.g, color.b, color.a]));
        }
    }
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;
    use florui_style::InteractionState;
    use taffy::prelude::*;

    use super::*;

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
        let card_pixel = buffer.get_pixel(5, 5);
        assert_eq!(
            [card_pixel[0], card_pixel[1], card_pixel[2]],
            [0x1e, 0x1e, 0x22]
        );
        // Inside the button (offset by the card's padding).
        let button_pixel = buffer.get_pixel(15, 15);
        assert_eq!(
            [button_pixel[0], button_pixel[1], button_pixel[2]],
            [0x42, 0x73, 0x4f]
        );
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

        let decoded = image::open(&path).unwrap().to_rgba8();
        let pixel = decoded.get_pixel(10, 10);
        assert_eq!([pixel[0], pixel[1], pixel[2]], [0xff, 0, 0]);

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
        let pixel = buffer.get_pixel(10, 10);
        assert_eq!(
            [pixel[0], pixel[1], pixel[2]],
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
        let pixel = buffer.get_pixel(10, 10);
        assert_eq!([pixel[0], pixel[1], pixel[2]], [0x1e, 0x1e, 0x22]);
    }
}

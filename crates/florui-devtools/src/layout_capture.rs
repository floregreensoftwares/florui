//! Paints every node's computed background at its real, layout-computed
//! position and size — a genuine capture of a styled, laid-out tree,
//! rather than [`crate::element_scene`]'s fixed-margin canvas-plus-one-box
//! stand-in for a tree the engine could not yet size or position.
//!
//! There is still no text rendering and no borders/shadows/opacity/
//! transforms/clipping — this paints flat background rectangles only, in
//! the real position and size the style+layout pipeline computed.

use std::collections::HashMap;
use std::path::Path;

use florui_layout::{BoxLayout, absolute_position};
use florui_style::{Arena, ComputedStyle, NodeId, Rgba};
use image::{ImageBuffer, Rgba as ImageRgba};

#[derive(Debug)]
pub struct LayoutCaptureError {
    path: std::path::PathBuf,
    source: image::ImageError,
}

impl std::fmt::Display for LayoutCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not write capture {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for LayoutCaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Renders `width` x `height` physical pixels — `canvas` everywhere, then
/// every node's background rectangle at its real computed position and
/// size — and writes the result as a PNG.
pub fn paint_to_png(
    path: &Path,
    width: u32,
    height: u32,
    canvas: Rgba,
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
) -> Result<(), LayoutCaptureError> {
    let mut buffer: ImageBuffer<ImageRgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(
        width,
        height,
        ImageRgba([canvas.r, canvas.g, canvas.b, canvas.a]),
    );

    // HashMap iteration order is unspecified, so bigger boxes are painted
    // first and smaller ones on top — a correct painter's-algorithm order
    // for a tree with no overlapping siblings, though real stacking-context
    // paint order is a later concern this does not attempt.
    let mut nodes: Vec<NodeId> = layouts.keys().copied().collect();
    nodes.sort_by(|a, b| area_of(layouts, b).total_cmp(&area_of(layouts, a)));

    for node in nodes {
        let layout = layouts[&node];
        let color = styles
            .get(&node)
            .map_or(Rgba::TRANSPARENT, |s| s.background_color);
        if color.a == 0 {
            continue;
        }
        let (x, y) = absolute_position(arena, layouts, node);
        fill_rect(&mut buffer, x, y, layout.width, layout.height, color);
    }

    buffer.save(path).map_err(|source| LayoutCaptureError {
        path: path.to_owned(),
        source,
    })
}

fn area_of(layouts: &HashMap<NodeId, BoxLayout>, node: &NodeId) -> f32 {
    let layout = layouts[node];
    layout.width * layout.height
}

fn fill_rect(
    buffer: &mut ImageBuffer<ImageRgba<u8>, Vec<u8>>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Rgba,
) {
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

        let dir =
            std::env::temp_dir().join(format!("florui-layout-capture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.png");

        paint_to_png(
            &path,
            100,
            60,
            Rgba::opaque(0, 0, 0),
            &arena,
            &styles,
            &layouts,
        )
        .unwrap();

        let decoded = image::open(&path).unwrap().to_rgba8();
        // Inside the card, outside the button: the card's own color.
        let card_pixel = decoded.get_pixel(5, 5);
        assert_eq!(
            [card_pixel[0], card_pixel[1], card_pixel[2]],
            [0x1e, 0x1e, 0x22]
        );
        // Inside the button (offset by the card's padding).
        let button_pixel = decoded.get_pixel(15, 15);
        assert_eq!(
            [button_pixel[0], button_pixel[1], button_pixel[2]],
            [0x42, 0x73, 0x4f]
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}

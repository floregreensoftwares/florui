//! Renders the preview scene: a canvas with one inset rectangle standing in
//! for a single drawable element. Pure pixel computation, shared by the live
//! preview window, the offscreen capture path, and the reference-comparison
//! harness so all three draw the same thing.

use crate::color::Rgba;

/// Margin, in physical pixels, between the canvas edge and the element box
/// in the interactive preview (equal on every side).
pub const ELEMENT_MARGIN: u32 = 24;

/// The interactive preview's inset: equal on every side. A reference fixture
/// that needs to detect an orientation flip (top/bottom or left/right
/// swapped) must use an asymmetric [`Insets`] instead — a uniform inset
/// looks identical whether or not such a swap happened.
pub const ELEMENT_INSET: Insets = Insets::uniform(ELEMENT_MARGIN);

/// Distance from each canvas edge to the element box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insets {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl Insets {
    pub const fn uniform(value: u32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// The element's box in physical pixels, or absence of one when the canvas
/// is too small to fit the requested insets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ElementBox {
    /// Whether the physical pixel at `(px, py)` falls inside this box.
    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }
}

/// Computes the element box for a `width` x `height` canvas under `inset`.
/// Returns `None` when the insets leave no room for a box (matching the
/// fill functions below, which fall back to canvas-only in that case).
pub fn element_box(width: u32, height: u32, inset: Insets) -> Option<ElementBox> {
    let box_width = width
        .checked_sub(inset.left)
        .and_then(|w| w.checked_sub(inset.right))
        .filter(|&w| w > 0)?;
    let box_height = height
        .checked_sub(inset.top)
        .and_then(|h| h.checked_sub(inset.bottom))
        .filter(|&h| h > 0)?;
    Some(ElementBox {
        x: inset.left,
        y: inset.top,
        width: box_width,
        height: box_height,
    })
}

/// Fills `width` x `height` RGBA8 pixels: `canvas` everywhere, `element`
/// inside the box described by `inset`.
pub fn render_rgba8_inset(
    width: u32,
    height: u32,
    canvas: Rgba,
    element: Rgba,
    inset: Insets,
) -> Vec<u8> {
    let bounds = element_box(width, height, inset);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let color = match bounds {
                Some(b) if b.contains(x, y) => element,
                _ => canvas,
            };
            pixels.extend_from_slice(&[color.r, color.g, color.b, color.a]);
        }
    }
    pixels
}

/// Packs the same scene into 0RGB `u32` pixels for a `softbuffer` surface.
pub fn render_0rgb_inset(
    width: u32,
    height: u32,
    canvas: Rgba,
    element: Rgba,
    inset: Insets,
) -> Vec<u32> {
    let bounds = element_box(width, height, inset);
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        for x in 0..width {
            let color = match bounds {
                Some(b) if b.contains(x, y) => element,
                _ => canvas,
            };
            pixels.push(u32::from_be_bytes([0, color.r, color.g, color.b]));
        }
    }
    pixels
}

/// Fills `width` x `height` RGBA8 pixels using the interactive preview's
/// equal-on-every-side [`ELEMENT_INSET`].
pub fn render_rgba8(width: u32, height: u32, canvas: Rgba, element: Rgba) -> Vec<u8> {
    render_rgba8_inset(width, height, canvas, element, ELEMENT_INSET)
}

/// Packs the same scene into 0RGB `u32` pixels using [`ELEMENT_INSET`].
pub fn render_0rgb(width: u32, height: u32, canvas: Rgba, element: Rgba) -> Vec<u32> {
    render_0rgb_inset(width, height, canvas, element, ELEMENT_INSET)
}

/// Draws a `thickness`-pixel outline around `rect` directly into an already
/// rendered 0RGB buffer, for the preview window's selection highlight. Kept
/// separate from the fill functions so the reference-comparison path (which
/// never has a selection) stays unaffected by this concern.
pub fn outline_rect(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    rect: ElementBox,
    color: Rgba,
    thickness: u32,
) {
    let packed = u32::from_be_bytes([0, color.r, color.g, color.b]);
    let mut set = |x: u32, y: u32| {
        if x < width && y < height {
            pixels[(y * width + x) as usize] = packed;
        }
    };

    for t in 0..thickness {
        for x in rect.x.saturating_sub(t)..=(rect.x + rect.width + t).min(width.saturating_sub(1)) {
            set(x, rect.y.saturating_sub(t));
            set(x, rect.y + rect.height + t);
        }
        for y in rect.y.saturating_sub(t)..=(rect.y + rect.height + t).min(height.saturating_sub(1))
        {
            set(rect.x.saturating_sub(t), y);
            set(rect.x + rect.width + t, y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_pixel_is_element_color() {
        let canvas = Rgba::opaque(10, 10, 10);
        let element = Rgba::opaque(200, 50, 50);
        let pixels = render_rgba8(200, 200, canvas, element);
        let idx = (100 * 200 + 100) * 4;
        assert_eq!(&pixels[idx..idx + 4], &[200, 50, 50, 255]);
    }

    #[test]
    fn corner_pixel_is_canvas_color() {
        let canvas = Rgba::opaque(10, 10, 10);
        let element = Rgba::opaque(200, 50, 50);
        let pixels = render_rgba8(200, 200, canvas, element);
        assert_eq!(&pixels[0..4], &[10, 10, 10, 255]);
    }

    #[test]
    fn too_small_canvas_has_no_element() {
        let canvas = Rgba::opaque(1, 2, 3);
        let pixels = render_rgba8(10, 10, canvas, Rgba::opaque(9, 9, 9));
        assert!(pixels.chunks(4).all(|p| p == [1, 2, 3, 255]));
    }

    #[test]
    fn asymmetric_insets_place_each_edge_independently() {
        let inset = Insets {
            top: 8,
            right: 24,
            bottom: 40,
            left: 56,
        };
        let bounds = element_box(320, 240, inset).expect("box should fit");
        assert_eq!(bounds.x, 56);
        assert_eq!(bounds.y, 8);
        assert_eq!(bounds.width, 320 - 56 - 24);
        assert_eq!(bounds.height, 240 - 8 - 40);

        // A top/bottom or left/right swap would move these boundary pixels,
        // so each assertion below is only satisfied by the exact, unswapped
        // insets above.
        let canvas = Rgba::opaque(1, 1, 1);
        let element = Rgba::opaque(2, 2, 2);
        let pixels = render_rgba8_inset(320, 240, canvas, element, inset);
        let at = |x: u32, y: u32| {
            let idx = ((y * 320 + x) * 4) as usize;
            &pixels[idx..idx + 4]
        };
        assert_eq!(at(56, 8), [2, 2, 2, 255], "top-left corner of the box");
        assert_eq!(at(55, 8), [1, 1, 1, 255], "just left of the box");
        assert_eq!(at(56, 7), [1, 1, 1, 255], "just above the box");
        assert_eq!(
            at(320 - 24 - 1, 240 - 40 - 1),
            [2, 2, 2, 255],
            "bottom-right corner of the box"
        );
    }

    #[test]
    fn element_box_is_none_when_insets_leave_no_room() {
        assert!(element_box(10, 10, Insets::uniform(5)).is_none());
        assert!(element_box(10, 10, Insets::uniform(6)).is_none());
    }

    #[test]
    fn outline_rect_draws_a_border_without_filling_the_interior() {
        let width = 20;
        let height = 20;
        let background = Rgba::opaque(0, 0, 0);
        let mut pixels = render_0rgb(width, height, background, background);
        let rect = ElementBox {
            x: 5,
            y: 5,
            width: 6,
            height: 6,
        };
        let highlight = Rgba::opaque(255, 0, 0);
        outline_rect(&mut pixels, width, height, rect, highlight, 1);

        let packed = |c: Rgba| u32::from_be_bytes([0, c.r, c.g, c.b]);
        assert_eq!(pixels[(5 * width + 5) as usize], packed(highlight));
        assert_eq!(pixels[(5 * width + 11) as usize], packed(highlight));
        assert_eq!(
            pixels[(8 * width + 8) as usize],
            packed(background),
            "interior stays untouched"
        );
    }
}

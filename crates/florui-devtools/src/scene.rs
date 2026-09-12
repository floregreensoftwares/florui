//! Renders the preview scene: a canvas with one inset rectangle standing in
//! for a single drawable element. Pure pixel computation, shared by the live
//! preview window and the offscreen capture path so both draw the same thing.

use crate::color::Rgba;

/// Margin, in physical pixels, between the canvas edge and the element box.
pub const ELEMENT_MARGIN: u32 = 24;

/// Fills `width` x `height` RGBA8 pixels: `canvas` everywhere, `element`
/// inside a rectangle inset by [`ELEMENT_MARGIN`] on every side. Returns the
/// canvas color unchanged when the canvas is too small to fit any margin.
pub fn render_rgba8(width: u32, height: u32, canvas: Rgba, element: Rgba) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);

    let has_element = width > ELEMENT_MARGIN * 2 && height > ELEMENT_MARGIN * 2;
    let x_range = ELEMENT_MARGIN..width.saturating_sub(ELEMENT_MARGIN);
    let y_range = ELEMENT_MARGIN..height.saturating_sub(ELEMENT_MARGIN);

    for y in 0..height {
        for x in 0..width {
            let color = if has_element && x_range.contains(&x) && y_range.contains(&y) {
                element
            } else {
                canvas
            };
            pixels.extend_from_slice(&[color.r, color.g, color.b, color.a]);
        }
    }

    pixels
}

/// Packs the same scene into 0RGB `u32` pixels for a `softbuffer` surface.
pub fn render_0rgb(width: u32, height: u32, canvas: Rgba, element: Rgba) -> Vec<u32> {
    let has_element = width > ELEMENT_MARGIN * 2 && height > ELEMENT_MARGIN * 2;
    let x_range = ELEMENT_MARGIN..width.saturating_sub(ELEMENT_MARGIN);
    let y_range = ELEMENT_MARGIN..height.saturating_sub(ELEMENT_MARGIN);

    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        for x in 0..width {
            let color = if has_element && x_range.contains(&x) && y_range.contains(&y) {
                element
            } else {
                canvas
            };
            pixels.push(u32::from_be_bytes([0, color.r, color.g, color.b]));
        }
    }
    pixels
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
}

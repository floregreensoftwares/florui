//! Offscreen capture: renders the current scene to a PNG without opening a
//! window. This is the foundation for automated reference comparison against
//! a browser capture.

use std::path::Path;

use image::{ImageBuffer, Rgba as ImageRgba};

use crate::color::Rgba;
use crate::scene::render_rgba8;

#[derive(Debug)]
pub struct CaptureError {
    path: std::path::PathBuf,
    source: image::ImageError,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not write capture {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Renders `width` x `height` physical pixels and writes them as a PNG.
///
/// The caller records viewport size and DPR alongside the file, per the
/// conformance fixture contract; this function only produces the pixels.
pub fn capture_to_png(
    path: &Path,
    width: u32,
    height: u32,
    canvas: Rgba,
    element: Rgba,
) -> Result<(), CaptureError> {
    let pixels = render_rgba8(width, height, canvas, element);
    let buffer: ImageBuffer<ImageRgba<u8>, _> = ImageBuffer::from_raw(width, height, pixels)
        .expect("render_rgba8 always returns width * height * 4 bytes");

    buffer.save(path).map_err(|source| CaptureError {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_readable_png() {
        let dir = std::env::temp_dir().join(format!("florui-capture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.png");

        capture_to_png(&path, 64, 64, Rgba::opaque(1, 2, 3), Rgba::opaque(9, 8, 7)).unwrap();

        let decoded = image::open(&path).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (64, 64));

        std::fs::remove_dir_all(&dir).ok();
    }
}

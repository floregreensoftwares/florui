use crate::RawIcon;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum IconError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    UnsupportedFormat {
        path: PathBuf,
        extension: Option<String>,
    },
    Svg(resvg::usvg::Error),
    /// A syntactically valid SVG with no content (`viewBox`/size resolves
    /// to zero on at least one axis) — nothing to scale a raster size
    /// against.
    SvgEmptySize {
        path: Option<PathBuf>,
    },
    Png(image::ImageError),
}

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IconError::Io { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            IconError::UnsupportedFormat { path, extension } => write!(
                f,
                "{} has an unsupported icon extension ({}) -- only .svg and .png are supported",
                path.display(),
                extension.as_deref().unwrap_or("none")
            ),
            IconError::Svg(err) => write!(f, "could not parse SVG: {err}"),
            IconError::SvgEmptySize { path: Some(path) } => {
                write!(f, "{} has no visible content to rasterize", path.display())
            }
            IconError::SvgEmptySize { path: None } => {
                write!(f, "SVG has no visible content to rasterize")
            }
            IconError::Png(err) => write!(f, "could not decode PNG: {err}"),
        }
    }
}

impl std::error::Error for IconError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IconError::Io { source, .. } => Some(source),
            IconError::Svg(err) => Some(err),
            IconError::Png(err) => Some(err),
            IconError::UnsupportedFormat { .. } | IconError::SvgEmptySize { .. } => None,
        }
    }
}

/// Rasterizes `svg_text` to exactly `size` x `size` RGBA pixels, scaled
/// uniformly to fit the SVG's own intrinsic size (aspect-preserving,
/// centered within the square). Pure -- takes no path, touches no disk.
///
/// `tiny-skia` (via `resvg`) produces premultiplied alpha; this
/// unpremultiplies every pixel before returning, since
/// `winit::window::Icon::from_rgba` (and platform icon APIs generally)
/// expect straight alpha.
pub fn decode_svg(svg_text: &str, size: u32) -> Result<RawIcon, IconError> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(svg_text, &options).map_err(IconError::Svg)?;
    let intrinsic = tree.size();
    let longest = intrinsic.width().max(intrinsic.height());
    if longest <= 0.0 {
        return Err(IconError::SvgEmptySize { path: None });
    }
    let scale = size as f32 / longest;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(size, size).expect("size is checked nonzero by the caller");
    let offset_x = (size as f32 - intrinsic.width() * scale) / 2.0;
    let offset_y = (size as f32 - intrinsic.height() * scale) / 2.0;
    let transform =
        resvg::tiny_skia::Transform::from_translate(offset_x, offset_y).pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut rgba = pixmap.data().to_vec();
    unpremultiply(&mut rgba);
    Ok(RawIcon {
        rgba,
        width: size,
        height: size,
    })
}

/// Straight = premultiplied * 255 / alpha, per channel. `tiny-skia`'s own
/// premultiplied invariant guarantees `premultiplied <= alpha`, so this
/// never exceeds 255 and never needs clamping -- but the multiply itself
/// overflows `u8` (max 65025), so it runs in `u16`. At `alpha == 0` the
/// pixel is fully transparent and whatever RGB `tiny-skia` already wrote
/// there (typically 0,0,0) is invisible regardless -- skip the division
/// rather than divide by zero.
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = pixel[3] as u16;
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u16) * 255 / alpha) as u8;
        }
    }
}

/// Decodes `bytes` as a PNG at its own native resolution -- `size` has no
/// PNG analogue (no vector content to rasterize at an arbitrary size).
/// Pure -- takes no path, touches no disk.
pub fn decode_png(bytes: &[u8]) -> Result<RawIcon, IconError> {
    let image = image::load_from_memory(bytes)
        .map_err(IconError::Png)?
        .to_rgba8();
    let (width, height) = image.dimensions();
    Ok(RawIcon {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Reads and decodes `path`, dispatching on its extension (`.svg`/`.png`,
/// case-insensitive). `size` only affects SVG rasterization.
pub fn load_icon_at_size(path: &Path, size: u32) -> Result<RawIcon, IconError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("svg") => {
            let text = std::fs::read_to_string(path).map_err(|source| IconError::Io {
                path: path.to_owned(),
                source,
            })?;
            decode_svg(&text, size).map_err(|err| match err {
                IconError::SvgEmptySize { .. } => IconError::SvgEmptySize {
                    path: Some(path.to_owned()),
                },
                other => other,
            })
        }
        Some("png") => {
            let bytes = std::fs::read(path).map_err(|source| IconError::Io {
                path: path.to_owned(),
                source,
            })?;
            decode_png(&bytes)
        }
        _ => Err(IconError::UnsupportedFormat {
            path: path.to_owned(),
            extension,
        }),
    }
}

/// [`load_icon_at_size`] at [`crate::DEFAULT_ICON_SIZE`].
pub fn load_icon(path: &Path) -> Result<RawIcon, IconError> {
    load_icon_at_size(path, crate::DEFAULT_ICON_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE_CIRCLE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <circle cx="16" cy="16" r="14" fill="#3366ff"/>
</svg>"##;

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * width + x) * 4) as usize;
        rgba[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn decode_svg_rasterizes_to_the_requested_size() {
        let icon = decode_svg(BLUE_CIRCLE_SVG, 64).unwrap();
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
    }

    #[test]
    fn decode_svg_unpremultiplies_a_known_fill_color_at_center() {
        let icon = decode_svg(BLUE_CIRCLE_SVG, 64).unwrap();
        let [r, g, b, a] = pixel(&icon.rgba, 64, 32, 32);
        assert_eq!(a, 255, "center of the circle should be fully opaque");
        assert!(
            b > r && b > g,
            "center should be blue-dominant, got {r},{g},{b}"
        );
    }

    #[test]
    fn decode_svg_leaves_a_fully_transparent_pixel_at_alpha_zero() {
        let icon = decode_svg(BLUE_CIRCLE_SVG, 64).unwrap();
        let [_, _, _, a] = pixel(&icon.rgba, 64, 0, 0);
        assert_eq!(a, 0, "outside the circle should be transparent");
    }

    #[test]
    fn decode_svg_rejects_unparseable_markup() {
        let err = decode_svg("not an svg at all", 64).unwrap_err();
        assert!(matches!(err, IconError::Svg(_)));
    }

    fn encode_test_png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([x as u8, y as u8, 128, 255])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn decode_png_reports_the_real_dimensions() {
        let bytes = encode_test_png(17, 9);
        let icon = decode_png(&bytes).unwrap();
        assert_eq!((icon.width, icon.height), (17, 9));
        assert_eq!(icon.rgba.len(), 17 * 9 * 4);
    }

    #[test]
    fn decode_png_rejects_corrupt_bytes() {
        let err = decode_png(&[0, 1, 2, 3, 4]).unwrap_err();
        assert!(matches!(err, IconError::Png(_)));
    }

    #[test]
    fn load_icon_at_size_dispatches_svg_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.svg");
        std::fs::write(&path, BLUE_CIRCLE_SVG).unwrap();
        let icon = load_icon_at_size(&path, 48).unwrap();
        assert_eq!((icon.width, icon.height), (48, 48));
    }

    #[test]
    fn load_icon_at_size_dispatches_png_by_extension_and_ignores_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.PNG"); // case-insensitive extension
        std::fs::write(&path, encode_test_png(10, 10)).unwrap();
        let icon = load_icon_at_size(&path, 999).unwrap();
        assert_eq!((icon.width, icon.height), (10, 10));
    }

    #[test]
    fn load_icon_at_size_rejects_an_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.bmp");
        std::fs::write(&path, b"whatever").unwrap();
        let err = load_icon_at_size(&path, 64).unwrap_err();
        assert!(matches!(err, IconError::UnsupportedFormat { .. }));
    }

    #[test]
    fn load_icon_at_size_reports_a_missing_file_as_io_error() {
        let err = load_icon_at_size(Path::new("does/not/exist.svg"), 64).unwrap_err();
        assert!(matches!(err, IconError::Io { .. }));
    }

    #[test]
    fn load_icon_defaults_to_the_standard_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.svg");
        std::fs::write(&path, BLUE_CIRCLE_SVG).unwrap();
        let icon = load_icon(&path).unwrap();
        assert_eq!(
            (icon.width, icon.height),
            (crate::DEFAULT_ICON_SIZE, crate::DEFAULT_ICON_SIZE)
        );
    }
}

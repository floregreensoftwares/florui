//! Per-pixel comparison between a Chromium reference capture and Florui's
//! own render: dimension/alpha checks, percent/mean/max error, and a diff
//! image, per this project's visual-comparison criteria.

use std::fmt;

use image::{Rgba as ImageRgba, RgbaImage};
use serde::{Deserialize, Serialize};

/// Per-channel and alpha tolerance, in 0-255 units. `0`/`0` matches the rule
/// that fixtures classified as exact require zero differing pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelDiffOptions {
    pub channel_tolerance: u8,
    pub alpha_tolerance: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PixelSummary {
    pub differing_pixels: u64,
    pub total_pixels: u64,
    pub percent_different: f64,
    pub mean_error: f64,
    pub max_error: u8,
}

impl PixelSummary {
    pub fn is_exact_match(&self) -> bool {
        self.differing_pixels == 0
    }
}

#[derive(Debug)]
pub struct PixelReport {
    pub summary: PixelSummary,
    /// Transparent where pixels match, opaque red where they differ.
    pub diff_image: RgbaImage,
}

#[derive(Debug)]
pub enum PixelCompareError {
    DimensionMismatch {
        reference: (u32, u32),
        result: (u32, u32),
    },
}

impl fmt::Display for PixelCompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelCompareError::DimensionMismatch { reference, result } => write!(
                f,
                "image dimensions differ: reference is {}x{}, result is {}x{}",
                reference.0, reference.1, result.0, result.1
            ),
        }
    }
}

impl std::error::Error for PixelCompareError {}

/// Compares `reference` (Chromium capture) against `result` (Florui's own
/// render) pixel by pixel. Dimensions and alpha are checked explicitly,
/// rather than folded silently into a single color distance metric.
///
/// Both images are treated as plain sRGB byte buffers with no color-profile
/// normalization — a documented bootstrap limitation. No fixture with a
/// non-default color profile should be added until that normalization
/// exists.
pub fn compare_pixels(
    reference: &RgbaImage,
    result: &RgbaImage,
    options: &PixelDiffOptions,
) -> Result<PixelReport, PixelCompareError> {
    if reference.dimensions() != result.dimensions() {
        return Err(PixelCompareError::DimensionMismatch {
            reference: reference.dimensions(),
            result: result.dimensions(),
        });
    }

    let (width, height) = reference.dimensions();
    let total_pixels = u64::from(width) * u64::from(height);
    let mut differing_pixels = 0u64;
    let mut error_sum = 0f64;
    let mut max_error = 0u8;
    let mut diff_image = RgbaImage::new(width, height);

    for (x, y, reference_pixel) in reference.enumerate_pixels() {
        let result_pixel = result.get_pixel(x, y);
        let channel_error = reference_pixel.0[..3]
            .iter()
            .zip(result_pixel.0[..3].iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        let alpha_error = reference_pixel.0[3].abs_diff(result_pixel.0[3]);

        let differs =
            channel_error > options.channel_tolerance || alpha_error > options.alpha_tolerance;
        if differs {
            differing_pixels += 1;
            diff_image.put_pixel(x, y, ImageRgba([255, 0, 0, 255]));
        } else {
            diff_image.put_pixel(x, y, ImageRgba([0, 0, 0, 0]));
        }

        error_sum += f64::from(channel_error.max(alpha_error));
        max_error = max_error.max(channel_error).max(alpha_error);
    }

    let percent_different = if total_pixels == 0 {
        0.0
    } else {
        (differing_pixels as f64 / total_pixels as f64) * 100.0
    };
    let mean_error = if total_pixels == 0 {
        0.0
    } else {
        error_sum / total_pixels as f64
    };

    Ok(PixelReport {
        summary: PixelSummary {
            differing_pixels,
            total_pixels,
            percent_different,
            mean_error,
            max_error,
        },
        diff_image,
    })
}

/// Composites `reference` and `result` at 50% opacity each, as the
/// "overlay" failure artifact — enough to spot a misplaced or misrendered
/// region at a glance.
///
/// Precondition: both images have the same dimensions. Callers only
/// produce an overlay after a successful [`compare_pixels`] call, which
/// already guarantees this.
pub fn overlay_images(reference: &RgbaImage, result: &RgbaImage) -> RgbaImage {
    let (width, height) = reference.dimensions();
    RgbaImage::from_fn(width, height, |x, y| {
        let a = reference.get_pixel(x, y).0;
        let b = result.get_pixel(x, y).0;
        let blend = |i: usize| ((u16::from(a[i]) + u16::from(b[i])) / 2) as u8;
        ImageRgba([blend(0), blend(1), blend(2), blend(3)])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_fn(width, height, |_, _| ImageRgba(color))
    }

    #[test]
    fn identical_images_have_zero_diff() {
        let a = solid(10, 10, [10, 20, 30, 255]);
        let b = solid(10, 10, [10, 20, 30, 255]);
        let report = compare_pixels(&a, &b, &PixelDiffOptions::default()).unwrap();
        assert!(report.summary.is_exact_match());
        assert_eq!(report.summary.percent_different, 0.0);
        assert_eq!(report.summary.mean_error, 0.0);
        assert_eq!(report.summary.max_error, 0);
    }

    #[test]
    fn a_single_differing_pixel_is_measured_correctly() {
        let mut a = solid(10, 10, [0, 0, 0, 255]);
        let b = solid(10, 10, [0, 0, 0, 255]);
        a.put_pixel(3, 4, ImageRgba([50, 0, 0, 255]));

        let report = compare_pixels(&a, &b, &PixelDiffOptions::default()).unwrap();
        assert_eq!(report.summary.differing_pixels, 1);
        assert_eq!(report.summary.total_pixels, 100);
        assert_eq!(report.summary.percent_different, 1.0);
        assert_eq!(report.summary.max_error, 50);
        // Only the differing pixel contributes error, spread across 100 pixels.
        assert!((report.summary.mean_error - 0.5).abs() < 1e-9);
        assert_eq!(
            *report.diff_image.get_pixel(3, 4),
            ImageRgba([255, 0, 0, 255])
        );
        assert_eq!(*report.diff_image.get_pixel(0, 0), ImageRgba([0, 0, 0, 0]));
    }

    #[test]
    fn alpha_mismatch_is_detected_even_with_matching_color() {
        let a = solid(4, 4, [10, 10, 10, 255]);
        let b = solid(4, 4, [10, 10, 10, 200]);
        let report = compare_pixels(&a, &b, &PixelDiffOptions::default()).unwrap();
        assert!(!report.summary.is_exact_match());
        assert_eq!(report.summary.differing_pixels, 16);
    }

    #[test]
    fn tolerance_absorbs_small_differences() {
        let a = solid(4, 4, [10, 10, 10, 255]);
        let b = solid(4, 4, [12, 10, 10, 255]);
        let options = PixelDiffOptions {
            channel_tolerance: 5,
            alpha_tolerance: 0,
        };
        let report = compare_pixels(&a, &b, &options).unwrap();
        assert!(report.summary.is_exact_match());
    }

    #[test]
    fn overlay_blends_both_images_evenly() {
        let a = solid(2, 2, [0, 0, 0, 255]);
        let b = solid(2, 2, [200, 100, 50, 255]);
        let overlay = overlay_images(&a, &b);
        assert_eq!(*overlay.get_pixel(0, 0), ImageRgba([100, 50, 25, 255]));
    }

    #[test]
    fn dimension_mismatch_is_a_hard_error() {
        let a = solid(10, 10, [0, 0, 0, 255]);
        let b = solid(10, 20, [0, 0, 0, 255]);
        let error = compare_pixels(&a, &b, &PixelDiffOptions::default()).unwrap_err();
        assert!(matches!(
            error,
            PixelCompareError::DimensionMismatch {
                reference: (10, 10),
                result: (10, 20)
            }
        ));
    }
}

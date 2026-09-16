//! A hand-rolled Gaussian blur for an 8-bit alpha mask — see this crate's
//! own module doc for why: tiny-skia has no blur or mask-filter primitive
//! of its own to paint `box-shadow`'s `blur-radius` with.
//!
//! # Matching the CSS spec's blur radius
//!
//! CSS Backgrounds and Borders §7.2 doesn't mandate a specific blur
//! algorithm, but its own non-normative note gives the correspondence
//! real browsers use: a `blur-radius` of `X` corresponds to a Gaussian
//! blur with standard deviation `X / 2`. [`gaussian_blur_in_place`] takes
//! that already-converted `sigma_px`, not the raw CSS blur radius.
//!
//! A direct separable convolution (one horizontal pass, one vertical
//! pass), not a box-blur approximation of one: this runs once per
//! painted shadow, not per animation frame, so the simpler, obviously
//! correct implementation is worth more than the extra speed a box-blur
//! approximation would buy.

/// A normalized 1-D Gaussian kernel, truncated at `3 * sigma_px` — beyond
/// that the contribution is visually negligible (well under 1% of the
/// peak) and not worth the extra taps. The returned radius is how far the
/// kernel reaches on each side of its center tap.
pub(crate) fn gaussian_kernel(sigma_px: f32) -> Vec<f32> {
    let radius = (sigma_px * 3.0).ceil().max(1.0) as i32;
    let two_sigma_sq = 2.0 * sigma_px * sigma_px;
    let mut kernel = Vec::with_capacity((radius * 2 + 1) as usize);
    let mut sum = 0.0f32;
    for offset in -radius..=radius {
        let weight = (-((offset * offset) as f32) / two_sigma_sq).exp();
        kernel.push(weight);
        sum += weight;
    }
    for weight in &mut kernel {
        *weight /= sum;
    }
    kernel
}

/// How far a [`gaussian_kernel`] built from `sigma_px` reaches on each
/// side of its center tap — the minimum margin a caller needs to pad an
/// alpha buffer by on every side so the convolution below never needs a
/// value it doesn't have.
pub(crate) fn kernel_radius(sigma_px: f32) -> u32 {
    (sigma_px * 3.0).ceil().max(1.0) as u32
}

/// Blurs `samples` (a `width x height`, row-major, row `0` at the top
/// alpha buffer) in place with a real separable Gaussian of standard
/// deviation `sigma_px`. Every value the kernel would need from outside
/// the buffer is treated as `0` — correct as long as the caller padded
/// the buffer by at least [`kernel_radius`] beyond whatever real content
/// it painted into it (see this module's own doc), not merely clamped to
/// an edge value.
pub(crate) fn gaussian_blur_in_place(samples: &mut [u8], width: u32, height: u32, sigma_px: f32) {
    let kernel = gaussian_kernel(sigma_px);
    let radius = (kernel.len() / 2) as i32;
    let (w, h) = (width as i32, height as i32);

    let source: Vec<f32> = samples.iter().map(|&byte| byte as f32).collect();
    let mut horizontal = vec![0.0f32; source.len()];
    for row in 0..h {
        for col in 0..w {
            let mut acc = 0.0f32;
            for (tap, &weight) in kernel.iter().enumerate() {
                let sample_col = col + tap as i32 - radius;
                if sample_col >= 0 && sample_col < w {
                    acc += source[(row * w + sample_col) as usize] * weight;
                }
            }
            horizontal[(row * w + col) as usize] = acc;
        }
    }

    let mut vertical = vec![0.0f32; source.len()];
    for row in 0..h {
        for col in 0..w {
            let mut acc = 0.0f32;
            for (tap, &weight) in kernel.iter().enumerate() {
                let sample_row = row + tap as i32 - radius;
                if sample_row >= 0 && sample_row < h {
                    acc += horizontal[(sample_row * w + col) as usize] * weight;
                }
            }
            vertical[(row * w + col) as usize] = acc;
        }
    }

    for (dest, value) in samples.iter_mut().zip(vertical) {
        *dest = value.round().clamp(0.0, 255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_weights_sum_to_one() {
        for sigma in [0.5, 1.0, 4.0, 12.5] {
            let kernel = gaussian_kernel(sigma);
            let sum: f32 = kernel.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-4,
                "sigma {sigma}: kernel sum {sum}, expected ~1.0"
            );
        }
    }

    #[test]
    fn kernel_is_symmetric_and_peaks_at_center() {
        let kernel = gaussian_kernel(3.0);
        let mid = kernel.len() / 2;
        for offset in 1..=mid {
            assert!(
                (kernel[mid - offset] - kernel[mid + offset]).abs() < 1e-6,
                "kernel should be symmetric around its center"
            );
            assert!(
                kernel[mid] >= kernel[mid - offset],
                "center weight should be the peak"
            );
        }
    }

    /// A solid rect stamped into a padded buffer and blurred: alpha must
    /// fall off monotonically moving away from the shape's own edge, not
    /// dip and recover — the exact defect class an inverted blend factor
    /// would produce.
    #[test]
    fn blurring_a_solid_rect_falls_off_monotonically_away_from_the_edge() {
        let width = 120u32;
        let height = 80u32;
        let mut samples = vec![0u8; (width as usize) * (height as usize)];
        // A 60x40 solid rect at (10,10)-(70,50).
        for row in 10..50u32 {
            for col in 10..70u32 {
                samples[(row * width + col) as usize] = 255;
            }
        }

        gaussian_blur_in_place(&mut samples, width, height, 6.0);

        let row = 30u32; // vertical middle of the shape, clear of corners
        let mut previous = 255u8;
        for col in 70..110u32 {
            let value = samples[(row * width + col) as usize];
            assert!(
                value <= previous,
                "alpha increased moving away from the shape at column {col}: {value} > {previous}"
            );
            previous = value;
        }
        assert!(
            previous < 40,
            "expected the mask to have faded well below full opacity by the far edge, got \
             {previous}"
        );
    }

    #[test]
    fn deep_inside_stays_opaque_and_far_outside_stays_transparent() {
        let width = 40u32;
        let height = 40u32;
        let mut samples = vec![0u8; (width as usize) * (height as usize)];
        for row in 5..35u32 {
            for col in 5..35u32 {
                samples[(row * width + col) as usize] = 255;
            }
        }

        gaussian_blur_in_place(&mut samples, width, height, 2.0);

        assert_eq!(samples[(20 * width + 20) as usize], 255, "deep inside");
        assert_eq!(samples[(width + 1) as usize], 0, "far outside");
    }

    #[test]
    fn kernel_radius_grows_with_sigma_and_is_never_zero() {
        assert_eq!(kernel_radius(0.01), 1);
        assert!(kernel_radius(10.0) > kernel_radius(2.0));
    }
}

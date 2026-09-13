//! Physical-to-logical pixel conversion for a HiDPI-aware desktop host.
//!
//! **Not yet wired into [`crate::run`]'s desktop host.** Converting only
//! the layout viewport to logical pixels would shrink everything the
//! tree paints into a corner of an unchanged, still-physical-resolution
//! canvas — layout and painting both need to move to a consistent unit
//! together, or neither should. This module carries the arithmetic
//! (already correct, already tested against `winit`'s own conversion) so
//! it can be reviewed on its own before that wiring lands, rather than as
//! one large, harder-to-review change mixing the two.
//!
//! See `florui-platform`'s crate-level scope note for the underlying gap
//! this is tracking: the desktop host currently passes physical pixels
//! straight through as the layout viewport, with no device-pixel-ratio
//! scaling at all.

use winit::dpi::{LogicalSize, PhysicalSize};

/// A window's real physical size, expressed as the logical (CSS-style)
/// size `florui_style`'s declared `width`/`height` are actually meant to
/// be measured against, plus the `scale_factor` a paint step would need
/// to multiply computed layout positions back up by — rendering at full
/// device resolution, rather than blurrily upscaling a logical-resolution
/// canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportScale {
    pub logical: LogicalSize<f32>,
    pub scale_factor: f64,
}

/// Computes the logical size a `winit` window's physical size and scale
/// factor correspond to. A thin wrapper over `winit`'s own
/// [`PhysicalSize::to_logical`] — the conversion itself is not this
/// crate's to redefine, only to apply consistently once wired in.
pub fn viewport_scale(physical: PhysicalSize<u32>, scale_factor: f64) -> ViewportScale {
    ViewportScale {
        logical: physical.to_logical(scale_factor),
        scale_factor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_factor_one_is_the_identity() {
        let result = viewport_scale(PhysicalSize::new(800, 600), 1.0);
        assert_eq!(result.logical, LogicalSize::new(800.0, 600.0));
    }

    #[test]
    fn a_2x_display_halves_the_logical_size() {
        let result = viewport_scale(PhysicalSize::new(1600, 1200), 2.0);
        assert_eq!(result.logical, LogicalSize::new(800.0, 600.0));
    }

    #[test]
    fn scale_factor_is_reported_alongside_the_logical_size() {
        let result = viewport_scale(PhysicalSize::new(1600, 1200), 2.0);
        assert_eq!(result.scale_factor, 2.0);
    }

    #[test]
    fn a_fractional_scale_factor_matches_winit_s_own_conversion_exactly() {
        // 1.5x is a common HiDPI setting ("150%"); this isn't asserting a
        // specific rounding rule florui doesn't own, only that this
        // wrapper doesn't add any behavior of its own on top of winit's.
        let physical = PhysicalSize::new(1200u32, 900u32);
        let direct: LogicalSize<f32> = physical.to_logical(1.5);
        let wrapped = viewport_scale(physical, 1.5);
        assert_eq!(wrapped.logical, direct);
    }
}

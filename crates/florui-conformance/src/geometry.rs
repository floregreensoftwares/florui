//! Compares the selected element's box between a Chromium reference and
//! Florui's own engine, as this project's geometry comparison layer. Both
//! boxes are expressed in CSS pixels; DPR conversion from the engine's
//! physical-pixel box happens here, not inside `florui-devtools`, so the
//! engine's own types stay DPR-agnostic.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoxGeometryPx {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BoxGeometryPx {
    /// Converts an engine box given in physical pixels to CSS pixels.
    pub fn from_physical(x: u32, y: u32, width: u32, height: u32, device_pixel_ratio: f64) -> Self {
        Self {
            x: x as f64 / device_pixel_ratio,
            y: y as f64 / device_pixel_ratio,
            width: width as f64 / device_pixel_ratio,
            height: height as f64 / device_pixel_ratio,
        }
    }

    fn max_axis_delta(&self, other: &Self) -> f64 {
        [
            (self.x - other.x).abs(),
            (self.y - other.y).abs(),
            (self.width - other.width).abs(),
            (self.height - other.height).abs(),
        ]
        .into_iter()
        .fold(0.0, f64::max)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeometryReport {
    pub reference: BoxGeometryPx,
    pub engine: BoxGeometryPx,
    pub max_axis_delta_px: f64,
    pub within_tolerance: bool,
}

/// Compares `reference` (from the Chromium capture's `getBoundingClientRect`)
/// against `engine` (Florui's own recorded box). The exact fixture uses
/// `tolerance_px = 0.0`.
pub fn compare_geometry(
    reference: BoxGeometryPx,
    engine: BoxGeometryPx,
    tolerance_px: f64,
) -> GeometryReport {
    let max_axis_delta_px = reference.max_axis_delta(&engine);
    GeometryReport {
        reference,
        engine,
        max_axis_delta_px,
        within_tolerance: max_axis_delta_px <= tolerance_px,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_boxes_are_exact() {
        let box_a = BoxGeometryPx {
            x: 56.0,
            y: 8.0,
            width: 240.0,
            height: 192.0,
        };
        let report = compare_geometry(box_a, box_a, 0.0);
        assert_eq!(report.max_axis_delta_px, 0.0);
        assert!(report.within_tolerance);
    }

    #[test]
    fn a_shifted_edge_is_out_of_tolerance_at_zero() {
        let reference = BoxGeometryPx {
            x: 56.0,
            y: 8.0,
            width: 240.0,
            height: 192.0,
        };
        // left/right swapped relative to the fixture's actual insets.
        let engine = BoxGeometryPx {
            x: 24.0,
            y: 8.0,
            width: 240.0,
            height: 192.0,
        };
        let report = compare_geometry(reference, engine, 0.0);
        assert_eq!(report.max_axis_delta_px, 32.0);
        assert!(!report.within_tolerance);
    }

    #[test]
    fn delta_within_a_positive_tolerance_passes() {
        let reference = BoxGeometryPx {
            x: 56.0,
            y: 8.0,
            width: 240.0,
            height: 192.0,
        };
        let engine = BoxGeometryPx {
            x: 56.4,
            ..reference
        };
        let report = compare_geometry(reference, engine, 0.5);
        assert!(report.within_tolerance);
    }

    #[test]
    fn from_physical_divides_by_device_pixel_ratio() {
        let box_px = BoxGeometryPx::from_physical(112, 16, 480, 384, 2.0);
        assert_eq!(
            box_px,
            BoxGeometryPx {
                x: 56.0,
                y: 8.0,
                width: 240.0,
                height: 192.0,
            }
        );
    }
}

//! Loads a reference fixture: an HTML/CSS pair plus a manifest describing
//! viewport, DPR, and a neutral expected result, per this project's fixture
//! contract.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewportSpec {
    pub width_css_px: u32,
    pub height_css_px: u32,
    pub device_pixel_ratio: f64,
}

impl ViewportSpec {
    pub fn width_physical_px(&self) -> u32 {
        (self.width_css_px as f64 * self.device_pixel_ratio).round() as u32
    }

    pub fn height_physical_px(&self) -> u32 {
        (self.height_css_px as f64 * self.device_pixel_ratio).round() as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsetsSpec {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl InsetsSpec {
    /// Converts these CSS-pixel insets to the physical-pixel [`Insets`]
    /// Florui's engine functions expect, scaling by `device_pixel_ratio`.
    ///
    /// There is deliberately no `From<InsetsSpec> for Insets`: a
    /// DPR-unaware conversion silently produces the wrong physical box at
    /// any `device_pixel_ratio != 1.0`, which is exactly the kind of bug
    /// real DPR validation exists to catch.
    pub fn to_physical(self, device_pixel_ratio: f64) -> florui_devtools::scene::Insets {
        let scale = |value: u32| (f64::from(value) * device_pixel_ratio).round() as u32;
        florui_devtools::scene::Insets {
            top: scale(self.top),
            right: scale(self.right),
            bottom: scale(self.bottom),
            left: scale(self.left),
        }
    }
}

/// States the fixture's expected canvas/element colors and geometry
/// literally, mirroring the numbers already written in its own CSS.
///
/// Expected results must be a neutral description, without using the
/// layout implementation itself to generate expected output — and Florui
/// has no real cascade or layout yet to derive them from. This is a
/// documented stopgap: once real style/layout exists, this field should be
/// deleted and the engine side computed from the same CSS Chromium reads,
/// instead of duplicated by hand here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedResult {
    pub canvas_color: String,
    pub element_color: String,
    pub insets_css_px: InsetsSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Classification {
    Exact,
    Tolerant {
        threshold_percent: f64,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureManifest {
    pub id: String,
    pub description: String,
    pub html: String,
    pub css: String,
    pub element_id: String,
    pub viewport: ViewportSpec,
    pub expected: ExpectedResult,
    pub classification: Classification,
}

#[derive(Debug, Clone)]
pub struct ReferenceFixture {
    pub dir: PathBuf,
    pub manifest: FixtureManifest,
}

impl ReferenceFixture {
    pub fn html_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.html)
    }

    pub fn css_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.css)
    }
}

#[derive(Debug)]
pub enum FixtureError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    MissingAsset {
        path: PathBuf,
    },
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixtureError::Read { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            FixtureError::Parse { path, source } => {
                write!(f, "could not parse manifest {}: {source}", path.display())
            }
            FixtureError::MissingAsset { path } => {
                write!(f, "fixture asset not found: {}", path.display())
            }
        }
    }
}

impl std::error::Error for FixtureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FixtureError::Read { source, .. } => Some(source),
            FixtureError::Parse { source, .. } => Some(source),
            FixtureError::MissingAsset { .. } => None,
        }
    }
}

/// Loads `dir/manifest.json` and validates that its declared `html`/`css`
/// assets actually exist alongside it.
pub fn load_reference_fixture(dir: &Path) -> Result<ReferenceFixture, FixtureError> {
    let manifest_path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).map_err(|source| FixtureError::Read {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest: FixtureManifest =
        serde_json::from_str(&text).map_err(|source| FixtureError::Parse {
            path: manifest_path.clone(),
            source,
        })?;

    let fixture = ReferenceFixture {
        dir: dir.to_owned(),
        manifest,
    };

    for path in [fixture.html_path(), fixture.css_path()] {
        if !path.exists() {
            return Err(FixtureError::MissingAsset { path });
        }
    }

    Ok(fixture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(dir: &Path, manifest_json: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest_json).unwrap();
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        std::fs::write(dir.join("style.css"), "body {}").unwrap();
    }

    fn valid_manifest_json() -> &'static str {
        r##"{
            "id": "test-fixture",
            "description": "a test fixture",
            "html": "index.html",
            "css": "style.css",
            "element_id": "el",
            "viewport": { "width_css_px": 100, "height_css_px": 80, "device_pixel_ratio": 1.0 },
            "expected": {
                "canvas_color": "#111111",
                "element_color": "#222222",
                "insets_css_px": { "top": 1, "right": 2, "bottom": 3, "left": 4 }
            },
            "classification": { "kind": "exact" }
        }"##
    }

    fn scratch_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "florui-conformance-fixture-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn loads_a_valid_fixture() {
        let dir = scratch_dir("valid");
        write_fixture(&dir, valid_manifest_json());

        let fixture = load_reference_fixture(&dir).unwrap();
        assert_eq!(fixture.manifest.id, "test-fixture");
        assert_eq!(fixture.manifest.viewport.width_css_px, 100);
        assert_eq!(fixture.manifest.expected.insets_css_px.left, 4);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_asset_is_reported() {
        let dir = scratch_dir("missing-asset");
        write_fixture(&dir, valid_manifest_json());
        std::fs::remove_file(dir.join("style.css")).unwrap();

        let error = load_reference_fixture(&dir).unwrap_err();
        assert!(matches!(error, FixtureError::MissingAsset { .. }));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_json_is_reported() {
        let dir = scratch_dir("malformed");
        write_fixture(&dir, "{ not valid json");

        let error = load_reference_fixture(&dir).unwrap_err();
        assert!(matches!(error, FixtureError::Parse { .. }));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn viewport_scales_by_device_pixel_ratio() {
        let viewport = ViewportSpec {
            width_css_px: 320,
            height_css_px: 240,
            device_pixel_ratio: 2.0,
        };
        assert_eq!(viewport.width_physical_px(), 640);
        assert_eq!(viewport.height_physical_px(), 480);
    }

    #[test]
    fn insets_to_physical_scales_every_edge_independently() {
        let insets = InsetsSpec {
            top: 8,
            right: 24,
            bottom: 40,
            left: 56,
        };
        let physical = insets.to_physical(2.0);
        assert_eq!(
            physical,
            florui_devtools::scene::Insets {
                top: 16,
                right: 48,
                bottom: 80,
                left: 112,
            }
        );
    }

    #[test]
    fn insets_to_physical_is_identity_at_dpr_one() {
        let insets = InsetsSpec {
            top: 8,
            right: 24,
            bottom: 40,
            left: 56,
        };
        let physical = insets.to_physical(1.0);
        assert_eq!(
            physical,
            florui_devtools::scene::Insets {
                top: 8,
                right: 24,
                bottom: 40,
                left: 56,
            }
        );
    }
}

//! Loads a reference fixture: an HTML/CSS pair (the Chromium side) plus a
//! [`FloruiSpec`] (the same element, rendered through Florui's own real
//! style/layout/paint pipeline — see [`crate::engine`]) and a manifest
//! tying the two together.

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

/// The same element under test, rendered through Florui's own real
/// style/layout/paint pipeline (see [`crate::engine::render_fixture`])
/// instead of the Chromium HTML/CSS pair — no expected pixel/geometry
/// values are hand-typed anywhere in this crate any more: both sides are
/// real renders, compared against each other by [`crate::pixels`]/
/// [`crate::geometry`].
///
/// `tag`/`text` describe the single element under test (wrapped in a
/// synthetic `<div>` matching the HTML side's `<body>`, so a bare
/// `<span>`'s real inline-vs-block distinction isn't lost to Stylo's own
/// root-element blockification — see `florui_style::stylo`'s
/// `to_display` doc for that rule). `css` is real author CSS through the
/// same `florui_style::parse_stylesheet` path an application uses; empty
/// for a bare-element fixture, so the framework's own default stylesheet
/// is what's actually under test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloruiSpec {
    pub tag: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub css: String,
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
    /// Written into the Chromium HTML/CSS pair's own `body`
    /// `background-color` *and* passed as Florui's own `paint_to_buffer`
    /// canvas color — one declared value for both renders, rather than
    /// two hand-typed strings that could silently drift apart.
    pub canvas_color: String,
    pub florui: FloruiSpec,
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
            "canvas_color": "#111111",
            "florui": { "tag": "div", "text": "", "css": "" },
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
        assert_eq!(fixture.manifest.florui.tag, "div");

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
}

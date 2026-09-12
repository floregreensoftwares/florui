//! End-to-end check against a real Chromium binary: launches it, captures
//! `fixtures/reference/inset-rect-exact`, and compares it against Florui's
//! own render of the same insets.
//!
//! `#[ignore]`d because no Chromium revision is pinned/vendored for CI in
//! this slice (see `driver.rs`'s `ChromiumOptions::executable` docs). Run
//! locally with an explicit binary:
//!
//! ```sh
//! FLORUI_CHROMIUM_TEST="C:/Program Files/Google/Chrome/Application/chrome.exe" \
//!     cargo test -p florui-conformance -- --ignored
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use florui_conformance::driver::{ChromiumDriver, ChromiumOptions};
use florui_conformance::geometry::{BoxGeometryPx, compare_geometry};
use florui_conformance::pixels::{PixelDiffOptions, compare_pixels};
use florui_conformance::reference_fixture::load_reference_fixture;
use florui_devtools::scene::{element_box, render_rgba8_inset};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("reference")
        .join("inset-rect-exact")
}

#[test]
#[ignore = "requires a real Chromium binary, set FLORUI_CHROMIUM_TEST to run"]
fn exact_fixture_matches_chromium() {
    let Some(chromium) = std::env::var_os("FLORUI_CHROMIUM_TEST") else {
        panic!(
            "set FLORUI_CHROMIUM_TEST to a Chrome/Edge/Chromium executable path to run this test"
        );
    };

    let fixture = load_reference_fixture(&fixture_dir()).expect("fixture should load");

    let profile_dir =
        std::env::temp_dir().join(format!("florui-conformance-it-{}", std::process::id()));
    let driver = ChromiumDriver::launch(ChromiumOptions {
        executable: PathBuf::from(chromium),
        user_data_dir: profile_dir.clone(),
        headless: true,
        launch_timeout: Duration::from_secs(30),
    })
    .expect("driver should launch");

    let capture = driver.capture(&fixture).expect("capture should succeed");

    let viewport = &fixture.manifest.viewport;
    let insets = fixture
        .manifest
        .expected
        .insets_css_px
        .to_physical(viewport.device_pixel_ratio);
    let engine_pixels = render_rgba8_inset(
        viewport.width_physical_px(),
        viewport.height_physical_px(),
        parse_hex(&fixture.manifest.expected.canvas_color),
        parse_hex(&fixture.manifest.expected.element_color),
        insets,
    );
    let engine_image = image::RgbaImage::from_raw(
        viewport.width_physical_px(),
        viewport.height_physical_px(),
        engine_pixels,
    )
    .expect("engine pixels should match declared dimensions");

    let pixel_report = compare_pixels(&capture.image, &engine_image, &PixelDiffOptions::default())
        .expect("dimensions should match");
    assert!(
        pixel_report.summary.is_exact_match(),
        "expected zero pixel diff, got {} differing pixels ({:.2}%)",
        pixel_report.summary.differing_pixels,
        pixel_report.summary.percent_different
    );

    let engine_box = element_box(
        viewport.width_physical_px(),
        viewport.height_physical_px(),
        insets,
    )
    .expect("box should fit the viewport");
    let engine_box_css_px = BoxGeometryPx::from_physical(
        engine_box.x,
        engine_box.y,
        engine_box.width,
        engine_box.height,
        viewport.device_pixel_ratio,
    );
    let geometry_report = compare_geometry(capture.element_box_css_px, engine_box_css_px, 0.0);
    assert!(
        geometry_report.within_tolerance,
        "geometry mismatch: reference {:?} vs engine {:?} (max delta {}px)",
        geometry_report.reference, geometry_report.engine, geometry_report.max_axis_delta_px
    );

    std::fs::remove_dir_all(&profile_dir).ok();
}

fn parse_hex(hex: &str) -> florui_devtools::color::Rgba {
    florui_devtools::color::parse_hex_color(hex).expect("fixture colors must be valid hex")
}

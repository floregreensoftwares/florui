use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};

use florui_conformance::driver::{ChromiumDriver, ChromiumOptions};
use florui_conformance::geometry::{BoxGeometryPx, compare_geometry};
use florui_conformance::pixels::{PixelDiffOptions, compare_pixels, overlay_images};
use florui_conformance::reference_fixture::load_reference_fixture;
use florui_conformance::report::{ArtifactPaths, Outcome, Report, classify, write_report};
use florui_devtools::diagnostics::{dim_text, failure, success};
use florui_devtools::scene::{element_box, render_rgba8_inset};

#[derive(Parser)]
#[command(name = "florui", about = "Florui project CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a minimal compiling application.
    New { name: String },
    /// Launch the native preview host and watch a CSS fixture for changes.
    Dev {
        #[arg(long, default_value = "fixtures/dev/app.css")]
        fixture: PathBuf,
    },
    /// Run component, lifecycle, geometry, and visual test suites.
    Test,
    /// Compare a reference fixture's Chromium capture against Florui's own render.
    Compare {
        /// Directory containing the fixture's manifest.json, HTML, and CSS.
        #[arg(long, default_value = "fixtures/reference/inset-rect-exact")]
        fixture: PathBuf,
        /// Path to a Chromium-family binary (Chrome, Chromium, or Edge).
        #[arg(long, env = "FLORUI_CHROMIUM")]
        chromium: Option<PathBuf>,
        /// Directory to write report.json and image artifacts under.
        #[arg(long, default_value = "target/florui-conformance")]
        out_dir: PathBuf,
        /// Run Chromium with a visible window instead of headless.
        #[arg(long)]
        headed: bool,
        /// Keep the isolated Chromium profile directory after the run.
        #[arg(long)]
        keep_profile: bool,
    },
    /// Produce a release/debug artifact for a supported target.
    Build {
        #[arg(long, default_value = "native")]
        target: String,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Dev { fixture } => run_dev(fixture),
        Command::New { name } => not_implemented(&format!("`florui new {name}`")),
        Command::Test => not_implemented("`florui test`"),
        Command::Compare {
            fixture,
            chromium,
            out_dir,
            headed,
            keep_profile,
        } => run_compare(fixture, chromium, out_dir, headed, keep_profile),
        Command::Build { target } => not_implemented(&format!("`florui build --target {target}`")),
    }
}

fn run_dev(fixture: PathBuf) -> ExitCode {
    if !fixture.exists() {
        eprintln!("fixture not found: {}", fixture.display());
        return ExitCode::FAILURE;
    }
    match florui_devtools::preview::run(fixture) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("{}", failure(&message.to_string()));
    ExitCode::FAILURE
}

fn cleanup_profile(dir: &Path, keep: bool) {
    if !keep {
        std::fs::remove_dir_all(dir).ok();
    }
}

fn run_compare(
    fixture_path: PathBuf,
    chromium: Option<PathBuf>,
    out_dir: PathBuf,
    headed: bool,
    keep_profile: bool,
) -> ExitCode {
    let chromium = match chromium {
        Some(chromium) => chromium,
        None => {
            let pinned = florui_conformance::pin::default_executable_path();
            if pinned.exists() {
                println!(
                    "{}",
                    dim_text(&format!(
                        "using pinned Chromium {} at {}",
                        florui_conformance::pin::pin().version,
                        pinned.display()
                    ))
                );
                pinned
            } else {
                eprintln!("{}", failure("no Chromium binary configured"));
                eprintln!(
                    "run scripts/fetch-chromium.ps1 to fetch the pinned build, or pass --chromium <path> / set FLORUI_CHROMIUM, e.g.:"
                );
                eprintln!(
                    r#"  --chromium "C:\Program Files\Google\Chrome\Application\chrome.exe""#
                );
                eprintln!(
                    r#"  --chromium "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe""#
                );
                return ExitCode::FAILURE;
            }
        }
    };

    let fixture = match load_reference_fixture(&fixture_path) {
        Ok(fixture) => fixture,
        Err(err) => return fail(format!("could not load fixture: {err}")),
    };

    // Chrome resolves a relative --user-data-dir against its own executable
    // directory, not the caller's cwd. Inside a write-protected install
    // directory (e.g. Program Files) that silently fails and Chrome falls
    // back to the real default profile, which then refuses remote
    // debugging outright ("non-default data directory" required). An
    // absolute path avoids that entirely.
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        return fail(format!("could not create {}: {err}", out_dir.display()));
    }
    let out_dir = match out_dir.canonicalize() {
        Ok(path) => path,
        Err(err) => return fail(format!("could not resolve {}: {err}", out_dir.display())),
    };

    let fixture_out_dir = out_dir.join(&fixture.manifest.id);
    let profile_dir = out_dir.join(format!("chrome-profile-{}", std::process::id()));

    let driver = match ChromiumDriver::launch(ChromiumOptions {
        executable: chromium.clone(),
        user_data_dir: profile_dir.clone(),
        headless: !headed,
        launch_timeout: Duration::from_secs(30),
    }) {
        Ok(driver) => driver,
        Err(err) => return fail(format!("could not launch Chromium: {err}")),
    };

    let capture_result = driver.capture(&fixture);
    // Chrome holds the profile directory open until the process exits, so
    // the driver (and the browser it owns) must be dropped before any
    // attempt to remove that directory, or the removal silently fails.
    drop(driver);
    let capture = match capture_result {
        Ok(capture) => capture,
        Err(err) => {
            cleanup_profile(&profile_dir, keep_profile);
            return fail(format!("capture failed: {err}"));
        }
    };

    let canvas_color =
        match florui_devtools::color::parse_hex_color(&fixture.manifest.expected.canvas_color) {
            Ok(color) => color,
            Err(err) => return fail(format!("invalid canvas_color in fixture manifest: {err}")),
        };
    let element_color =
        match florui_devtools::color::parse_hex_color(&fixture.manifest.expected.element_color) {
            Ok(color) => color,
            Err(err) => return fail(format!("invalid element_color in fixture manifest: {err}")),
        };

    let viewport = fixture.manifest.viewport;
    let insets = fixture
        .manifest
        .expected
        .insets_css_px
        .to_physical(viewport.device_pixel_ratio);
    let engine_pixels = render_rgba8_inset(
        viewport.width_physical_px(),
        viewport.height_physical_px(),
        canvas_color,
        element_color,
        insets,
    );
    let Some(engine_image) = image::RgbaImage::from_raw(
        viewport.width_physical_px(),
        viewport.height_physical_px(),
        engine_pixels,
    ) else {
        cleanup_profile(&profile_dir, keep_profile);
        return fail("engine render did not match the declared viewport dimensions");
    };

    let pixel_report =
        match compare_pixels(&capture.image, &engine_image, &PixelDiffOptions::default()) {
            Ok(report) => report,
            Err(err) => {
                cleanup_profile(&profile_dir, keep_profile);
                return fail(err);
            }
        };

    let Some(engine_box) = element_box(
        viewport.width_physical_px(),
        viewport.height_physical_px(),
        insets,
    ) else {
        cleanup_profile(&profile_dir, keep_profile);
        return fail("fixture insets leave no room for an element box in the declared viewport");
    };
    let engine_box_css_px = BoxGeometryPx::from_physical(
        engine_box.x,
        engine_box.y,
        engine_box.width,
        engine_box.height,
        viewport.device_pixel_ratio,
    );
    let geometry_report = compare_geometry(capture.element_box_css_px, engine_box_css_px, 0.0);

    let outcome = classify(
        &fixture.manifest.classification,
        &pixel_report.summary,
        &geometry_report,
    );

    if let Err(err) = std::fs::create_dir_all(&fixture_out_dir) {
        cleanup_profile(&profile_dir, keep_profile);
        return fail(format!(
            "could not create {}: {err}",
            fixture_out_dir.display()
        ));
    }

    let reference_path = fixture_out_dir.join("reference.png");
    let result_path = fixture_out_dir.join("result.png");
    let diff_path = fixture_out_dir.join("diff.png");
    let overlay_path = fixture_out_dir.join("overlay.png");
    let overlay = overlay_images(&capture.image, &engine_image);

    for (image, path) in [
        (&capture.image, &reference_path),
        (&engine_image, &result_path),
        (&pixel_report.diff_image, &diff_path),
        (&overlay, &overlay_path),
    ] {
        if let Err(err) = image.save(path) {
            cleanup_profile(&profile_dir, keep_profile);
            return fail(format!("could not write {}: {err}", path.display()));
        }
    }

    let captured_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let report = Report {
        fixture_id: fixture.manifest.id.clone(),
        captured_at_unix_seconds,
        chromium_executable: chromium,
        pixels: pixel_report.summary,
        geometry: geometry_report,
        artifacts: ArtifactPaths {
            reference: reference_path,
            result: result_path,
            diff: diff_path,
            overlay: overlay_path,
        },
        outcome,
    };
    if let Err(err) = write_report(&report, &fixture_out_dir) {
        cleanup_profile(&profile_dir, keep_profile);
        return fail(err);
    }

    cleanup_profile(&profile_dir, keep_profile);

    match outcome {
        Outcome::Pass => {
            println!(
                "{}",
                success(&format!("✔ {} — exact match", fixture.manifest.id))
            );
            ExitCode::SUCCESS
        }
        Outcome::Fail => {
            println!(
                "{}",
                failure(&format!(
                    "✘ {} — {} differing pixels ({:.2}%), max geometry delta {:.1}px",
                    fixture.manifest.id,
                    pixel_report.summary.differing_pixels,
                    pixel_report.summary.percent_different,
                    geometry_report.max_axis_delta_px
                ))
            );
            println!(
                "{}",
                dim_text(&format!(
                    "artifacts written to {}",
                    fixture_out_dir.display()
                ))
            );
            ExitCode::FAILURE
        }
    }
}

fn not_implemented(command: &str) -> ExitCode {
    eprintln!("{command} is not implemented yet.");
    ExitCode::FAILURE
}

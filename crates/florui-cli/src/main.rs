use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde::Serialize;

use florui_conformance::driver::{ChromiumDriver, ChromiumOptions};
use florui_conformance::engine::render_fixture;
use florui_conformance::geometry::{GeometryReport, compare_geometry};
use florui_conformance::pixels::{
    PixelDiffOptions, PixelSummary, anaglyph_overlay, compare_pixels, overlay_images,
};
use florui_conformance::reference_fixture::load_reference_fixture;
use florui_conformance::report::{ArtifactPaths, Outcome, Report, classify, write_report};
use florui_devtools::diagnostics::{dim_text, failure, success};

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
    /// Run the Cargo test suite plus the visual/geometry reference fixtures.
    Test,
    /// Compare a reference fixture's Chromium capture against Florui's own render.
    Compare {
        /// Directory containing the fixture's manifest.json, HTML, and CSS.
        #[arg(long, default_value = "fixtures/reference/div-default")]
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
    /// Produce a release artifact for a supported target.
    Build {
        #[arg(long, default_value = "native")]
        target: String,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Dev { fixture } => run_dev(fixture),
        Command::New { name } => not_implemented(&format!("`florui new {name}`")),
        Command::Test => run_test(),
        Command::Compare {
            fixture,
            chromium,
            out_dir,
            headed,
            keep_profile,
        } => run_compare(fixture, chromium, out_dir, headed, keep_profile),
        Command::Build { target } => run_build(target),
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

/// Resolves a Chromium binary: the explicit path/env value if given, else
/// the pinned build fetched by `scripts/fetch-chromium.ps1`, if present.
/// Never falls back to guessing a system install — callers decide how to
/// report an unresolved binary, since a missing one means different things
/// to `compare` (a hard error) and `test` (a skipped suite).
fn resolve_chromium(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path);
    }
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
        Some(pinned)
    } else {
        None
    }
}

fn discover_reference_fixtures(fixtures_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(fixtures_root) else {
        return Vec::new();
    };
    let mut fixtures: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("manifest.json").is_file())
        .collect();
    fixtures.sort();
    fixtures
}

struct CompareResult {
    fixture_id: String,
    outcome: Outcome,
    pixels: PixelSummary,
    geometry: GeometryReport,
    artifacts_dir: PathBuf,
}

/// Runs one fixture's capture/compare/report cycle against an
/// already-launched `driver`, so a batch of fixtures (see `run_test`) can
/// share a single Chromium instance instead of relaunching it per fixture.
fn compare_fixture(
    driver: &ChromiumDriver,
    fixture_path: &Path,
    out_dir: &Path,
    chromium_executable: &Path,
) -> Result<CompareResult, String> {
    let fixture = load_reference_fixture(fixture_path)
        .map_err(|err| format!("could not load fixture: {err}"))?;

    let capture = driver
        .capture(&fixture)
        .map_err(|err| format!("capture failed: {err}"))?;

    let viewport = fixture.manifest.viewport;
    if viewport.device_pixel_ratio != 1.0 {
        return Err(format!(
            "fixture {:?} declares device_pixel_ratio {}, but florui_conformance::engine only \
             supports 1.0 so far",
            fixture.manifest.id, viewport.device_pixel_ratio
        ));
    }
    let engine = render_fixture(
        &fixture.manifest.florui,
        &fixture.manifest.canvas_color,
        viewport.width_css_px,
        viewport.height_css_px,
    )
    .map_err(|err| format!("engine render failed: {err}"))?;

    let pixel_report = compare_pixels(&capture.image, &engine.image, &PixelDiffOptions::default())
        .map_err(|err| err.to_string())?;
    let geometry_report =
        compare_geometry(capture.element_box_css_px, engine.element_box_css_px, 1.5);

    let outcome = classify(
        &fixture.manifest.classification,
        &pixel_report.summary,
        &geometry_report,
    );

    let fixture_out_dir = out_dir.join(&fixture.manifest.id);
    std::fs::create_dir_all(&fixture_out_dir)
        .map_err(|err| format!("could not create {}: {err}", fixture_out_dir.display()))?;

    let reference_path = fixture_out_dir.join("reference.png");
    let result_path = fixture_out_dir.join("result.png");
    let diff_path = fixture_out_dir.join("diff.png");
    let overlay_path = fixture_out_dir.join("overlay.png");
    let anaglyph_path = fixture_out_dir.join("florui_vs_chromium.png");
    let overlay = overlay_images(&capture.image, &engine.image);
    let anaglyph = anaglyph_overlay(&capture.image, &engine.image);

    for (image, path) in [
        (&capture.image, &reference_path),
        (&engine.image, &result_path),
        (&pixel_report.diff_image, &diff_path),
        (&overlay, &overlay_path),
        (&anaglyph, &anaglyph_path),
    ] {
        image
            .save(path)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    }

    let captured_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let report = Report {
        fixture_id: fixture.manifest.id.clone(),
        captured_at_unix_seconds,
        chromium_executable: chromium_executable.to_owned(),
        pixels: pixel_report.summary,
        geometry: geometry_report,
        artifacts: ArtifactPaths {
            reference: reference_path,
            result: result_path,
            diff: diff_path,
            overlay: overlay_path,
            anaglyph: anaglyph_path,
        },
        outcome,
    };
    write_report(&report, &fixture_out_dir).map_err(|err| err.to_string())?;

    Ok(CompareResult {
        fixture_id: fixture.manifest.id,
        outcome,
        pixels: pixel_report.summary,
        geometry: geometry_report,
        artifacts_dir: fixture_out_dir,
    })
}

fn print_compare_result(result: &CompareResult) {
    match result.outcome {
        Outcome::Pass => {
            println!(
                "{}",
                success(&format!("✔ {} — exact match", result.fixture_id))
            );
        }
        Outcome::Fail => {
            println!(
                "{}",
                failure(&format!(
                    "✘ {} — {} differing pixels ({:.2}%), max geometry delta {:.1}px",
                    result.fixture_id,
                    result.pixels.differing_pixels,
                    result.pixels.percent_different,
                    result.geometry.max_axis_delta_px
                ))
            );
            println!(
                "{}",
                dim_text(&format!(
                    "artifacts written to {}",
                    result.artifacts_dir.display()
                ))
            );
        }
    }
}

fn run_compare(
    fixture_path: PathBuf,
    chromium: Option<PathBuf>,
    out_dir: PathBuf,
    headed: bool,
    keep_profile: bool,
) -> ExitCode {
    let Some(chromium) = resolve_chromium(chromium) else {
        eprintln!("{}", failure("no Chromium binary configured"));
        eprintln!(
            "run scripts/fetch-chromium.ps1 to fetch the pinned build, or pass --chromium <path> / set FLORUI_CHROMIUM, e.g.:"
        );
        eprintln!(r#"  --chromium "C:\Program Files\Google\Chrome\Application\chrome.exe""#);
        eprintln!(r#"  --chromium "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe""#);
        return ExitCode::FAILURE;
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

    let result = compare_fixture(&driver, &fixture_path, &out_dir, &chromium);
    // Chrome holds the profile directory open until the process exits, so
    // the driver (and the browser it owns) must be dropped before any
    // attempt to remove that directory, or the removal silently fails.
    drop(driver);
    cleanup_profile(&profile_dir, keep_profile);

    match result {
        Ok(result) => {
            let exit = match result.outcome {
                Outcome::Pass => ExitCode::SUCCESS,
                Outcome::Fail => ExitCode::FAILURE,
            };
            print_compare_result(&result);
            exit
        }
        Err(err) => fail(err),
    }
}

fn run_test() -> ExitCode {
    println!("{}", dim_text("running cargo test --workspace..."));
    let cargo_ok = match std::process::Command::new("cargo")
        .args(["test", "--workspace"])
        .status()
    {
        Ok(status) => status.success(),
        Err(err) => {
            eprintln!("{}", failure(&format!("could not run cargo test: {err}")));
            false
        }
    };
    println!();
    if cargo_ok {
        println!("{}", success("✔ cargo test --workspace"));
    } else {
        println!("{}", failure("✘ cargo test --workspace"));
    }

    let fixtures = discover_reference_fixtures(Path::new("fixtures/reference"));
    if fixtures.is_empty() {
        return if cargo_ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    println!();
    let Some(chromium) = resolve_chromium(None) else {
        println!(
            "{}",
            dim_text(
                "skipping the visual/geometry suite: no Chromium found (run scripts/fetch-chromium.ps1, or pass --chromium/FLORUI_CHROMIUM to `florui compare` directly)"
            )
        );
        return if cargo_ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    };

    let out_dir = PathBuf::from("target/florui-conformance");
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        return fail(format!("could not create {}: {err}", out_dir.display()));
    }
    let out_dir = match out_dir.canonicalize() {
        Ok(path) => path,
        Err(err) => return fail(format!("could not resolve {}: {err}", out_dir.display())),
    };
    let profile_dir = out_dir.join(format!("chrome-profile-{}", std::process::id()));

    let driver = match ChromiumDriver::launch(ChromiumOptions {
        executable: chromium.clone(),
        user_data_dir: profile_dir.clone(),
        headless: true,
        launch_timeout: Duration::from_secs(30),
    }) {
        Ok(driver) => driver,
        Err(err) => return fail(format!("could not launch Chromium: {err}")),
    };

    let mut visual_ok = true;
    for fixture_path in &fixtures {
        match compare_fixture(&driver, fixture_path, &out_dir, &chromium) {
            Ok(result) => {
                if result.outcome == Outcome::Fail {
                    visual_ok = false;
                }
                print_compare_result(&result);
            }
            Err(err) => {
                println!(
                    "{}",
                    failure(&format!("✘ {}: {err}", fixture_path.display()))
                );
                visual_ok = false;
            }
        }
    }
    drop(driver);
    cleanup_profile(&profile_dir, false);

    if cargo_ok && visual_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[derive(Serialize)]
struct BuildReport {
    florui_target: String,
    profile: String,
    rustc_version: String,
    built_at_unix_seconds: u64,
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn run_build(target: String) -> ExitCode {
    if target != "native" {
        eprintln!(
            "{}",
            failure(&format!("target \"{target}\" is not supported yet"))
        );
        eprintln!(
            "only \"native\" is currently supported; the web target has its own separate milestone"
        );
        return ExitCode::FAILURE;
    }

    println!(
        "{}",
        dim_text("building native artifact (cargo build --release --workspace)...")
    );
    let status = match std::process::Command::new("cargo")
        .args(["build", "--release", "--workspace"])
        .status()
    {
        Ok(status) => status,
        Err(err) => return fail(format!("could not run cargo build: {err}")),
    };
    if !status.success() {
        return fail(format!("cargo build exited with {status}"));
    }

    let report = BuildReport {
        florui_target: target.clone(),
        profile: "release".to_owned(),
        rustc_version: rustc_version(),
        built_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let out_dir = PathBuf::from("target/florui-build");
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        return fail(format!("could not create {}: {err}", out_dir.display()));
    }
    let report_path = out_dir.join("report.json");
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(err) => return fail(format!("could not encode build report: {err}")),
    };
    if let Err(err) = std::fs::write(&report_path, json) {
        return fail(format!("could not write {}: {err}", report_path.display()));
    }

    println!(
        "{}",
        success(&format!("✔ built target \"{target}\" (release)"))
    );
    println!(
        "{}",
        dim_text(&format!("build report: {}", report_path.display()))
    );
    ExitCode::SUCCESS
}

fn not_implemented(command: &str) -> ExitCode {
    eprintln!("{command} is not implemented yet.");
    ExitCode::FAILURE
}

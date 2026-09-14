use std::path::{Path, PathBuf};
use std::process::{Command as ChildCommand, ExitCode, ExitStatus};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
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
    /// Resolves the current project via `cargo metadata` (works from any
    /// directory inside it, the same way `cargo build` does), builds and
    /// runs the cargo example declared in its `Cargo.toml`
    /// (`[package.metadata.florui.dev] example = "..."`), and restarts it
    /// whenever its Rust source changes — with an explicit report that its
    /// in-memory state was just reset, since a fresh process has none of
    /// the old one's. CSS reload for that running example is its own
    /// concern (see `florui_platform::run_with_css_reload`) — this only
    /// watches `.rs` files.
    ///
    /// `--fixture` instead runs the original, more primitive engine-only
    /// preview (one drawable rectangle driven directly by a CSS file, no
    /// real components) that this loop bootstrapped from — still useful
    /// for the conformance/geometry harness, but not a real project.
    Dev {
        /// Run the primitive CSS-fixture engine preview at this path
        /// instead of resolving and running the current project.
        #[arg(long, conflicts_with = "example")]
        fixture: Option<PathBuf>,
        /// Run this cargo example instead of the one declared in the
        /// resolved project's `[package.metadata.florui.dev]`.
        #[arg(long, conflicts_with = "fixture")]
        example: Option<String>,
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
        Command::Dev { fixture, example } => match fixture {
            Some(fixture) => run_dev(fixture),
            None => run_dev_example(example),
        },
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

/// `target/<debug>/examples/<example><EXE_SUFFIX>` under `target_dir` —
/// cargo's own, undocumented-but-stable convention for where `cargo build
/// --example <name>` places its binary.
fn example_exe_path(target_dir: &Path, example: &str) -> PathBuf {
    target_dir
        .join("debug")
        .join("examples")
        .join(format!("{example}{}", std::env::consts::EXE_SUFFIX))
}

/// What `florui dev` needs to know about the project it was invoked
/// inside, entirely from `cargo metadata` — never an assumed `./target` or
/// `.` for the package's own directory, since neither holds for a
/// workspace member invoked from one of its own subdirectories (cargo's
/// build output always lives at the *workspace* root, and the package
/// resolved for `dev` is wherever cargo itself says the current directory
/// belongs, not necessarily cwd itself).
#[derive(Debug)]
struct ResolvedProject {
    /// The resolved package's own directory (`Cargo.toml`'s parent) — what
    /// `dev` watches for `.rs` changes, regardless of which of its
    /// subdirectories it was actually invoked from.
    package_root: PathBuf,
    /// Where `cargo build` places its output; shared by every workspace
    /// member, so this is almost never `package_root`-relative.
    target_dir: PathBuf,
    /// From `[package.metadata.florui.dev] example = "..."` in the
    /// resolved package's own `Cargo.toml`, if declared there.
    default_example: Option<String>,
}

/// Asks cargo (via `cargo metadata`, not a hand-rolled directory walk) to
/// resolve which package the current directory belongs to — the same
/// resolution `cargo build`/`cargo run` already do from any of a package's
/// subdirectories — and pulls this project's dev configuration out of it.
fn resolve_project() -> Result<ResolvedProject, String> {
    let output = ChildCommand::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|err| format!("could not run cargo metadata: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("could not parse cargo metadata output: {err}"))?;
    parse_resolved_project(&metadata)
}

/// The pure, JSON-in half of [`resolve_project`] — separated out so it can
/// be tested against a hand-built payload without actually shelling out to
/// cargo.
fn parse_resolved_project(metadata: &serde_json::Value) -> Result<ResolvedProject, String> {
    let target_dir = metadata
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata output had no target_directory field".to_owned())?;

    let root_id = metadata
        .get("resolve")
        .and_then(|resolve| resolve.get("root"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            "could not determine which package to run `florui dev` for — invoke it from \
             inside a specific package's own directory, not the workspace root, or pass \
             --fixture / --example explicitly"
                .to_owned()
        })?;

    let package = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some(root_id))
        .ok_or_else(|| {
            "cargo metadata's resolved package was not in its own packages list".to_owned()
        })?;

    let manifest_path = package
        .get("manifest_path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "cargo metadata's package entry had no manifest_path".to_owned())?;
    let package_root = Path::new(manifest_path)
        .parent()
        .ok_or_else(|| format!("{manifest_path} has no parent directory"))?
        .to_owned();

    let default_example = package
        .get("metadata")
        .and_then(|metadata| metadata.get("florui"))
        .and_then(|florui| florui.get("dev"))
        .and_then(|dev| dev.get("example"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);

    Ok(ResolvedProject {
        package_root,
        target_dir,
        default_example,
    })
}

/// Runs `cargo build --example <example>`, inheriting stdio so cargo's own
/// compiler diagnostics reach the terminal directly rather than being
/// re-parsed and re-rendered here.
fn cargo_build_example(example: &str) -> std::io::Result<ExitStatus> {
    ChildCommand::new("cargo")
        .args(["build", "--example", example])
        .status()
}

/// Watches `package_root`'s `src` and `examples` directories (whichever
/// exist) for `.rs` file changes, sending on `tx` for each — never the
/// whole package root, since that would also see `cargo build`'s own
/// writes under `target/` and rebuild forever in response to its own
/// output. Returns every watcher created; a caller must keep them alive
/// for as long as it wants to keep watching.
fn watch_rust_sources(
    package_root: &Path,
    tx: mpsc::Sender<()>,
) -> notify::Result<Vec<RecommendedWatcher>> {
    let mut watchers = Vec::new();
    for subdir in ["src", "examples"] {
        let dir = package_root.join(subdir);
        if !dir.is_dir() {
            continue;
        }
        let tx = tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else { return };
                let touches_rust = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "rs"));
                if touches_rust {
                    // The loop may already have stopped reading; nothing to
                    // do if so.
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(&dir, RecursiveMode::Recursive)?;
        watchers.push(watcher);
    }
    Ok(watchers)
}

fn exit_code_from_status(status: ExitStatus) -> ExitCode {
    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Resolves the current project (see [`resolve_project`]) and builds and
/// runs its declared example — `example_override`, if given, takes
/// precedence over `[package.metadata.florui.dev]` the same way `cargo run
/// --example` overrides a crate's own `default-run` — restarting it with
/// an explicit report that its in-memory state was just reset, since a
/// fresh process has none of the old one's, whenever a `.rs` file under
/// the resolved package's `src`/`examples` changes. A build that fails
/// leaves whichever version last built successfully running untouched, the
/// same "stale but working, plus an actionable diagnostic" recovery
/// contract the CSS-fixture preview already established; cargo's own
/// compiler output is that diagnostic here, printed directly rather than
/// re-parsed.
///
/// Exits once the running example's own window closes on its own (not as
/// a result of a restart this loop performed), returning its exit code.
fn run_dev_example(example_override: Option<String>) -> ExitCode {
    let project = match resolve_project() {
        Ok(project) => project,
        Err(err) => return fail(err),
    };
    let example = match example_override.or(project.default_example) {
        Some(example) => example,
        None => {
            return fail(
                "no dev entry point configured for this package — add\n\n    \
                 [package.metadata.florui.dev]\n    example = \"<name>\"\n\n\
                 to its Cargo.toml, or pass --example <name> explicitly",
            );
        }
    };
    let package_root = project.package_root;
    let exe_path = example_exe_path(&project.target_dir, &example);

    println!(
        "{}",
        dim_text(&format!("building example \"{example}\"..."))
    );
    match cargo_build_example(&example) {
        Ok(status) if status.success() => {}
        Ok(status) => return fail(format!("cargo build exited with {status}")),
        Err(err) => return fail(format!("could not run cargo build: {err}")),
    }

    // `None` means "no window is currently running" — true after a rebuild
    // whose linker step failed (the old process was already killed to free
    // the .exe for that attempt), not just before the very first spawn.
    // The timeout arm below must only treat a dead child as "the user
    // closed it" when this is `Some`; otherwise it would mistake "still
    // waiting for the developer to fix a build error" for the window
    // having closed on its own and exit the whole loop.
    let mut child = match ChildCommand::new(&exe_path).spawn() {
        Ok(child) => Some(child),
        Err(err) => {
            return fail(format!(
                "could not run built example at {}: {err}",
                exe_path.display()
            ));
        }
    };
    println!("{}", success(&format!("✔ running \"{example}\"")));

    let (tx, rx) = mpsc::channel();
    let _watchers = match watch_rust_sources(&package_root, tx) {
        Ok(watchers) => watchers,
        Err(err) => return fail(format!("could not watch Rust sources: {err}")),
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) => {
                // A single save can fire several filesystem events (and an
                // editor writing multiple files at once fires more); drain
                // whatever else arrives in a short window so one save
                // triggers exactly one rebuild, not a burst of them.
                let debounce_until = std::time::Instant::now() + Duration::from_millis(150);
                while let Some(remaining) = debounce_until
                    .checked_duration_since(std::time::Instant::now())
                    .filter(|remaining| !remaining.is_zero())
                {
                    if rx.recv_timeout(remaining).is_err() {
                        break;
                    }
                }

                // The running .exe is locked on Windows — linking a new one
                // over it fails outright while the old process still holds
                // it open, so the old process must die *before* the build
                // is even attempted, not after it succeeds. That also means
                // a failed rebuild here cannot leave a stale-but-working
                // window the way a CSS-only reload failure can: there is
                // no window at all until the next successful build. `child`
                // stays `None` for the rest of this iteration until (and
                // unless) a rebuild actually succeeds below.
                if let Some(mut old) = child.take() {
                    let _ = old.kill();
                    let _ = old.wait();
                }
                println!(
                    "{}",
                    dim_text(&format!(
                        "Rust source changed — rebuilding \"{example}\" (window closed; \
                         in-memory UI state was reset)..."
                    ))
                );
                match cargo_build_example(&example) {
                    Ok(status) if status.success() => match ChildCommand::new(&exe_path).spawn() {
                        Ok(new_child) => {
                            child = Some(new_child);
                            println!(
                                "{}",
                                success(&format!("✔ rebuilt and restarted \"{example}\""))
                            );
                        }
                        Err(err) => {
                            return fail(format!(
                                "rebuild succeeded but could not restart {}: {err}",
                                exe_path.display()
                            ));
                        }
                    },
                    Ok(status) => {
                        println!(
                            "{}",
                            failure(&format!("✘ build failed (cargo exited with {status})"))
                        );
                        println!(
                            "{}",
                            dim_text(
                                "no window is open — fix the error above and save again to retry"
                            )
                        );
                    }
                    Err(err) => {
                        println!(
                            "{}",
                            failure(&format!("✘ could not run cargo build: {err}"))
                        );
                        println!(
                            "{}",
                            dim_text(
                                "no window is open — fix the error above and save again to retry"
                            )
                        );
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(current) = &mut child
                    && let Ok(Some(status)) = current.try_wait()
                {
                    return exit_code_from_status(status);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                eprintln!(
                    "{}",
                    failure("the source watcher stopped unexpectedly; exiting")
                );
                if let Some(mut current) = child.take() {
                    let _ = current.kill();
                }
                return ExitCode::FAILURE;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_exe_path_joins_target_debug_examples_and_the_platform_exe_suffix() {
        let path = example_exe_path(Path::new("target"), "counter");
        assert_eq!(
            path,
            Path::new("target")
                .join("debug")
                .join("examples")
                .join(format!("counter{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn example_exe_path_respects_a_non_default_target_dir() {
        let path = example_exe_path(Path::new("/custom/target"), "counter");
        assert_eq!(
            path,
            Path::new("/custom/target")
                .join("debug")
                .join("examples")
                .join(format!("counter{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn exit_code_from_status_maps_a_successful_child_to_success() {
        // A trivial command that always succeeds, portable across the
        // platforms `std::process` supports.
        let status = ChildCommand::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit 0"][..]
            } else {
                &[][..]
            })
            .status()
            .expect("a trivial command should always be spawnable");
        assert_eq!(exit_code_from_status(status), ExitCode::SUCCESS);
    }

    #[test]
    fn exit_code_from_status_maps_a_failing_child_to_failure() {
        let status = ChildCommand::new(if cfg!(windows) { "cmd" } else { "false" })
            .args(if cfg!(windows) {
                &["/C", "exit 1"][..]
            } else {
                &[][..]
            })
            .status()
            .expect("a trivial command should always be spawnable");
        assert_eq!(exit_code_from_status(status), ExitCode::FAILURE);
    }

    /// A `cargo metadata` payload shaped like the real thing, but reduced
    /// to only the fields `parse_resolved_project` reads — resolving to
    /// `root_id`, whose own `metadata.florui.dev.example`, if given, is
    /// `dev_example`.
    fn metadata_with(root_id: &str, dev_example: Option<&str>) -> serde_json::Value {
        let metadata = match dev_example {
            Some(example) => serde_json::json!({"florui": {"dev": {"example": example}}}),
            None => serde_json::json!({}),
        };
        serde_json::json!({
            "target_directory": "/workspace/target",
            "resolve": {"root": root_id},
            "packages": [
                {
                    "id": "path+file:///workspace/other#0.1.0",
                    "manifest_path": "/workspace/other/Cargo.toml",
                    "metadata": null,
                },
                {
                    "id": root_id,
                    "manifest_path": "/workspace/app/Cargo.toml",
                    "metadata": metadata,
                },
            ],
        })
    }

    #[test]
    fn parse_resolved_project_reads_the_root_packages_own_dev_example() {
        let metadata = metadata_with("path+file:///workspace/app#0.1.0", Some("counter"));
        let project = parse_resolved_project(&metadata).expect("this payload should resolve");
        assert_eq!(project.package_root, Path::new("/workspace/app"));
        assert_eq!(project.target_dir, Path::new("/workspace/target"));
        assert_eq!(project.default_example.as_deref(), Some("counter"));
    }

    #[test]
    fn parse_resolved_project_leaves_default_example_none_when_undeclared() {
        let metadata = metadata_with("path+file:///workspace/app#0.1.0", None);
        let project = parse_resolved_project(&metadata).expect("this payload should resolve");
        assert_eq!(
            project.default_example, None,
            "an undeclared [package.metadata.florui.dev] must not be treated as an error — \
             only as \"no default,\" letting --example still override it"
        );
    }

    #[test]
    fn parse_resolved_project_fails_with_no_resolve_root() {
        // What a virtual workspace root (no [package] of its own) actually
        // produces — invoking `florui dev` there must fail with an
        // actionable message, not panic or silently pick some package.
        let metadata = serde_json::json!({
            "target_directory": "/workspace/target",
            "resolve": {"root": null},
            "packages": [],
        });
        let error = parse_resolved_project(&metadata).unwrap_err();
        assert!(
            error.contains("--fixture") && error.contains("--example"),
            "the error should point at both escape hatches, got: {error}"
        );
    }
}

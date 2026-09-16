//! `florui doctor`: real, observed evidence about the local environment
//! and (when invoked inside one) the current project — never a readiness
//! claim for a capability nothing here actually probed. Every check
//! carries a stable id, so a script or CI job can key off one specific
//! result instead of parsing human prose.
//!
//! Graphics and window-presentation probing (`--graphics`/`--presentation`)
//! run in a separate, bounded child process (this same `florui` binary,
//! re-invoked with a hidden internal flag) rather than in this one: a
//! crashed or hung graphics driver then only takes down that child, and a
//! probe that never responds is killed after a fixed timeout instead of
//! hanging the whole report. `--presentation` implies `--graphics` (it
//! cannot mean anything without a real adapter first) and, on success,
//! opens one clearly-titled temporary window just long enough to ask
//! `florui_platform::gpu::GpuPresenter` what it actually negotiated
//! before closing it again — this reports which presentation path came
//! up (and its real surface format/alpha mode), not a claim that a human
//! has visually confirmed the result; visual confirmation is a stronger,
//! separate kind of evidence this command does not produce.

use std::env;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command as ChildCommand, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const SCHEMA_VERSION: u32 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The hidden flag this binary re-invokes itself with to run each bounded
/// probe as a real child process — see this module's own doc.
pub const INTERNAL_GRAPHICS_PROBE_FLAG: &str = "--internal-graphics-probe";
pub const INTERNAL_PRESENTATION_PROBE_FLAG: &str = "--internal-presentation-probe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Warning,
    Fail,
    Unknown,
    Skipped,
    NotApplicable,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warning => "warning",
            Status::Fail => "fail",
            Status::Unknown => "unknown",
            Status::Skipped => "skipped",
            Status::NotApplicable => "not_applicable",
        }
    }

    /// Whether this status counts as "applicable" for strict mode's own
    /// extended failure condition — a check that never ran for a real
    /// reason (skipped, not applicable) is not the same as one that ran
    /// and came back uncertain.
    fn is_applicable(self) -> bool {
        !matches!(self, Status::Skipped | Status::NotApplicable)
    }
}

struct Check {
    id: &'static str,
    category: &'static str,
    status: Status,
    required: bool,
    observed: Option<String>,
    expected: Option<String>,
    evidence: String,
    reason: Option<String>,
    remediation: Option<String>,
}

impl Check {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "category": self.category,
            "status": self.status.as_str(),
            "required": self.required,
            "observed": self.observed,
            "expected": self.expected,
            "evidence": self.evidence,
            "reason": self.reason,
            "remediation": self.remediation,
        })
    }
}

pub struct Options {
    pub target: String,
    pub graphics: bool,
    pub presentation: bool,
    pub distribution: bool,
    pub json: bool,
    pub strict: bool,
}

pub fn run(options: Options) -> ExitCode {
    let started = Instant::now();
    let graphics = options.graphics || options.presentation;
    let mut checks = Vec::new();

    checks.extend(environment_checks());
    checks.extend(project_checks());

    match options.target.as_str() {
        "native" => {
            if graphics {
                let probe = run_bounded_probe(INTERNAL_GRAPHICS_PROBE_FLAG);
                checks.push(graphics_check(&probe));
                if options.presentation {
                    checks.push(presentation_check(
                        &probe,
                        &run_bounded_probe(INTERNAL_PRESENTATION_PROBE_FLAG),
                    ));
                }
            }
            if options.distribution {
                checks.push(Check {
                    id: "distribution.packaging",
                    category: "distribution",
                    status: Status::Unknown,
                    required: false,
                    observed: None,
                    expected: None,
                    evidence: "no packaging/signing pipeline exists in this CLI yet".to_string(),
                    reason: Some(
                        "distribution diagnostics are not implemented yet, not a property of \
                         this environment"
                            .to_string(),
                    ),
                    remediation: None,
                });
            }
        }
        "web" => {
            checks.push(Check {
                id: "target.web",
                category: "target",
                status: Status::Unknown,
                required: false,
                observed: None,
                expected: None,
                evidence: "the Web target has no build pipeline in this CLI yet".to_string(),
                reason: Some("the Web target is not implemented yet".to_string()),
                remediation: None,
            });
            if graphics {
                checks.push(Check {
                    id: "graphics.unsupported_target",
                    category: "graphics",
                    status: Status::NotApplicable,
                    required: false,
                    observed: None,
                    expected: None,
                    evidence: "graphics/presentation probing is only implemented for --target \
                                native"
                        .to_string(),
                    reason: None,
                    remediation: None,
                });
            }
            if options.distribution {
                // A real request this build genuinely cannot honor yet for
                // this target, distinct from `NotApplicable` (which means
                // the check itself makes no sense here) -- distribution
                // diagnostics do make sense for a Web target eventually,
                // this CLI just doesn't implement them yet.
                checks.push(Check {
                    id: "distribution.unsupported_target",
                    category: "distribution",
                    status: Status::Skipped,
                    required: false,
                    observed: None,
                    expected: None,
                    evidence: "distribution diagnostics are only implemented for --target native"
                        .to_string(),
                    reason: None,
                    remediation: None,
                });
            }
        }
        other => {
            checks.push(Check {
                id: "target.unknown",
                category: "target",
                status: Status::Fail,
                required: true,
                observed: Some(other.to_string()),
                expected: Some("native or web".to_string()),
                evidence: format!("--target {other} is not a recognized target selector"),
                reason: None,
                remediation: Some("pass --target native or --target web".to_string()),
            });
        }
    }

    report(
        checks,
        options.target,
        started,
        options.json,
        options.strict,
    )
}

fn environment_checks() -> Vec<Check> {
    vec![
        tool_version_check("rust.cargo", "cargo", true),
        tool_version_check("rust.rustc", "rustc", true),
        tool_version_check("rust.rustfmt", "rustfmt", false),
    ]
}

fn tool_version_check(id: &'static str, program: &str, required: bool) -> Check {
    match ChildCommand::new(program).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Check {
                id,
                category: "toolchain",
                status: Status::Pass,
                required,
                observed: Some(version.clone()),
                expected: None,
                evidence: format!("`{program} --version` succeeded: {version}"),
                reason: None,
                remediation: None,
            }
        }
        Ok(output) => Check {
            id,
            category: "toolchain",
            status: if required {
                Status::Fail
            } else {
                Status::Warning
            },
            required,
            observed: None,
            expected: None,
            evidence: format!(
                "`{program} --version` exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            reason: None,
            remediation: Some(format!("install or repair {program}")),
        },
        Err(error) => Check {
            id,
            category: "toolchain",
            status: if required {
                Status::Fail
            } else {
                Status::Warning
            },
            required,
            observed: None,
            expected: None,
            evidence: format!("could not run `{program}`: {error}"),
            reason: None,
            remediation: Some(format!("install {program} and ensure it is on PATH")),
        },
    }
}

/// Real `cargo metadata`-backed project facts — deliberately separate
/// from `main.rs`'s own `resolve_project`, which hard-errors outside a
/// resolvable project; a doctor run outside one should still report
/// environment checks and mark project checks not applicable, not fail
/// outright.
#[derive(Debug, PartialEq)]
struct ProjectFacts {
    workspace_root: PathBuf,
    manifest_path: PathBuf,
    declared_example: Option<String>,
    example_targets: Vec<String>,
}

fn resolve_project_facts() -> Option<ProjectFacts> {
    // No `--no-deps`: that flag leaves `resolve` (and so `resolve.root`,
    // the field this function actually needs) entirely null. Reading an
    // existing `Cargo.lock` this way is still offline and never writes
    // anything -- `cargo metadata` alone never modifies the lockfile.
    let output = ChildCommand::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).ok()?;
    parse_project_facts(&metadata)
}

/// The pure, JSON-in half of [`resolve_project_facts`] — separated out so
/// it can be tested against a hand-built payload without actually
/// shelling out to cargo, mirroring `main.rs`'s own
/// `resolve_project`/`parse_resolved_project` split.
fn parse_project_facts(metadata: &Value) -> Option<ProjectFacts> {
    let workspace_root: PathBuf = metadata.get("workspace_root")?.as_str()?.into();
    // A `null` root (outside any specific package's own directory, the
    // same case `main.rs`'s own `resolve_project` treats as
    // unresolvable) must bail the whole function via `?`, not fall
    // through to default/empty facts about a project that was never
    // actually resolved.
    let root_id = metadata.get("resolve")?.get("root")?.as_str()?;
    let package = metadata
        .get("packages")?
        .as_array()?
        .iter()
        .find(|package| package.get("id").and_then(Value::as_str) == Some(root_id))?;

    let manifest_path = package
        .get("manifest_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("Cargo.toml"));

    let declared_example = package
        .get("metadata")
        .and_then(|metadata| metadata.get("florui"))
        .and_then(|florui| florui.get("dev"))
        .and_then(|dev| dev.get("example"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let example_targets = package
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|target| {
            target
                .get("kind")
                .and_then(Value::as_array)
                .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("example")))
        })
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();

    Some(ProjectFacts {
        workspace_root,
        manifest_path,
        declared_example,
        example_targets,
    })
}

fn project_checks() -> Vec<Check> {
    let Some(facts) = resolve_project_facts() else {
        return vec![Check {
            id: "project.resolved",
            category: "project",
            status: Status::NotApplicable,
            required: false,
            observed: None,
            expected: None,
            evidence: "not invoked inside a resolvable Cargo project".to_string(),
            reason: None,
            remediation: None,
        }];
    };

    let mut checks = vec![Check {
        id: "project.manifest_readable",
        category: "project",
        status: Status::Pass,
        required: false,
        observed: Some(display_redacted(&facts.manifest_path)),
        expected: None,
        evidence: "cargo metadata resolved this project's own Cargo.toml".to_string(),
        reason: None,
        remediation: None,
    }];

    let lockfile = facts.workspace_root.join("Cargo.lock");
    checks.push(if lockfile.is_file() {
        Check {
            id: "project.lockfile_readable",
            category: "project",
            status: Status::Pass,
            required: false,
            observed: Some(display_redacted(&lockfile)),
            expected: None,
            evidence: "Cargo.lock exists at the workspace root".to_string(),
            reason: None,
            remediation: None,
        }
    } else {
        Check {
            id: "project.lockfile_readable",
            category: "project",
            status: Status::Warning,
            required: false,
            observed: None,
            expected: Some(display_redacted(&lockfile)),
            evidence: "no Cargo.lock at the workspace root".to_string(),
            reason: None,
            remediation: Some(
                "run `cargo metadata` or `cargo build` once to generate it".to_string(),
            ),
        }
    });

    checks.push(match &facts.declared_example {
        None => Check {
            id: "project.dev_example_target_exists",
            category: "project",
            status: Status::NotApplicable,
            required: false,
            observed: None,
            expected: None,
            evidence: "no [package.metadata.florui.dev] example declared".to_string(),
            reason: None,
            remediation: None,
        },
        Some(example) if facts.example_targets.iter().any(|name| name == example) => Check {
            id: "project.dev_example_target_exists",
            category: "project",
            status: Status::Pass,
            required: false,
            observed: Some(example.clone()),
            expected: None,
            evidence: format!("cargo example target `{example}` exists"),
            reason: None,
            remediation: None,
        },
        Some(example) => Check {
            id: "project.dev_example_target_exists",
            category: "project",
            status: Status::Fail,
            required: false,
            observed: Some(example.clone()),
            expected: Some("a matching [[example]] target".to_string()),
            evidence: format!(
                "[package.metadata.florui.dev] declares example \"{example}\", but no \
                 matching [[example]] target exists"
            ),
            reason: None,
            remediation: Some(format!(
                "add an examples/{example}.rs (or fix the declared name)"
            )),
        },
    });

    checks
}

/// Replaces the user's home directory prefix with `~` — this crate's own
/// small share of the redaction contract every check's evidence and
/// observed/expected fields follow, so a doctor report is safe to paste
/// into an issue without hand-editing paths first.
fn display_redacted(path: &Path) -> String {
    let rendered = path.display().to_string();
    match dirs_home() {
        Some(home) if rendered.starts_with(&home) => {
            format!("~{}", &rendered[home.len()..])
        }
        _ => rendered,
    }
}

fn dirs_home() -> Option<String> {
    env::var("USERPROFILE").or_else(|_| env::var("HOME")).ok()
}

/// Runs this same binary again with `flag`, bounded to [`PROBE_TIMEOUT`] —
/// killed and reported as timed out rather than left to hang the whole
/// report. Returns the probe's own parsed JSON on a clean, successful
/// exit; `Err` carries a human-readable reason otherwise (missing
/// adapter, probe panic, timeout), which the caller turns into real
/// check evidence rather than a raw process error.
fn run_bounded_probe(flag: &str) -> Result<Value, String> {
    let exe =
        env::current_exe().map_err(|error| format!("could not find own executable: {error}"))?;
    let mut child = ChildCommand::new(exe)
        .arg(flag)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not spawn probe process: {error}"))?;

    let (tx, rx) = mpsc::channel();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    thread::spawn(move || {
        use std::io::Read;
        let mut out = String::new();
        let mut err = String::new();
        if let Some(stdout) = stdout.as_mut() {
            let _ = stdout.read_to_string(&mut out);
        }
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut err);
        }
        let _ = tx.send((out, err));
    });

    let status = match child.try_wait() {
        Ok(Some(status)) => Some(status),
        Ok(None) => wait_bounded(&mut child, PROBE_TIMEOUT),
        Err(error) => return Err(format!("could not check probe process status: {error}")),
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "probe process did not finish within {PROBE_TIMEOUT:?} and was killed"
        ));
    };

    let (stdout, stderr) = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    if !status.success() {
        return Err(format!(
            "probe process exited with {status}: {}",
            stderr.trim()
        ));
    }
    serde_json::from_str(stdout.trim())
        .map_err(|error| format!("probe process produced unparsable output: {error}"))
}

fn wait_bounded(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

fn graphics_check(probe: &Result<Value, String>) -> Check {
    match probe {
        Ok(evidence) => Check {
            id: "graphics.adapter",
            category: "graphics",
            status: Status::Pass,
            required: false,
            observed: evidence
                .get("adapter_name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            expected: None,
            evidence: format!(
                "backend={} adapter={} device_type={} driver={} driver_info={}",
                evidence
                    .get("backend")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                evidence
                    .get("adapter_name")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                evidence
                    .get("device_type")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                evidence
                    .get("driver")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                evidence
                    .get("driver_info")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
            ),
            reason: None,
            remediation: None,
        },
        Err(error) => Check {
            id: "graphics.adapter",
            category: "graphics",
            status: Status::Fail,
            required: false,
            observed: None,
            expected: None,
            evidence: error.clone(),
            reason: Some(
                "no usable GPU adapter came up -- florui_platform's own softbuffer CPU path \
                 remains the fallback"
                    .to_string(),
            ),
            remediation: Some("install or update graphics drivers".to_string()),
        },
    }
}

fn presentation_check(
    graphics_probe: &Result<Value, String>,
    probe: &Result<Value, String>,
) -> Check {
    match probe {
        Ok(evidence) => {
            let capability = evidence
                .get("capability")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let format = evidence
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let alpha_mode = evidence
                .get("alpha_mode")
                .and_then(Value::as_str)
                .unwrap_or("?");
            // A real adapter existing but no presenter coming up for a real
            // window is worth flagging even though it is not, on its own,
            // an environment failure -- see this module's own doc.
            let status = if capability == "cpu" && graphics_probe.is_ok() {
                Status::Warning
            } else {
                Status::Pass
            };
            let reason = (capability == "cpu" && graphics_probe.is_ok()).then(|| {
                "a real graphics adapter exists, but no GPU presenter could be created for a \
                 real window; presentation falls back to the CPU/softbuffer path"
                    .to_string()
            });
            Check {
                id: "presentation.capability",
                category: "presentation",
                status,
                required: false,
                observed: Some(capability.to_string()),
                expected: None,
                evidence: format!(
                    "capability={capability} format={format} alpha_mode={alpha_mode} -- a \
                     successful negotiation, not a human-verified visual confirmation (see \
                     this command's own doc)"
                ),
                reason,
                remediation: None,
            }
        }
        Err(error) => Check {
            id: "presentation.capability",
            category: "presentation",
            status: Status::Unknown,
            required: false,
            observed: None,
            expected: None,
            evidence: error.clone(),
            reason: Some("could not evaluate real window presentation".to_string()),
            remediation: None,
        },
    }
}

/// Whether `checks` should make `florui doctor` exit non-zero: a required
/// check failed or came back unverified, or (only under `strict`) any
/// applicable check came back as a warning or unknown result — pulled
/// out as its own pure function so this decision is directly testable
/// against a hand-built check list, not just observable through a real
/// process's exit code.
fn should_fail(checks: &[Check], strict: bool) -> bool {
    let required_failed = checks
        .iter()
        .any(|check| check.required && check.status == Status::Fail);
    let required_unverified = checks
        .iter()
        .any(|check| check.required && check.status == Status::Unknown);
    let strict_triggered = strict
        && checks.iter().any(|check| {
            check.status.is_applicable()
                && matches!(check.status, Status::Warning | Status::Unknown)
        });
    required_failed || required_unverified || strict_triggered
}

fn report(
    checks: Vec<Check>,
    target: String,
    started: Instant,
    json: bool,
    strict: bool,
) -> ExitCode {
    let failed = should_fail(&checks, strict);

    if json {
        let generated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let document = json!({
            "schema_version": SCHEMA_VERSION,
            "florui_version": env!("CARGO_PKG_VERSION"),
            "target": target,
            "generated_at_unix": generated_at,
            "duration_ms": started.elapsed().as_millis(),
            "checks": checks.iter().map(Check::to_json).collect::<Vec<_>>(),
        });
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "{document}");
    } else {
        print_human(&checks);
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_human(checks: &[Check]) {
    for check in checks {
        let marker = match check.status {
            Status::Pass => florui_devtools::diagnostics::success("PASS"),
            Status::Warning => florui_devtools::diagnostics::warning("WARN"),
            Status::Fail => florui_devtools::diagnostics::failure("FAIL"),
            Status::Unknown => florui_devtools::diagnostics::dim_text("UNKNOWN"),
            Status::Skipped => florui_devtools::diagnostics::dim_text("SKIPPED"),
            Status::NotApplicable => florui_devtools::diagnostics::dim_text("N/A"),
        };
        println!("{marker} {} -- {}", check.id, check.evidence);
        if let Some(reason) = &check.reason {
            println!("       {}", florui_devtools::diagnostics::dim_text(reason));
        }
        if let Some(remediation) = &check.remediation {
            println!(
                "       {} {remediation}",
                florui_devtools::diagnostics::dim_text("fix:")
            );
        }
    }
}

/// Runs inside the child process spawned by [`run_bounded_probe`] for
/// [`INTERNAL_GRAPHICS_PROBE_FLAG`] -- prints one line of JSON to stdout
/// and returns the process exit code; never called from a normal
/// `florui doctor` invocation itself.
pub fn run_internal_graphics_probe() -> ExitCode {
    match florui_platform::gpu::probe_graphics() {
        Ok(probe) => {
            let document = json!({
                "backend": probe.backend,
                "adapter_name": probe.adapter_name,
                "device_type": probe.device_type,
                "driver": probe.driver,
                "driver_info": probe.driver_info,
            });
            println!("{document}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs inside the child process spawned by [`run_bounded_probe`] for
/// [`INTERNAL_PRESENTATION_PROBE_FLAG`] -- opens one real, clearly
/// titled temporary window, asks a real [`florui_platform::gpu::GpuPresenter`]
/// what it negotiated, prints one line of JSON, and exits. Never called
/// from a normal `florui doctor` invocation itself.
pub fn run_internal_presentation_probe() -> ExitCode {
    use std::sync::Arc;

    use winit::application::ApplicationHandler;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::window::{Window, WindowId};

    struct Probe {
        result: Option<Result<String, String>>,
    }

    impl ApplicationHandler for Probe {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let attrs = florui_platform::gpu::transparent_capable_attributes(
                Window::default_attributes().with_title("florui doctor -- presentation probe"),
            );
            let outcome = match event_loop.create_window(attrs) {
                Ok(window) => {
                    let window = Arc::new(window);
                    match florui_platform::gpu::GpuPresenter::try_new(window) {
                        Some(presenter) => Ok(json!({
                            "capability": match presenter.capability() {
                                florui_platform::gpu::PresentationCapability::GpuTransparent => "gpu_transparent",
                                florui_platform::gpu::PresentationCapability::GpuOpaque => "gpu_opaque",
                                florui_platform::gpu::PresentationCapability::Cpu => "cpu",
                            },
                            "format": format!("{:?}", presenter.format()),
                            "alpha_mode": format!("{:?}", presenter.alpha_mode()),
                        })
                        .to_string()),
                        None => Ok(json!({
                            "capability": "cpu",
                            "format": null,
                            "alpha_mode": null,
                        })
                        .to_string()),
                    }
                }
                Err(error) => Err(format!("could not create a probe window: {error}")),
            };
            self.result = Some(outcome);
            event_loop.exit();
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            _event: winit::event::WindowEvent,
        ) {
        }
    }

    let Ok(event_loop) = EventLoop::new() else {
        eprintln!("could not create an event loop for the presentation probe");
        return ExitCode::FAILURE;
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut probe = Probe { result: None };
    if event_loop.run_app(&mut probe).is_err() {
        eprintln!("presentation probe event loop failed");
        return ExitCode::FAILURE;
    }

    match probe.result {
        Some(Ok(json)) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Some(Err(error)) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("presentation probe exited without producing a result");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &'static str, status: Status, required: bool) -> Check {
        Check {
            id,
            category: "test",
            status,
            required,
            observed: None,
            expected: None,
            evidence: String::new(),
            reason: None,
            remediation: None,
        }
    }

    #[test]
    fn skipped_and_not_applicable_are_not_applicable() {
        assert!(!Status::Skipped.is_applicable());
        assert!(!Status::NotApplicable.is_applicable());
    }

    #[test]
    fn pass_warning_fail_and_unknown_are_applicable() {
        assert!(Status::Pass.is_applicable());
        assert!(Status::Warning.is_applicable());
        assert!(Status::Fail.is_applicable());
        assert!(Status::Unknown.is_applicable());
    }

    #[test]
    fn a_required_failure_fails_even_without_strict() {
        let checks = vec![check("a", Status::Fail, true)];
        assert!(should_fail(&checks, false));
    }

    #[test]
    fn a_required_unknown_result_fails_even_without_strict() {
        let checks = vec![check("a", Status::Unknown, true)];
        assert!(
            should_fail(&checks, false),
            "an unverified required check must not be silently treated as passing"
        );
    }

    #[test]
    fn an_optional_failure_does_not_fail_without_strict() {
        let checks = vec![check("a", Status::Fail, false)];
        assert!(!should_fail(&checks, false));
    }

    #[test]
    fn strict_fails_on_an_optional_warning() {
        let checks = vec![check("a", Status::Warning, false)];
        assert!(!should_fail(&checks, false));
        assert!(should_fail(&checks, true));
    }

    #[test]
    fn strict_does_not_fail_on_a_not_applicable_or_skipped_result() {
        let checks = vec![
            check("a", Status::NotApplicable, false),
            check("b", Status::Skipped, false),
        ];
        assert!(
            !should_fail(&checks, true),
            "strict extends the failure condition to applicable results only"
        );
    }

    #[test]
    fn an_all_pass_report_never_fails() {
        let checks = vec![
            check("a", Status::Pass, true),
            check("b", Status::Pass, false),
        ];
        assert!(!should_fail(&checks, false));
        assert!(!should_fail(&checks, true));
    }

    fn metadata_with(root_id: &str, dev_example: Option<&str>, example_targets: &[&str]) -> Value {
        let metadata = match dev_example {
            Some(example) => json!({"florui": {"dev": {"example": example}}}),
            None => json!({}),
        };
        let targets: Vec<Value> = example_targets
            .iter()
            .map(|name| json!({"name": name, "kind": ["example"]}))
            .chain(std::iter::once(json!({"name": "main", "kind": ["bin"]})))
            .collect();
        json!({
            "workspace_root": "/workspace",
            "resolve": {"root": root_id},
            "packages": [
                {
                    "id": root_id,
                    "manifest_path": "/workspace/app/Cargo.toml",
                    "metadata": metadata,
                    "targets": targets,
                },
            ],
        })
    }

    #[test]
    fn parse_project_facts_reads_the_declared_example_and_confirms_it_exists() {
        let metadata = metadata_with("app#0.1.0", Some("counter"), &["counter"]);
        let facts = parse_project_facts(&metadata).expect("this payload should resolve");
        assert_eq!(facts.workspace_root, Path::new("/workspace"));
        assert_eq!(facts.manifest_path, Path::new("/workspace/app/Cargo.toml"));
        assert_eq!(facts.declared_example.as_deref(), Some("counter"));
        assert!(facts.example_targets.iter().any(|name| name == "counter"));
    }

    #[test]
    fn parse_project_facts_leaves_declared_example_none_when_undeclared() {
        let metadata = metadata_with("app#0.1.0", None, &[]);
        let facts = parse_project_facts(&metadata).expect("this payload should resolve");
        assert_eq!(facts.declared_example, None);
    }

    #[test]
    fn parse_project_facts_excludes_non_example_targets() {
        let metadata = metadata_with("app#0.1.0", None, &["counter"]);
        let facts = parse_project_facts(&metadata).expect("this payload should resolve");
        assert!(
            !facts.example_targets.iter().any(|name| name == "main"),
            "a [[bin]] target must not be reported as a real [[example]] target"
        );
    }

    #[test]
    fn parse_project_facts_returns_none_without_a_resolved_root() {
        let metadata = json!({
            "workspace_root": "/workspace",
            "resolve": {"root": null},
            "packages": [],
        });
        assert!(parse_project_facts(&metadata).is_none());
    }
}

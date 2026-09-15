//! Versioned run history for the conformance suite: every `florui
//! compare-all` invocation gets its own numbered `runs/NNNNNN/` directory
//! under a persistent output root, diffed against a baseline run (the most
//! recent prior one by default) so a regression across a code change shows
//! up as a concrete delta instead of having to be re-derived from raw pixel
//! numbers by hand each time. Modeled on the sibling `rustedf` project's
//! `tools/pdf-interop/compare.mjs`, reimplemented here in Rust rather than
//! Node so it stays a `cargo`-only dependency, and simplified for a single
//! engine pair (Florui vs. Chromium) instead of `rustedf`'s three.
//!
//! This module owns the pure bookkeeping — run ids, manifests, deltas,
//! Markdown rendering; `florui-cli` owns launching Chromium and actually
//! rendering each fixture (already `compare_fixture`'s job), calling into
//! here only to allocate a run directory up front and fold the results
//! together afterward.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::GeometryReport;
use crate::pixels::PixelSummary;
use crate::report::Outcome;

pub const RUNS_DIRECTORY: &str = "runs";
const RUN_ID_WIDTH: usize = 6;

#[derive(Debug)]
pub enum RunHistoryError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Encode {
        source: serde_json::Error,
    },
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// An explicitly-requested `--baseline <id>` has no `report.json` —
    /// distinct from [`Self::Io`]/[`Self::Decode`] since this is a caller
    /// error (a typo'd or never-completed run id), not an I/O failure.
    BaselineNotFound {
        run_id: String,
    },
}

impl fmt::Display for RunHistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunHistoryError::Io { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
            RunHistoryError::Encode { source } => write!(f, "could not encode run data: {source}"),
            RunHistoryError::Decode { path, source } => {
                write!(f, "could not parse {}: {source}", path.display())
            }
            RunHistoryError::BaselineNotFound { run_id } => {
                write!(
                    f,
                    "baseline run {run_id} has no report.json to compare against"
                )
            }
        }
    }
}

impl std::error::Error for RunHistoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunHistoryError::Io { source, .. } => Some(source),
            RunHistoryError::Encode { source } => Some(source),
            RunHistoryError::Decode { source, .. } => Some(source),
            RunHistoryError::BaselineNotFound { .. } => None,
        }
    }
}

fn io_err(path: &Path, source: std::io::Error) -> RunHistoryError {
    RunHistoryError::Io {
        path: path.to_owned(),
        source,
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), RunHistoryError> {
    let json =
        serde_json::to_string_pretty(value).map_err(|source| RunHistoryError::Encode { source })?;
    std::fs::write(path, json + "\n").map_err(|source| io_err(path, source))
}

/// Reads and parses `path` as JSON, or `Ok(None)` if it simply doesn't
/// exist yet (an incomplete or baseline-less run) — only a genuine I/O
/// failure (permissions, a directory in its place) is an error.
fn read_json_if_present<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<Option<T>, RunHistoryError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            serde_json::from_str(&contents)
                .map(Some)
                .map_err(|source| RunHistoryError::Decode {
                    path: path.to_owned(),
                    source,
                })
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_err(path, source)),
    }
}

/// Zero-pads `number` to [`RUN_ID_WIDTH`] digits — `runs/000042`, not
/// `runs/42`, so directory listings sort the same as run order without a
/// caller needing to parse and compare numerically.
pub fn run_id(number: u32) -> String {
    format!("{number:0width$}", width = RUN_ID_WIDTH)
}

fn is_run_id(name: &str) -> bool {
    name.len() == RUN_ID_WIDTH && name.bytes().all(|b| b.is_ascii_digit())
}

/// Every run id already recorded under `output_root/runs/`, sorted
/// ascending (oldest first) — empty, not an error, if no run has ever
/// happened there yet.
pub fn list_run_ids(output_root: &Path) -> Result<Vec<String>, RunHistoryError> {
    let runs_dir = output_root.join(RUNS_DIRECTORY);
    let entries = match std::fs::read_dir(&runs_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_err(&runs_dir, source)),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| io_err(&runs_dir, source))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_run_id(name) && entry.path().is_dir() {
            ids.push(name.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

/// Claims the next unused run directory after `existing`'s highest id
/// (or `000001` if there is none yet), creating it. Loops past any id
/// that turns out to already exist rather than trusting `existing` alone
/// — the same race `rustedf`'s own allocator guards against, cheap
/// insurance against a concurrent run or a leftover directory `existing`
/// didn't see.
pub fn allocate_run(
    output_root: &Path,
    existing: &[String],
) -> Result<(String, PathBuf), RunHistoryError> {
    let runs_dir = output_root.join(RUNS_DIRECTORY);
    std::fs::create_dir_all(&runs_dir).map_err(|source| io_err(&runs_dir, source))?;
    let mut next: u32 = existing
        .last()
        .and_then(|id| id.parse().ok())
        .map_or(1, |last: u32| last + 1);
    loop {
        let id = run_id(next);
        let directory = runs_dir.join(&id);
        match std::fs::create_dir(&directory) {
            Ok(()) => return Ok((id, directory)),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => next += 1,
            Err(source) => return Err(io_err(&directory, source)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub version: u32,
    pub run_id: String,
    pub baseline_run: Option<String>,
    pub fixture_count: usize,
    pub status: RunStatus,
}

pub fn write_manifest(run_dir: &Path, manifest: &RunManifest) -> Result<(), RunHistoryError> {
    write_json(&run_dir.join("manifest.json"), manifest)
}

fn load_manifest(run_dir: &Path) -> Result<Option<RunManifest>, RunHistoryError> {
    read_json_if_present(&run_dir.join("manifest.json"))
}

/// One fixture's outcome inside a run's own `report.json` — the same
/// facts [`crate::report::Report`] already carries per fixture, minus the
/// artifact paths (those live at fixed relative paths inside the run
/// directory — `<fixture_id>/reference.png` etc. — not worth
/// re-serializing here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureOutcome {
    pub fixture_id: String,
    pub outcome: Outcome,
    pub pixels: PixelSummary,
    pub geometry: GeometryReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub version: u32,
    pub run_id: String,
    pub baseline_run: Option<String>,
    pub fixtures: Vec<FixtureOutcome>,
}

pub fn write_run_report(run_dir: &Path, report: &RunReport) -> Result<(), RunHistoryError> {
    write_json(&run_dir.join("report.json"), report)
}

/// Loads a completed run's `report.json`, or `None` if that run never
/// finished (or never happened) — used both to resolve `--baseline auto`
/// and to load an explicitly-named one.
pub fn load_run_report(output_root: &Path, id: &str) -> Result<Option<RunReport>, RunHistoryError> {
    read_json_if_present(
        &output_root
            .join(RUNS_DIRECTORY)
            .join(id)
            .join("report.json"),
    )
}

/// Which prior run, if any, a new run should be diffed against.
pub enum BaselineSelector {
    /// The most recent run with a completed `report.json`, skipping over
    /// any that never finished — the default.
    Auto,
    /// No delta at all (this run's own `delta.json`/`delta.md` records
    /// `status: baseline`, same as the very first run ever).
    None,
    /// A specific run id — an error if it has no `report.json`, since the
    /// caller named it deliberately and a silent fallback to "no
    /// baseline" would hide a typo.
    Explicit(String),
}

pub fn select_baseline(
    output_root: &Path,
    existing: &[String],
    selector: &BaselineSelector,
) -> Result<Option<(String, RunReport)>, RunHistoryError> {
    match selector {
        BaselineSelector::None => Ok(None),
        BaselineSelector::Explicit(id) => match load_run_report(output_root, id)? {
            Some(report) => Ok(Some((id.clone(), report))),
            None => Err(RunHistoryError::BaselineNotFound { run_id: id.clone() }),
        },
        BaselineSelector::Auto => {
            for id in existing.iter().rev() {
                if let Some(report) = load_run_report(output_root, id)? {
                    return Ok(Some((id.clone(), report)));
                }
            }
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaStatus {
    /// No baseline was available (or requested) to diff against — the
    /// first run ever, or `--baseline none`.
    Baseline,
    Unchanged,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixtureChange {
    Added {
        fixture_id: String,
    },
    Removed {
        fixture_id: String,
    },
    Changed {
        fixture_id: String,
        // Boxed so this variant doesn't dwarf `Added`/`Removed` and blow
        // up every `Vec<FixtureChange>` allocation to fit the biggest
        // case.
        previous: Box<FixtureOutcome>,
        current: Box<FixtureOutcome>,
    },
}

/// Below this, a difference in `percent_different`/`max_axis_delta_px`
/// between two runs is measurement noise, not a real regression — either
/// genuine run-to-run jitter in Chromium's own capture (two separate
/// launches are not guaranteed bit-for-bit identical) or a few ULP of
/// floating-point drift surviving a JSON round trip. A real regression
/// moves these by whole percentage points or pixels, several orders of
/// magnitude above this.
const PIXEL_PERCENT_EPSILON: f64 = 0.05;
const GEOMETRY_PX_EPSILON: f64 = 0.05;

fn fixture_outcome_changed(before: &FixtureOutcome, after: &FixtureOutcome) -> bool {
    before.outcome != after.outcome
        || (before.pixels.percent_different - after.pixels.percent_different).abs()
            > PIXEL_PERCENT_EPSILON
        || (before.geometry.max_axis_delta_px - after.geometry.max_axis_delta_px).abs()
            > GEOMETRY_PX_EPSILON
}

fn fixture_change_id(change: &FixtureChange) -> &str {
    match change {
        FixtureChange::Added { fixture_id }
        | FixtureChange::Removed { fixture_id }
        | FixtureChange::Changed { fixture_id, .. } => fixture_id,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDelta {
    pub run_id: String,
    pub baseline_run: Option<String>,
    pub status: DeltaStatus,
    pub changes: Vec<FixtureChange>,
}

/// Diffs `current` against `baseline` (`None` for the first run ever, or
/// an explicitly baseline-less one) fixture by fixture: added, removed, or
/// changed — see [`fixture_outcome_changed`] for what counts as a real
/// change versus measurement noise.
pub fn build_delta(
    run_id: String,
    baseline: Option<&(String, RunReport)>,
    current: &RunReport,
) -> RunDelta {
    let Some((baseline_id, baseline_report)) = baseline else {
        return RunDelta {
            run_id,
            baseline_run: None,
            status: DeltaStatus::Baseline,
            changes: Vec::new(),
        };
    };

    let mut fixture_ids: Vec<&str> = baseline_report
        .fixtures
        .iter()
        .map(|f| f.fixture_id.as_str())
        .chain(current.fixtures.iter().map(|f| f.fixture_id.as_str()))
        .collect();
    fixture_ids.sort_unstable();
    fixture_ids.dedup();

    let mut changes = Vec::new();
    for fixture_id in fixture_ids {
        let before = baseline_report
            .fixtures
            .iter()
            .find(|f| f.fixture_id == fixture_id);
        let after = current.fixtures.iter().find(|f| f.fixture_id == fixture_id);
        match (before, after) {
            (None, Some(_)) => changes.push(FixtureChange::Added {
                fixture_id: fixture_id.to_owned(),
            }),
            (Some(_), None) => changes.push(FixtureChange::Removed {
                fixture_id: fixture_id.to_owned(),
            }),
            (Some(before), Some(after)) if fixture_outcome_changed(before, after) => {
                changes.push(FixtureChange::Changed {
                    fixture_id: fixture_id.to_owned(),
                    previous: Box::new(before.clone()),
                    current: Box::new(after.clone()),
                })
            }
            _ => {}
        }
    }

    RunDelta {
        run_id,
        baseline_run: Some(baseline_id.clone()),
        status: if changes.is_empty() {
            DeltaStatus::Unchanged
        } else {
            DeltaStatus::Changed
        },
        changes,
    }
}

pub fn write_run_delta(run_dir: &Path, delta: &RunDelta) -> Result<(), RunHistoryError> {
    write_json(&run_dir.join("delta.json"), delta)
}

fn format_outcome(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Pass => "pass",
        Outcome::Fail => "fail",
    }
}

/// `runs/<id>/report.md` — one row per fixture, readable without opening
/// `report.json` or any image.
pub fn render_report_markdown(report: &RunReport) -> String {
    let mut lines = vec![
        format!("# Conformance run {}", report.run_id),
        String::new(),
        format!(
            "Baseline: {}",
            report.baseline_run.as_deref().unwrap_or("none")
        ),
        String::new(),
        "| Fixture | Outcome | Pixel diff | Max geometry delta (px) |".to_owned(),
        "| --- | --- | ---: | ---: |".to_owned(),
    ];
    for fixture in &report.fixtures {
        lines.push(format!(
            "| {} | {} | {:.2}% | {:.1} |",
            fixture.fixture_id,
            format_outcome(fixture.outcome),
            fixture.pixels.percent_different,
            fixture.geometry.max_axis_delta_px
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn format_delta_outcome(before: Outcome, after: Outcome) -> String {
    if before == after {
        format_outcome(after).to_owned()
    } else {
        format!("{} -> {}", format_outcome(before), format_outcome(after))
    }
}

/// `runs/<id>/delta.md` — what changed since the baseline run, or a plain
/// statement that nothing did. Deliberately does not decide whether a
/// change is a regression; a fixture moving from `fail` to `pass` and one
/// moving from `pass` to `fail` are reported identically as `changed`, for
/// a human to judge.
pub fn render_delta_markdown(delta: &RunDelta) -> String {
    let mut lines = vec![
        "# Conformance run delta".to_owned(),
        String::new(),
        format!("Run: {}", delta.run_id),
        format!(
            "Baseline: {}",
            delta.baseline_run.as_deref().unwrap_or("none")
        ),
        format!("Status: {}", delta_status_label(delta.status)),
        String::new(),
    ];
    match delta.status {
        DeltaStatus::Baseline => {
            lines.push("This is the first run, or no baseline was requested.".to_owned());
        }
        DeltaStatus::Unchanged => {
            lines.push("No fixture's outcome, pixel diff, or geometry changed.".to_owned());
        }
        DeltaStatus::Changed => {
            lines.push(
                "Changes require review — this harness does not decide whether an \
                 intentional change is a regression."
                    .to_owned(),
            );
            lines.push(String::new());
            lines.push("| Fixture | Kind | Details |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for change in &delta.changes {
                let (kind, details) = match change {
                    FixtureChange::Added { .. } => ("added", String::new()),
                    FixtureChange::Removed { .. } => ("removed", String::new()),
                    FixtureChange::Changed {
                        previous, current, ..
                    } => (
                        "changed",
                        format!(
                            "outcome: {}; pixel diff: {:.2}% -> {:.2}%; max geometry delta: {:.1}px -> {:.1}px",
                            format_delta_outcome(previous.outcome, current.outcome),
                            previous.pixels.percent_different,
                            current.pixels.percent_different,
                            previous.geometry.max_axis_delta_px,
                            current.geometry.max_axis_delta_px,
                        ),
                    ),
                };
                lines.push(format!(
                    "| {} | {kind} | {details} |",
                    fixture_change_id(change)
                ));
            }
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

fn delta_status_label(status: DeltaStatus) -> &'static str {
    match status {
        DeltaStatus::Baseline => "baseline",
        DeltaStatus::Unchanged => "unchanged",
        DeltaStatus::Changed => "changed",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub run_id: String,
    pub status: RunStatus,
    pub baseline_run: Option<String>,
    pub fixture_count: usize,
    pub delta_status: Option<DeltaStatus>,
    pub changes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub latest_run: Option<String>,
    pub runs: Vec<IndexEntry>,
}

fn load_delta(output_root: &Path, id: &str) -> Result<Option<RunDelta>, RunHistoryError> {
    read_json_if_present(&output_root.join(RUNS_DIRECTORY).join(id).join("delta.json"))
}

fn build_index(output_root: &Path) -> Result<Index, RunHistoryError> {
    let ids = list_run_ids(output_root)?;
    let mut runs = Vec::with_capacity(ids.len());
    for id in &ids {
        let run_dir = output_root.join(RUNS_DIRECTORY).join(id);
        let Some(manifest) = load_manifest(&run_dir)? else {
            // A run directory with no manifest.json is one whose very
            // first write failed mid-flight — nothing meaningful to index.
            continue;
        };
        let delta = load_delta(output_root, id)?;
        runs.push(IndexEntry {
            run_id: id.clone(),
            status: manifest.status,
            baseline_run: manifest.baseline_run,
            fixture_count: manifest.fixture_count,
            delta_status: delta.as_ref().map(|d| d.status),
            changes: delta.as_ref().map(|d| d.changes.len()),
        });
    }
    Ok(Index {
        version: 1,
        latest_run: runs.last().map(|r| r.run_id.clone()),
        runs,
    })
}

/// `output_root/index.md` — every run ever taken, newest last, so a
/// regression can be traced back to the run (and by extension the change)
/// that introduced it.
pub fn render_index_markdown(index: &Index) -> String {
    let mut lines = vec!["# Conformance run history".to_owned(), String::new()];
    match &index.latest_run {
        Some(id) => lines.push(format!(
            "Latest run: [{id}]({RUNS_DIRECTORY}/{id}/report.md)"
        )),
        None => lines.push("Latest run: none".to_owned()),
    }
    lines.push(String::new());
    lines.push("| Run | Status | Baseline | Fixtures | Changes |".to_owned());
    lines.push("| --- | --- | --- | ---: | ---: |".to_owned());
    for run in &index.runs {
        let status = run
            .delta_status
            .map(delta_status_label)
            .unwrap_or(match run.status {
                RunStatus::Running => "running",
                RunStatus::Complete => "complete",
            });
        let changes = run
            .changes
            .map(|n| n.to_string())
            .unwrap_or_else(|| "n/a".to_owned());
        lines.push(format!(
            "| [{}]({RUNS_DIRECTORY}/{}/report.md) | {status} | {} | {} | {changes} |",
            run.run_id,
            run.run_id,
            run.baseline_run.as_deref().unwrap_or("none"),
            run.fixture_count,
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Rebuilds `output_root/index.json` and `index.md` from every run
/// directory on disk — called once at the end of each `florui compare-all`
/// invocation, but safe to re-run any time (it never trusts a stale
/// in-memory list, only what's actually on disk).
pub fn write_index(output_root: &Path) -> Result<(), RunHistoryError> {
    let index = build_index(output_root)?;
    write_json(&output_root.join("index.json"), &index)?;
    std::fs::write(output_root.join("index.md"), render_index_markdown(&index))
        .map_err(|source| io_err(&output_root.join("index.md"), source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(percent_different: f64) -> PixelSummary {
        PixelSummary {
            differing_pixels: 0,
            total_pixels: 100,
            percent_different,
            mean_error: 0.0,
            max_error: 0,
        }
    }

    fn geometry(max_axis_delta_px: f64) -> GeometryReport {
        use crate::geometry::BoxGeometryPx;
        crate::geometry::compare_geometry(
            BoxGeometryPx {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            BoxGeometryPx {
                x: 0.0,
                y: 0.0,
                width: 10.0 + max_axis_delta_px,
                height: 10.0,
            },
            10.0,
        )
    }

    fn outcome_of(fixture_id: &str, percent_different: f64) -> FixtureOutcome {
        FixtureOutcome {
            fixture_id: fixture_id.to_owned(),
            outcome: Outcome::Pass,
            pixels: pixels(percent_different),
            geometry: geometry(0.0),
        }
    }

    #[test]
    fn run_id_zero_pads_to_six_digits() {
        assert_eq!(run_id(1), "000001");
        assert_eq!(run_id(42), "000042");
        assert_eq!(run_id(123_456), "123456");
    }

    #[test]
    fn the_first_run_ever_has_no_baseline_and_reports_status_baseline() {
        let current = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![outcome_of("div-default", 0.0)],
        };
        let delta = build_delta("000001".to_owned(), None, &current);
        assert_eq!(delta.status, DeltaStatus::Baseline);
        assert!(delta.changes.is_empty());
    }

    #[test]
    fn identical_fixtures_across_runs_report_unchanged() {
        let baseline = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![outcome_of("div-default", 1.0)],
        };
        let current = RunReport {
            version: 1,
            run_id: "000002".to_owned(),
            baseline_run: Some("000001".to_owned()),
            fixtures: vec![outcome_of("div-default", 1.0)],
        };
        let delta = build_delta(
            "000002".to_owned(),
            Some(&("000001".to_owned(), baseline)),
            &current,
        );
        assert_eq!(delta.status, DeltaStatus::Unchanged);
        assert!(delta.changes.is_empty());
    }

    #[test]
    fn a_fixture_whose_pixel_diff_moved_is_reported_as_changed() {
        let baseline = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![outcome_of("mixed-inline-scaled", 1.0)],
        };
        let current = RunReport {
            version: 1,
            run_id: "000002".to_owned(),
            baseline_run: Some("000001".to_owned()),
            fixtures: vec![outcome_of("mixed-inline-scaled", 5.0)],
        };
        let delta = build_delta(
            "000002".to_owned(),
            Some(&("000001".to_owned(), baseline)),
            &current,
        );
        assert_eq!(delta.status, DeltaStatus::Changed);
        assert_eq!(delta.changes.len(), 1);
        assert!(matches!(
            &delta.changes[0],
            FixtureChange::Changed { fixture_id, .. } if fixture_id == "mixed-inline-scaled"
        ));
    }

    #[test]
    fn a_fixture_whose_pixel_diff_moved_by_only_a_ulp_or_two_is_not_reported_as_changed() {
        // Real values observed round-tripping the identical measurement
        // through JSON with serde_json's default (non-`float_roundtrip`)
        // float parser: 1.9891666666666663 vs. ...665, a difference far
        // below anything a real rendering change could produce.
        let baseline = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![outcome_of("card-narrow", 1.9891666666666663)],
        };
        let current = RunReport {
            version: 1,
            run_id: "000002".to_owned(),
            baseline_run: Some("000001".to_owned()),
            fixtures: vec![outcome_of("card-narrow", 1.9891666666666665)],
        };
        let delta = build_delta(
            "000002".to_owned(),
            Some(&("000001".to_owned(), baseline)),
            &current,
        );
        assert_eq!(delta.status, DeltaStatus::Unchanged);
        assert!(delta.changes.is_empty());
    }

    #[test]
    fn a_fixture_only_in_the_current_run_is_reported_as_added() {
        let baseline = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![],
        };
        let current = RunReport {
            version: 1,
            run_id: "000002".to_owned(),
            baseline_run: Some("000001".to_owned()),
            fixtures: vec![outcome_of("card-scaled", 0.0)],
        };
        let delta = build_delta(
            "000002".to_owned(),
            Some(&("000001".to_owned(), baseline)),
            &current,
        );
        assert_eq!(
            delta.changes,
            vec![FixtureChange::Added {
                fixture_id: "card-scaled".to_owned()
            }]
        );
    }

    #[test]
    fn a_fixture_only_in_the_baseline_run_is_reported_as_removed() {
        let baseline = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![outcome_of("card-scaled", 0.0)],
        };
        let current = RunReport {
            version: 1,
            run_id: "000002".to_owned(),
            baseline_run: Some("000001".to_owned()),
            fixtures: vec![],
        };
        let delta = build_delta(
            "000002".to_owned(),
            Some(&("000001".to_owned(), baseline)),
            &current,
        );
        assert_eq!(
            delta.changes,
            vec![FixtureChange::Removed {
                fixture_id: "card-scaled".to_owned()
            }]
        );
    }

    #[test]
    fn report_markdown_lists_every_fixture_with_its_own_metrics() {
        let report = RunReport {
            version: 1,
            run_id: "000001".to_owned(),
            baseline_run: None,
            fixtures: vec![outcome_of("div-default", 1.5)],
        };
        let markdown = render_report_markdown(&report);
        assert!(markdown.contains("# Conformance run 000001"));
        assert!(markdown.contains("div-default"));
        assert!(markdown.contains("1.50%"));
    }

    #[test]
    fn delta_markdown_states_plainly_when_nothing_changed() {
        let delta = RunDelta {
            run_id: "000002".to_owned(),
            baseline_run: Some("000001".to_owned()),
            status: DeltaStatus::Unchanged,
            changes: Vec::new(),
        };
        let markdown = render_delta_markdown(&delta);
        assert!(markdown.contains("Status: unchanged"));
        assert!(markdown.contains("No fixture's outcome"));
    }

    #[test]
    fn index_markdown_links_every_run_to_its_own_report() {
        let index = Index {
            version: 1,
            latest_run: Some("000002".to_owned()),
            runs: vec![
                IndexEntry {
                    run_id: "000001".to_owned(),
                    status: RunStatus::Complete,
                    baseline_run: None,
                    fixture_count: 16,
                    delta_status: Some(DeltaStatus::Baseline),
                    changes: Some(0),
                },
                IndexEntry {
                    run_id: "000002".to_owned(),
                    status: RunStatus::Complete,
                    baseline_run: Some("000001".to_owned()),
                    fixture_count: 16,
                    delta_status: Some(DeltaStatus::Changed),
                    changes: Some(1),
                },
            ],
        };
        let markdown = render_index_markdown(&index);
        assert!(markdown.contains("Latest run: [000002](runs/000002/report.md)"));
        assert!(markdown.contains("[000001](runs/000001/report.md)"));
        assert!(markdown.contains("[000002](runs/000002/report.md)"));
    }

    #[test]
    fn allocate_run_claims_the_next_id_after_the_highest_existing_one() {
        let temp = std::env::temp_dir().join(format!(
            "florui-run-history-test-{}-{}",
            std::process::id(),
            "allocate_next"
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let (id, directory) = allocate_run(&temp, &["000001".to_owned(), "000002".to_owned()])
            .expect("allocation should succeed in a writable temp directory");
        assert_eq!(id, "000003");
        assert!(directory.is_dir());
        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn allocate_run_starts_at_one_with_no_existing_runs() {
        let temp = std::env::temp_dir().join(format!(
            "florui-run-history-test-{}-{}",
            std::process::id(),
            "allocate_first"
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let (id, _) = allocate_run(&temp, &[])
            .expect("allocation should succeed in a writable temp directory");
        assert_eq!(id, "000001");
        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn list_run_ids_is_empty_for_a_root_with_no_runs_directory_yet() {
        let temp = std::env::temp_dir().join(format!(
            "florui-run-history-test-{}-{}",
            std::process::id(),
            "list_empty"
        ));
        let ids = list_run_ids(&temp).expect("a missing runs/ directory is not an error");
        assert!(ids.is_empty());
    }

    #[test]
    fn select_baseline_explicit_errors_on_a_run_with_no_report() {
        let temp = std::env::temp_dir().join(format!(
            "florui-run-history-test-{}-{}",
            std::process::id(),
            "baseline_missing"
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let error = select_baseline(&temp, &[], &BaselineSelector::Explicit("000099".to_owned()))
            .unwrap_err();
        assert!(matches!(
            error,
            RunHistoryError::BaselineNotFound { run_id } if run_id == "000099"
        ));
        std::fs::remove_dir_all(&temp).ok();
    }
}

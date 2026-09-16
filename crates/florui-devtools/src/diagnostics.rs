//! Structured update events for the native preview host.
//!
//! Full causal diagnostics need recorded constraints and provenance that do
//! not exist yet. This is a minimal event stream: stable element IDs,
//! timestamps, and honest reporting of stale/failed updates.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A stable identity for a drawable element. IDs are never reused within a
/// process, so a held `ElementId` can be checked against removal instead of
/// silently referring to whatever now occupies the same slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId(u64);

impl ElementId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Debug)]
pub enum DevEvent<'a> {
    FixtureLoaded {
        path: &'a std::path::Path,
        revision: u64,
    },
    FixtureReloadFailed {
        path: &'a std::path::Path,
        error: &'a dyn std::error::Error,
    },
    FrameRendered {
        element: ElementId,
        revision: u64,
        render_time: std::time::Duration,
    },
}

impl fmt::Display for DevEvent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DevEvent::FixtureLoaded { path, revision } => {
                write!(
                    f,
                    "fixture loaded: {} (revision {revision})",
                    path.display()
                )
            }
            DevEvent::FixtureReloadFailed { path, error } => write!(
                f,
                "fixture reload failed for {}: {error} (keeping last valid revision)",
                path.display()
            ),
            DevEvent::FrameRendered {
                element,
                revision,
                render_time,
            } => write!(
                f,
                "frame rendered: element {element} at revision {revision} in {render_time:?}"
            ),
        }
    }
}

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RED_BOLD: &str = "\x1b[1;31m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Whether ANSI styling should be emitted, per the `NO_COLOR` convention
/// (<https://no-color.org>). Checked per call so tests and users can toggle
/// it via the environment without restarting.
fn colors_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, text: &str) -> String {
    if colors_enabled() {
        format!("{code}{text}{RESET}")
    } else {
        text.to_owned()
    }
}

fn dim(text: &str) -> String {
    paint(DIM, text)
}

/// Green text, for a command's success/pass output (e.g. `florui compare`).
pub fn success(text: &str) -> String {
    paint(GREEN, text)
}

/// Bold red text, for a command's failure output.
pub fn failure(text: &str) -> String {
    paint(RED_BOLD, text)
}

/// Yellow text, for a command's warning output — something worth noticing
/// that isn't itself a failure.
pub fn warning(text: &str) -> String {
    paint(YELLOW, text)
}

/// Dimmed text, for secondary detail alongside a success/failure line.
pub fn dim_text(text: &str) -> String {
    dim(text)
}

fn elapsed_label(epoch: Instant) -> String {
    format!("[{:>10.3?}]", epoch.elapsed())
}

/// Prints the one-time startup banner for `florui dev`, in the terminal
/// style of tools like Vite: a name/command line, then what is being
/// watched.
pub fn print_banner(fixture_path: &std::path::Path) {
    eprintln!();
    eprintln!("  {}", paint(BOLD, "florui dev"));
    eprintln!();
    eprintln!(
        "  {}  watching {}",
        paint(GREEN, "➜"),
        fixture_path.display()
    );
    eprintln!();
}

/// Logs an event with a monotonic elapsed timestamp relative to `epoch`,
/// styled so a successful reload, a failed one, and routine frame timing are
/// visually distinct at a glance — success in green, failure in red and
/// expanded, routine timing dimmed.
///
/// A dedicated sink (file, channel, inspector UI) can replace this once one
/// exists, without pulling in a logging framework this crate does not
/// otherwise need.
pub fn log_event(epoch: Instant, event: &DevEvent<'_>) {
    match event {
        DevEvent::FixtureLoaded { path, revision } => {
            eprintln!(
                "{} {} fixture ready {}",
                dim(&elapsed_label(epoch)),
                paint(GREEN, "✔"),
                dim(&format!("{} (revision {revision})", path.display()))
            );
        }
        DevEvent::FixtureReloadFailed { path, error } => {
            eprintln!(
                "{} {} {}",
                dim(&elapsed_label(epoch)),
                paint(RED, "✘"),
                paint(
                    RED_BOLD,
                    &format!("fixture reload failed: {}", path.display())
                )
            );
            eprintln!("               {}", paint(RED, &error.to_string()));
            eprintln!("               {}", dim("keeping last valid revision"));
        }
        DevEvent::FrameRendered {
            element,
            revision,
            render_time,
        } => {
            eprintln!(
                "{}",
                dim(&format!(
                    "{} frame {element} rendered at revision {revision} in {render_time:?}",
                    elapsed_label(epoch)
                ))
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_increasing() {
        let a = ElementId::next();
        let b = ElementId::next();
        assert_ne!(a, b);
    }

    /// Doesn't assert colored vs. plain output directly: `NO_COLOR` is
    /// process-global and tests run in parallel, so asserting on a specific
    /// state would be flaky depending on what's set in the ambient
    /// environment. The text itself must survive either way.
    #[test]
    fn public_formatters_preserve_their_text() {
        assert!(success("ok").contains("ok"));
        assert!(failure("boom").contains("boom"));
        assert!(dim_text("detail").contains("detail"));
    }
}

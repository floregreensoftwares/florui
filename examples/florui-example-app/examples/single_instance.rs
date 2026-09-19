//! Opt-in single-instance behavior: run this example, then run it again
//! (optionally with extra arguments) while the first is still open -- the
//! second launch hands its own launch arguments to the first instead of
//! opening a second window, and the first instance's own window updates to
//! show what it received.
//!
//! `cargo run --example single_instance -p florui-example-app -- some args`

use florui::prelude::*;
use florui_platform::{
    ActivationEvent, RunOutcome, SingleInstance, WindowSpec, run_single_instance, run_windows,
    use_activation_events,
};

const CSS: &str = include_str!("two_windows.css");

/// Resolves this package's real `florui.config.toml` -- same pattern
/// `two_windows.rs` already uses for window persistence. `None` (with a
/// logged reason) if `[app.activation].single_instance` isn't enabled or
/// `app.identifier` is unset -- an ordinary, expected configuration
/// choice, not a failure.
fn resolve_single_instance() -> Option<SingleInstance> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let facts = florui_config::resolve_cargo_project(&manifest_dir, None)
        .inspect_err(|error| eprintln!("single_instance: could not resolve the project: {error}"))
        .ok()?;
    let resolution = florui_config::resolve(&facts, Some(florui_config::Target::Native), None)
        .inspect_err(|error| {
            eprintln!("single_instance: could not resolve florui.config.toml: {error}")
        })
        .ok()?;
    if !resolution.config.app.activation.single_instance {
        return None;
    }
    let app_identifier = resolution.config.app.identifier?;
    Some(SingleInstance {
        // This crate's `florui.config.toml` is shared with every other
        // example here -- this suffix keeps this demo's own
        // single-instance scope from colliding with any of them.
        app_identifier: format!("{app_identifier}.single-instance-demo"),
        ..SingleInstance::default()
    })
}

fn build_window_spec() -> WindowSpec {
    WindowSpec::new(
        "Florui -- single instance",
        CSS,
        florui_style::Rgba::opaque(0x1e, 0x1e, 0x22),
        Default::default(),
        root,
    )
    .expect("this example's stylesheet should parse")
}

fn main() {
    let launch_event = ActivationEvent::Launch {
        args: std::env::args().collect(),
    };
    let Some(config) = resolve_single_instance() else {
        eprintln!(
            "single_instance: [app.activation].single_instance is not enabled (or app.identifier is unset) -- running without single-instance enforcement"
        );
        run_windows(vec![build_window_spec()])
            .expect("event loop should not fail on a real desktop session");
        return;
    };

    match run_single_instance(config, launch_event, vec![build_window_spec()]) {
        Ok(RunOutcome::Ran) => {}
        Ok(RunOutcome::HandedOff) => {
            println!(
                "single_instance: handed launch arguments off to the already-running instance"
            );
        }
        Ok(RunOutcome::HandoffFailed(event)) => {
            eprintln!(
                "single_instance: an existing instance owns this identifier but did not acknowledge the hand-off ({event:?}) -- becoming a fresh primary instead"
            );
            run_windows(vec![build_window_spec()])
                .expect("event loop should not fail on a real desktop session");
        }
        Err(error) => panic!("single_instance example failed: {error}"),
    }
}

fn root() -> Element {
    let pending = use_activation_events()
        .map(|events| events.take_pending())
        .unwrap_or_default();
    let hint = if pending.is_empty() {
        "No activation event received yet on this render.".to_owned()
    } else {
        pending
            .iter()
            .map(|event| format!("{event:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    view! {
        <div class="page">
            <p class="label">{"Single-instance demo"}</p>
            <p class="hint">
                {"Run this example again, optionally with extra arguments -- the second launch hands off instead of opening a new window."}
            </p>
            <p class="hint">{hint}</p>
        </div>
    }
}

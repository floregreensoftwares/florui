//! Two real windows open at once, each independently titled and iconed:
//! window A loads `florui-config`'s resolved `app.icons.source` at
//! runtime (`florui_icon::load_icon_at_size` against a real, resolved
//! path); window B starts with a build-time embedded icon
//! (`florui_icon::embed`, via `build.rs` -- no source file needed at ship
//! time) and its button swaps it live, through
//! `WindowControls::set_icon`, to window A's icon -- a visible,
//! deterministic proof of the update path, not a timer.
//!
//! `cargo run --example two_windows -p florui-example-app`

use florui::prelude::*;
use florui_icon::RawIcon;
use florui_platform::{WindowOptions, WindowSpec, run_windows, use_window_controls};
use florui_reactive::use_signal;
use florui_style::Rgba;

const CSS: &str = include_str!("two_windows.css");

mod second_window_icon {
    include!(concat!(env!("OUT_DIR"), "/second_window_icon.rs"));
}

/// Resolves this package's real `florui.config.toml` and loads
/// `app.icons.source` at runtime -- exactly what `florui-cli`'s own
/// `main.rs`/`doctor.rs` already do at ordinary process runtime, so doing
/// the same here isn't anything special. `None` (with a logged reason) on
/// any failure -- a missing/unreadable icon shouldn't take the whole demo
/// down.
fn load_config_resolved_icon() -> Option<RawIcon> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let facts = florui_config::resolve_cargo_project(&manifest_dir, None)
        .inspect_err(|error| eprintln!("two_windows: could not resolve the project: {error}"))
        .ok()?;
    let resolution = florui_config::resolve(&facts, Some(florui_config::Target::Native))
        .inspect_err(|error| {
            eprintln!("two_windows: could not resolve florui.config.toml: {error}")
        })
        .ok()?;
    let path = resolution.config.app.icons.source?;
    florui_icon::load_icon_at_size(&path, florui_icon::DEFAULT_ICON_SIZE)
        .inspect_err(|error| eprintln!("two_windows: could not load {}: {error}", path.display()))
        .ok()
}

fn main() {
    let icon_a = load_config_resolved_icon();

    let icon_b = RawIcon {
        rgba: second_window_icon::SECOND_WINDOW_ICON_RGBA.to_vec(),
        width: second_window_icon::SECOND_WINDOW_ICON_WIDTH,
        height: second_window_icon::SECOND_WINDOW_ICON_HEIGHT,
    };

    let spec_a = WindowSpec::new(
        "Florui -- Window A (config icon)",
        CSS,
        Rgba::opaque(0x1e, 0x1e, 0x22),
        WindowOptions {
            icon: icon_a.clone(),
            ..WindowOptions::default()
        },
        window_a,
    )
    .expect("window A's stylesheet should parse");

    let spec_b = WindowSpec::new(
        "Florui -- Window B (embedded icon)",
        CSS,
        Rgba::opaque(0x22, 0x1a, 0x1e),
        WindowOptions {
            icon: Some(icon_b),
            ..WindowOptions::default()
        },
        move || window_b(icon_a.clone()),
    )
    .expect("window B's stylesheet should parse");

    run_windows(vec![spec_a, spec_b])
        .expect("event loop should not fail on a real desktop session");
}

fn window_a() -> Element {
    view! {
        <div class="page">
            <p class="label">{"Window A"}</p>
            <p class="hint">
                {"Icon loaded at runtime from florui-config's resolved app.icons.source."}
            </p>
        </div>
    }
}

/// `swap_to` is window A's own icon (if it loaded), reused here to prove
/// the update path with a plain `RawIcon` regardless of where it came
/// from -- embedded or runtime-loaded, `WindowControls::set_icon` doesn't
/// care.
fn window_b(swap_to: Option<RawIcon>) -> Element {
    let controls = use_window_controls();
    let updated = use_signal(|| false);

    let handle_click = {
        let controls = controls.clone();
        let swap_to = swap_to.clone();
        let updated = updated.clone();
        move || {
            if let (Some(controls), Some(icon)) = (&controls, &swap_to)
                && controls.set_icon(icon).is_ok()
            {
                updated.set(true);
            }
        }
    };

    view! {
        <div class="page">
            <p class="label">{"Window B"}</p>
            <p class="hint">
                {"Icon embedded at build time -- no source file needed at ship time."}
            </p>
            <button class="button" onclick={handle_click}>
                {if updated.get() {
                    "Icon updated live -- now matches window A"
                } else {
                    "Swap to window A's icon"
                }}
            </button>
        </div>
    }
}

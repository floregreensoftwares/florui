//! Three real, "advanced" window capabilities beyond ordinary decoration
//! control: always-on-top, fullscreen, whole-window click-through (with
//! a safe 3s auto-revert, since a click-through window can't click its
//! own button to turn itself back off), and a real system tray icon
//! with working click events.
//!
//! `cargo run --example window_powers -p florui-example-app`

use std::time::Duration;

use florui::prelude::*;
use florui_platform::tray::{TrayEvent, TrayIcon};
use florui_platform::{InputMode, use_window_controls};
use florui_reactive::{Cleanup, blocking::sleep, use_effect, use_resource, use_signal};
use florui_style::Rgba;

const CSS: &str = include_str!("window_powers.css");

fn main() {
    florui_platform::run(
        "Florui window powers",
        CSS,
        Rgba::opaque(0x1e, 0x1e, 0x22),
        root,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn root() -> Element {
    let controls = use_window_controls();
    let always_on_top = use_signal(|| false);
    let fullscreen = use_signal(|| false);
    let click_through_ticks = use_signal(|| 0u64);
    let tray_clicks = use_signal(|| 0u32);

    // Auto-reverts click-through 3s after each toggle-on, so the demo
    // can never lock itself out of its own UI.
    {
        let controls = controls.clone();
        let _revert = use_resource(click_through_ticks.get(), move |tick| {
            let controls = controls.clone();
            async move {
                if tick > 0 {
                    sleep(Duration::from_secs(3)).await;
                    if let Some(controls) = &controls {
                        controls.set_input_mode(InputMode::Normal);
                    }
                }
                Ok::<(), ()>(())
            }
        });
    }

    // Tray icon: created once, kept alive for the effect's own scope,
    // removed on unmount.
    {
        let tray_clicks = tray_clicks.clone();
        use_effect((), move || {
            let tray = TrayIcon::new("Florui window powers", move |event| {
                if matches!(event, TrayEvent::LeftClick | TrayEvent::RightClick) {
                    tray_clicks.set(tray_clicks.get() + 1);
                }
            });
            Some(Box::new(move || drop(tray)) as Cleanup)
        });
    }

    let toggle_top = controls.clone();
    let always_on_top_state = always_on_top.clone();
    let toggle_fullscreen = controls.clone();
    let fullscreen_state = fullscreen.clone();
    let start_click_through = controls.clone();
    let click_through_ticks_click = click_through_ticks.clone();

    view! {
        <div class="app">
            <p class="body-text">
                {"Real window capabilities beyond decoration: always-on-top, fullscreen, \
                  click-through, and a system tray icon."}
            </p>
            <button
                class="power-button"
                onclick={move || {
                    let next = !always_on_top_state.get();
                    always_on_top_state.set(next);
                    if let Some(controls) = &toggle_top {
                        controls.set_always_on_top(next);
                    }
                }}
            >
                {if always_on_top.get() { "Always on top: ON" } else { "Always on top: OFF" }}
            </button>
            <button
                class="power-button"
                onclick={move || {
                    let next = !fullscreen_state.get();
                    fullscreen_state.set(next);
                    if let Some(controls) = &toggle_fullscreen {
                        controls.set_fullscreen(next);
                    }
                }}
            >
                {if fullscreen.get() { "Fullscreen: ON" } else { "Fullscreen: OFF" }}
            </button>
            <button
                class="power-button"
                onclick={move || {
                    if let Some(controls) = &start_click_through {
                        controls.set_input_mode(InputMode::Passthrough);
                    }
                    click_through_ticks_click.set(click_through_ticks_click.get() + 1);
                }}
            >
                {"Click-through for 3s"}
            </button>
            <p class="status">
                {format!("Tray icon clicks received: {}", tray_clicks.get())}
            </p>
        </div>
    }
}

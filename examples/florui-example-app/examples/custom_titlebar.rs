//! A real, application-drawn title bar: the same freedom a Claude
//! Code-style custom title bar gives an Electron/Tauri app, where extra
//! app content (icon buttons) lives in the same row as the window's own
//! minimize/maximize/close. `DecorationMode::Custom` removes the OS's own
//! chrome entirely, so everything here — dragging the window, minimizing,
//! maximizing, closing — goes through `florui_platform::use_window_controls`
//! and `WINDOW_DRAG_REGION_ID` instead of coming for free the way it does
//! under system decorations.
//!
//! `cargo run --example custom_titlebar -p florui-example-app`

use florui::prelude::*;
use florui_platform::appearance::DecorationMode;
use florui_platform::{WINDOW_DRAG_REGION_ID, WindowOptions, use_window_controls};
use florui_reactive::{Cleanup, use_effect, use_signal};
use florui_style::Rgba;

const CSS: &str = include_str!("custom_titlebar.css");

fn main() {
    florui_platform::run_with_options(
        "Florui custom title bar",
        CSS,
        Rgba::opaque(0x1e, 0x1e, 0x22),
        WindowOptions {
            decorations: DecorationMode::Custom,
            ..WindowOptions::default()
        },
        title_bar_demo,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn title_bar_demo() -> Element {
    let controls = use_window_controls();
    let maximized = controls.as_ref().is_some_and(|c| c.is_maximized());
    let focused = controls.as_ref().is_none_or(|c| c.is_focused());

    // Demonstrates close cancellation: while unsaved, closing (via this
    // button or the real OS close) is vetoed.
    let unsaved = use_signal(|| true);
    {
        let controls = controls.clone();
        let unsaved = unsaved.clone();
        use_effect((), move || {
            if let Some(controls) = &controls {
                let unsaved = unsaved.clone();
                controls.set_close_guard(move || !unsaved.get());
            }
            let controls = controls.clone();
            Some(Box::new(move || {
                if let Some(controls) = &controls {
                    controls.clear_close_guard();
                }
            }) as Cleanup)
        });
    }

    let minimize = controls.clone();
    let toggle_maximize = controls.clone();
    let close = controls.clone();
    let toggle_unsaved = unsaved.clone();

    view! {
        <div class="app">
            <div class={if focused { "titlebar" } else { "titlebar inactive" }}>
                <span class="titlebar-title">{"Florui"}</span>
                <div id={WINDOW_DRAG_REGION_ID} class="drag-region" />
                <div class="titlebar-extra">
                    <button class="icon-button">{"*"}</button>
                    <button class="icon-button">{"?"}</button>
                </div>
                <div class="window-buttons">
                    <button
                        class="window-button"
                        onclick={move || if let Some(controls) = &minimize {
                            controls.minimize();
                        }}
                    >
                        {"_"}
                    </button>
                    <button
                        class="window-button"
                        onclick={move || if let Some(controls) = &toggle_maximize {
                            controls.toggle_maximize();
                        }}
                    >
                        {if maximized { "[ ]" } else { "[]" }}
                    </button>
                    <button
                        class="close-button"
                        onclick={move || if let Some(controls) = &close {
                            controls.close();
                        }}
                    >
                        {"x"}
                    </button>
                </div>
            </div>
            <div class="body">
                <p class="body-text">
                    {"Drag from the empty title bar space. Minimize/maximize/close all drive \
                      the real window through WindowControls, not OS chrome."}
                </p>
                <p class="body-text">
                    {if unsaved.get() {
                        "Unsaved changes -- closing is blocked. Try the X or Alt+F4."
                    } else {
                        "Saved -- closing works normally."
                    }}
                </p>
                <button
                    class="window-button"
                    onclick={move || toggle_unsaved.set(!toggle_unsaved.get())}
                >
                    {if unsaved.get() { "Mark saved" } else { "Mark unsaved" }}
                </button>
            </div>
        </div>
    }
}

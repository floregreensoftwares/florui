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
use florui_style::Rgba;

const CSS: &str = include_str!("custom_titlebar.css");

fn main() {
    florui_platform::run_with_options(
        "Florui custom title bar",
        CSS,
        Rgba::opaque(0x1e, 0x1e, 0x22),
        WindowOptions {
            decorations: DecorationMode::Custom,
        },
        title_bar_demo,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn title_bar_demo() -> Element {
    let controls = use_window_controls();
    let maximized = controls.as_ref().is_some_and(|c| c.is_maximized());

    let minimize = controls.clone();
    let toggle_maximize = controls.clone();
    let close = controls.clone();

    view! {
        <div class="app">
            <div class="titlebar">
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
                    {"Drag this window from the empty space in the title bar. The buttons \
                      on the right are real: minimize, maximize/restore, and close all drive \
                      the actual window through WindowControls, not the OS's own chrome."}
                </p>
            </div>
        </div>
    }
}

//! A real native (OS-drawn) context menu, triggered from a button click.
//!
//! florui's own component event system has no right-click-specific event
//! yet — `DesktopHost` only ever dispatches `MouseButton::Left` into the
//! component tree, so `WindowControls::show_context_menu` (which works
//! from any UI-thread call, regardless of trigger) is demonstrated here
//! from an ordinary button instead. Wiring a real right-click-anywhere
//! trigger is a legitimate follow-up, not part of this slice.
//!
//! `cargo run --example context_menu -p florui-example-app`

use florui::prelude::*;
use florui_platform::{ContextMenuOutcome, MenuCommandId, MenuEntry, use_window_controls};
use florui_reactive::use_signal;
use florui_style::Rgba;

const CSS: &str = include_str!("context_menu.css");

const SAVE: MenuCommandId = MenuCommandId(1);
const SAVE_AS: MenuCommandId = MenuCommandId(2);
const AUTOSAVE: MenuCommandId = MenuCommandId(3);

fn main() {
    florui_platform::run(
        "Florui context menu",
        CSS,
        Rgba::opaque(0x1e, 0x1e, 0x22),
        root,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn root() -> Element {
    let controls = use_window_controls();
    let autosave = use_signal(|| false);
    let last_outcome = use_signal(|| "no selection yet".to_owned());

    let show_menu = controls.clone();
    let autosave_state = autosave.clone();
    let last_outcome_click = last_outcome.clone();

    view! {
        <div class="app">
            <p class="body-text">
                {"Right-click support inside the component tree doesn't exist \
                  yet -- click the button below to show a real native context \
                  menu instead."}
            </p>
            <button
                class="menu-button"
                onclick={move || {
                    let Some(controls) = &show_menu else { return; };
                    let items = vec![
                        MenuEntry::Action {
                            id: SAVE,
                            label: "Save".to_owned(),
                            enabled: true,
                            checked: false,
                            shortcut: Some("Ctrl+S".to_owned()),
                        },
                        MenuEntry::Action {
                            id: SAVE_AS,
                            label: "Save As... (disabled)".to_owned(),
                            enabled: false,
                            checked: false,
                            shortcut: None,
                        },
                        MenuEntry::Separator,
                        MenuEntry::Action {
                            id: AUTOSAVE,
                            label: "Autosave".to_owned(),
                            enabled: true,
                            checked: autosave_state.get(),
                            shortcut: None,
                        },
                    ];
                    let outcome = controls.show_context_menu(&items);
                    let text = match outcome {
                        ContextMenuOutcome::Selected(id) if id == SAVE => "Selected: Save".to_owned(),
                        ContextMenuOutcome::Selected(id) if id == AUTOSAVE => {
                            autosave_state.set(!autosave_state.get());
                            "Selected: Autosave (toggled)".to_owned()
                        }
                        ContextMenuOutcome::Selected(MenuCommandId(id)) => {
                            format!("Selected: unknown id {id}")
                        }
                        ContextMenuOutcome::Dismissed => "Dismissed, no selection".to_owned(),
                        ContextMenuOutcome::Unavailable => "Unavailable on this platform".to_owned(),
                    };
                    last_outcome_click.set(text);
                }}
            >
                {"Show context menu"}
            </button>
            <p class="status">
                {format!("Autosave: {}", if autosave.get() { "ON" } else { "OFF" })}
            </p>
            <p class="status">
                {format!("Last outcome: {}", last_outcome.get())}
            </p>
        </div>
    }
}

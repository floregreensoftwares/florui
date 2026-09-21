//! Real keyboard focus, live: Tab/Shift-Tab move focus between four real
//! buttons (document order, wrapping at either end), and Enter/Space
//! activate whichever one is currently focused -- the same real
//! `onclick` handler a mouse click already fires. `:focus` and
//! `:focus-visible` are real CSS matching real state, not a simulated
//! highlight: tabbing to a button lights up both; clicking one with the
//! mouse lights up only `:focus`, not `:focus-visible` -- the same
//! distinction real HTML draws.
//!
//! `cargo run --example keyboard_focus -p florui-example-app`

use florui::prelude::*;
use florui_reactive::use_signal;
use florui_style::Rgba;

const KEYBOARD_FOCUS_CSS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/examples/keyboard_focus.css");

fn main() {
    florui_platform::run_with_css_reload(
        "Florui keyboard focus",
        KEYBOARD_FOCUS_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        app,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn app() -> Element {
    let last_activated = use_signal(|| "none yet".to_string());
    let click_a = last_activated.clone();
    let click_b = last_activated.clone();
    let click_c = last_activated.clone();
    let click_d = last_activated.clone();

    view! {
        <div class="page">
            <p class="instructions">
                {"Tab / Shift+Tab moves focus. Enter or Space activates."}
            </p>
            <p class="status">{format!("Last activated: {}", last_activated.get())}</p>
            <div class="row">
                <button class="action" onclick={move || click_a.set("A".to_string())}>
                    {"A"}
                </button>
                <button class="action" onclick={move || click_b.set("B".to_string())}>
                    {"B"}
                </button>
                <button class="action" onclick={move || click_c.set("C".to_string())}>
                    {"C"}
                </button>
                <button class="action" onclick={move || click_d.set("D".to_string())}>
                    {"D"}
                </button>
            </div>
        </div>
    }
}

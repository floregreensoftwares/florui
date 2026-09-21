//! A real disabled button, live: "Disable target" flips the `disabled`
//! attribute on the button next to it. While disabled: a real author
//! `:disabled` rule dims it, its own `onclick` never fires, Tab skips it
//! entirely, and disabling it while it's focused moves focus off it --
//! all real behavior, not a simulated look.
//!
//! `cargo run --example disabled_button -p florui-example-app`

use florui::prelude::*;
use florui_reactive::use_signal;
use florui_style::Rgba;

const DISABLED_BUTTON_CSS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/examples/disabled_button.css");

fn main() {
    florui_platform::run_with_css_reload(
        "Florui disabled state",
        DISABLED_BUTTON_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        app,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn app() -> Element {
    let disabled = use_signal(|| false);
    let toggle = disabled.clone();
    let clicks = use_signal(|| 0);
    let on_click = clicks.clone();

    view! {
        <div class="page">
            <p class="instructions">
                {"Tab / Shift+Tab moves focus; the target is skipped while disabled."}
            </p>
            <p class="status">{format!("Target clicked {} time(s)", clicks.get())}</p>
            <div class="row">
                <button class="action" onclick={move || toggle.set(!toggle.get())}>
                    {if disabled.get() { "Enable target" } else { "Disable target" }}
                </button>
                <button
                    class="action target"
                    disabled={disabled.get()}
                    onclick={move || on_click.set(on_click.get() + 1)}
                >
                    {"Target"}
                </button>
            </div>
        </div>
    }
}

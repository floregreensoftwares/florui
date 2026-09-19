//! Three real windows, all sharing one stylesheet that colors itself
//! entirely through `@media (prefers-color-scheme: dark)`:
//!
//! - "System" follows the real OS light/dark preference live -- toggle
//!   Windows' own "Choose your mode" setting (Settings -> Personalization
//!   -> Colors) while it's open and watch it update, no restart needed.
//! - "Forced Light"/"Forced Dark" use `WindowOptions.theme` to override
//!   regardless of the OS setting, and stay immune to the same live
//!   toggle -- proving the override actually overrides, not just starts
//!   differently.
//!
//! `cargo run --example theme_test -p florui-example-app`

use florui::prelude::*;
use florui_platform::theme::ThemePreference;
use florui_platform::{WindowOptions, WindowSpec, run_windows};
use florui_style::Rgba;

const CSS: &str = include_str!("theme_test.css");

fn main() {
    let system = WindowSpec::new(
        "Florui theme test -- System (follows the real OS setting)",
        CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        WindowOptions::default(),
        page,
    )
    .expect("this example's own CSS must be valid");

    let forced_light = WindowSpec::new(
        "Florui theme test -- Forced Light",
        CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        WindowOptions {
            theme: ThemePreference::Light,
            ..WindowOptions::default()
        },
        page,
    )
    .expect("this example's own CSS must be valid");

    let forced_dark = WindowSpec::new(
        "Florui theme test -- Forced Dark",
        CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        WindowOptions {
            theme: ThemePreference::Dark,
            ..WindowOptions::default()
        },
        page,
    )
    .expect("this example's own CSS must be valid");

    run_windows(vec![system, forced_light, forced_dark])
        .expect("event loop should not fail on a real desktop session");
}

fn page() -> Element {
    view! {
        <div class="page">
            <p class="label">{"@media (prefers-color-scheme: dark)"}</p>
            <p class="hint">
                {"This window's own background and text color come entirely from that \
                  real media query -- nothing else in this component decides them."}
            </p>
        </div>
    }
}

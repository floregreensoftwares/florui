//! A visual check for explicit style scoping: two components declared
//! through separate `stylesheet_scoped!` calls both use the local class
//! name `.box`/`.label` -- render them side by side and see two distinct
//! colors, not a collision.
//!
//! Bypasses the app's normal `build.rs`/`FLORUI_STYLESHEETS` collection
//! pipeline (like this project's other hand-written examples), and so
//! also bypasses `florui_platform::run`'s single-CSS-string shape:
//! `florui_style::compile_sources` needs each `StylesheetSource` kept
//! separate, since a scope boundary is a property of one particular
//! source, not something a concatenated string could represent.
//!
//! `cargo run --example scope_test -p florui-example-app`

use florui_example_app::components::scope_test::blue_card;
use florui_example_app::components::scope_test::red_card;
use florui_example_app::components::scope_test::{ScopeTest, ScopeTestProps};
use florui_platform::{WindowOptions, WindowSpec};
use florui_style::Rgba;

fn main() {
    let sources = [
        florui_example_app::components::scope_test::__FLORUI_STYLESHEET,
        red_card::__FLORUI_STYLESHEET,
        blue_card::__FLORUI_STYLESHEET,
    ];
    let rules = florui_style::compile_sources(&sources)
        .expect("this example's own CSS fixtures must be valid");

    let spec = WindowSpec::with_rules(
        "Florui explicit style scoping",
        rules,
        Rgba::opaque(0x10, 0x10, 0x14),
        WindowOptions::default(),
        || ScopeTest(ScopeTestProps {}),
    );

    florui_platform::run_windows(vec![spec])
        .expect("event loop should not fail on a real desktop session");
}

//! The same counter app as `counter.rs`, paired with the real inspector
//! window: click any element in either window to select it, watch its real
//! resolved style and box model, and the counter still works normally
//! while being inspected.
//!
//! `cargo run --example inspector -p florui-example-app`

use florui_example_app::components::counter::{Counter, CounterProps};
use florui_style::Rgba;

const COUNTER_CSS: &str = include_str!("../src/components/counter.css");

fn main() {
    florui_devtools::live::run(
        "Florui counter",
        COUNTER_CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        || Counter(CounterProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

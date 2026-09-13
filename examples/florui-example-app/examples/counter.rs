//! A clickable counter, live: `+1` is a real `onclick` handler calling
//! `Signal::set` directly. `florui_platform::run` owns the window and
//! event loop entirely — this is just the component and its stylesheet.
//!
//! `cargo run --example counter -p florui-example-app`

use florui_example_app::components::counter::{Counter, CounterProps};
use florui_style::Rgba;

const COUNTER_CSS: &str = include_str!("../src/components/counter.css");

fn main() {
    florui_platform::run(
        "Florui counter",
        COUNTER_CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        || Counter(CounterProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

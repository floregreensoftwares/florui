//! A clickable counter, live: `+1` is a real `onclick` handler calling
//! `Signal::set` directly. `florui_platform::run_with_css_reload` owns the
//! window and event loop entirely and watches `counter.css` on disk —
//! editing and saving it restyles the window without resetting the click
//! count, since only the stylesheet changes, never the component's own
//! state. This is the proof of that: click a few times, then edit
//! `counter.css` and save — the count stays put.
//!
//! `cargo run --example counter -p florui-example-app`

use florui_example_app::components::counter::{Counter, CounterProps};
use florui_style::Rgba;

const COUNTER_CSS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/components/counter.css");

fn main() {
    florui_platform::run_with_css_reload(
        "Florui counter",
        COUNTER_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        || Counter(CounterProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

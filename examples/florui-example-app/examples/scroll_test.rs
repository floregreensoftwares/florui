//! A real, generic scrollable container: 40 rows inside a fixed-height
//! `overflow-y: auto` box, scrolled with a real mouse wheel or trackpad.
//! The status line above it reads back the live offset via
//! `use_scroll_offset`'s own `on_scroll` callback, a way to confirm
//! scrolling happened beyond eyeballing the rows move.
//!
//! Lives entirely here, not under `src/components/`, since it needs
//! `florui_platform::use_scroll_offset` directly -- every component under
//! `src/components/` stays platform-agnostic (only `florui`/
//! `florui-reactive`), and `florui-platform` is only a dev-dependency of
//! this crate, reachable from its own tests/examples/benches, not its lib.
//!
//! `cargo run --example scroll_test -p florui-example-app`

use florui::prelude::*;
use florui_platform::use_scroll_offset;
use florui_reactive::use_signal;
use florui_style::Rgba;

const CSS: &str = include_str!("scroll_test.css");

fn main() {
    florui_platform::run(
        "Florui scroll test",
        CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        widget,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn widget() -> Element {
    let offset = use_signal(|| (0.0f32, 0.0f32));
    let reported = offset.clone();
    // "scroll-box" below must carry this same id -- real wheel scrolling
    // only reaches an element with a live `use_scroll_offset` registration
    // for its own id, not merely a matching `overflow-y: auto`.
    use_scroll_offset("scroll-box", move |x, y| reported.set((x, y)));

    let (x, y) = offset.get();
    view! {
        <div class="page">
            <div class="status">{format!("scroll offset: ({x:.0}, {y:.0})")}</div>
            <div id="scroll-box" class="scroll-box">
                {(0..40).map(|i| view! {
                    <div class="row">{format!("Row {i}")}</div>
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}

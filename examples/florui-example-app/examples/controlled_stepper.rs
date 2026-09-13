//! A controlled input, live: this example owns the `Signal`, builds a
//! `Binding` around it with its own validation (clamped to 0..=10, so it
//! can actually reject a request), and hands only the `Binding` to
//! `Stepper` — the component never sees the `Signal` itself. Click past
//! either end and nothing happens, because the owner rejected it, not
//! because the button silently did nothing.
//!
//! `cargo run --example controlled_stepper -p florui-example-app`

use florui_example_app::components::stepper::{Stepper, StepperProps};
use florui_reactive::{Binding, use_signal};
use florui_style::Rgba;

const STEPPER_CSS: &str = include_str!("../src/components/stepper.css");

fn main() {
    florui_platform::run(
        "Florui controlled stepper",
        STEPPER_CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        || {
            let count = use_signal(|| 0);
            let value = Binding::new(count.get(), move |requested: i32| {
                if (0..=10).contains(&requested) {
                    count.set(requested);
                }
            });
            Stepper(StepperProps { value })
        },
    )
    .expect("event loop should not fail on a real desktop session");
}

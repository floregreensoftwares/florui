//! A visual check for real CSS `transition`/`@keyframes` support: a
//! hover-triggered row (background-color, transform, and opacity
//! transitions with different timing functions) and a continuously
//! animated row (opacity pulse, translateX slide, rotate, and a
//! multi-stop background-color cycle) -- all driven by Stylo's own
//! animation engine, sampled against a real wall clock.
//!
//! `cargo run --example motion_test -p florui-example-app`

use florui_example_app::components::motion_test::{MotionTest, MotionTestProps};
use florui_style::Rgba;

const MOTION_TEST_CSS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/components/motion_test.css"
);

fn main() {
    let css = std::fs::read_to_string(MOTION_TEST_CSS_PATH)
        .expect("motion_test.css should be readable next to this example's own component");
    florui_platform::run(
        "Florui motion test",
        &css,
        Rgba::opaque(0x10, 0x10, 0x14),
        || MotionTest(MotionTestProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

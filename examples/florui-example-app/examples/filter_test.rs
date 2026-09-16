//! A visual check for the `filter` property: ten boxes, each starting
//! from the same purple fill, with a different `filter` declaration --
//! blur, brightness (dimmed and brightened), contrast (flattened and
//! boosted), saturate (desaturated and oversaturated), an ordered chain
//! of two functions, and an unsupported `grayscale()` that should render
//! identically to "none".
//!
//! `cargo run --example filter_test -p florui-example-app`

use florui_example_app::components::filter_test::{FilterTest, FilterTestProps};
use florui_style::Rgba;

const FILTER_TEST_CSS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/components/filter_test.css");

fn main() {
    let css = std::fs::read_to_string(FILTER_TEST_CSS_PATH)
        .expect("filter_test.css should be readable next to this example's own component");
    florui_platform::run(
        "Florui filter test",
        &css,
        Rgba::opaque(0x10, 0x10, 0x14),
        || FilterTest(FilterTestProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

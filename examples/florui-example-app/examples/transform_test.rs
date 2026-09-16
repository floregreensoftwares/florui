//! A visual check for the `transform` property: eleven boxes, each with a
//! different `transform` declaration (translate, scale, rotate, matrix,
//! composed pairs in both orders, opacity combined with a transform, and
//! an unsupported `skewX()` that should render identically to "none").
//! Each box carries a red L-shaped marker on its own top-left corner so a
//! rotation's direction and pivot are visible at a glance, not just its
//! silhouette.
//!
//! `cargo run --example transform_test -p florui-example-app`

use florui_example_app::components::transform_test::{TransformTest, TransformTestProps};
use florui_style::Rgba;

const TRANSFORM_TEST_CSS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/components/transform_test.css"
);

fn main() {
    let css = std::fs::read_to_string(TRANSFORM_TEST_CSS_PATH)
        .expect("transform_test.css should be readable next to this example's own component");
    florui_platform::run(
        "Florui transform test",
        &css,
        Rgba::opaque(0x10, 0x10, 0x14),
        || TransformTest(TransformTestProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

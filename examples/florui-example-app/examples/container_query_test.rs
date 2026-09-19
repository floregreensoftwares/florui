//! A visual check for size container queries: an outer box whose width
//! follows the real window, and an inner box nested inside it with its own
//! fixed 300px width -- both share the same `.card` rule, each gray by
//! default and green once its own `@container (min-width: 500px)`
//! matches. Resize the window to see the outer one flip live; the inner
//! one never does, since its own containment is scoped to its own fixed
//! width, not the window or its outer ancestor.
//!
//! `cargo run --example container_query_test -p florui-example-app`

use florui_example_app::components::container_query_test::{
    ContainerQueryTest, ContainerQueryTestProps,
};
use florui_style::Rgba;

const CONTAINER_QUERY_TEST_CSS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/components/container_query_test.css"
);

fn main() {
    let css = std::fs::read_to_string(CONTAINER_QUERY_TEST_CSS_PATH)
        .expect("container_query_test.css should be readable next to this example's own component");
    florui_platform::run(
        "Florui container query test",
        &css,
        Rgba::opaque(0x10, 0x10, 0x14),
        || ContainerQueryTest(ContainerQueryTestProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

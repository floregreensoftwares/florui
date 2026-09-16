//! A visual check for `min-height`/`max-height` media queries: three
//! boxes, each gray by default, that turn green once the real window
//! height satisfies their own `@media` condition -- resize the window
//! to see each one flip live.
//!
//! `cargo run --example media_query_height_test -p florui-example-app`

use florui_example_app::components::media_query_height_test::{
    MediaQueryHeightTest, MediaQueryHeightTestProps,
};
use florui_style::Rgba;

const MEDIA_QUERY_HEIGHT_TEST_CSS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/components/media_query_height_test.css"
);

fn main() {
    let css = std::fs::read_to_string(MEDIA_QUERY_HEIGHT_TEST_CSS_PATH).expect(
        "media_query_height_test.css should be readable next to this example's own component",
    );
    florui_platform::run(
        "Florui media query height test",
        &css,
        Rgba::opaque(0x10, 0x10, 0x14),
        || MediaQueryHeightTest(MediaQueryHeightTestProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

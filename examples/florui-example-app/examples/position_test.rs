//! A visual check for real `position: relative`/`absolute` + `top`/
//! `right`/`bottom`/`left`, live:
//!
//! - The gray "container" is `position: relative`, establishing the
//!   positioning context the blue "absolute" box resolves its own
//!   `top`/`left` against.
//! - The blue box is pulled 20px down and 20px right from the
//!   container's own content-box origin -- and is *removed from normal
//!   flow* while it's at it: the green "sibling" box directly beneath it
//!   in source order does not get pushed down to make room, exactly as
//!   real CSS behaves.
//! - The overlapping red/orange pair demonstrates `z-index` now having a
//!   real effect on a `position: relative` element outside any flex/grid
//!   container: orange is earlier in source order but wins because of
//!   its higher `z-index`.
//!
//! `cargo run --example position_test -p florui-example-app`

use florui::prelude::*;
use florui_style::Rgba;

const POSITION_TEST_CSS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/examples/position_test.css");

fn main() {
    florui_platform::run_with_css_reload(
        "Florui position test",
        POSITION_TEST_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        app,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn app() -> Element {
    view! {
        <div class="page">
            <p class="instructions">
                {"Blue is pulled 20px down/right from the gray container's own origin, and \
                  doesn't push the green sibling down. Orange wins the overlap despite \
                  being earlier in source order, purely from z-index."}
            </p>
            <div class="container">
                <div class="absolute"></div>
            </div>
            <div class="sibling"></div>
            <div class="overlap-stage">
                <div class="orange"></div>
                <div class="red"></div>
            </div>
        </div>
    }
}

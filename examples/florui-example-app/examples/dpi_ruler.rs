//! A visual check for HiDPI viewport scaling: four boxes at fixed logical
//! sizes (50/100/150/200px), each labeled with its own size. Resize the
//! window, change Windows' display scaling (Settings > Display > Scale),
//! or drag it to a different-DPI monitor — the boxes should keep looking
//! the same physical size the whole time, updating immediately (no click
//! needed) since `DesktopHost` now handles `ScaleFactorChanged` directly.
//!
//! `cargo run --example dpi_ruler -p florui-example-app`

use florui_example_app::components::dpi_ruler::{DpiRuler, DpiRulerProps};
use florui_style::Rgba;

const DPI_RULER_CSS_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/components/dpi_ruler.css");

fn main() {
    let css = std::fs::read_to_string(DPI_RULER_CSS_PATH)
        .expect("dpi_ruler.css should be readable next to this example's own component");
    florui_platform::run(
        "Florui DPI ruler",
        &css,
        Rgba::opaque(0x10, 0x10, 0x14),
        || DpiRuler(DpiRulerProps {}),
    )
    .expect("event loop should not fail on a real desktop session");
}

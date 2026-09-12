//! Renders this app's actual `Card` component through the real
//! style+layout pipeline (`florui-style` cascade, `florui-layout`'s Taffy
//! block layout) and paints every node at its real computed position and
//! size via `florui_devtools::layout_capture` — no window, no fixed-margin
//! stand-in. Run with: `cargo run --example visual -p florui-example-app`

use florui_devtools::layout_capture;
use florui_example_app::components::card::{Card, CardProps};
use florui_style::{Arena, InteractionState, Rgba};
use taffy::prelude::*;

const CARD_CSS: &str = include_str!("../src/components/card.css");
const BUTTON_CSS: &str = include_str!("../src/components/button.css");

fn main() {
    let tree = Card(CardProps {
        title: "Florui".to_string(),
    });

    let css = format!("{CARD_CSS}\n{BUTTON_CSS}");
    let rules = florui_style::parse_stylesheet(&css)
        .expect("both stylesheets should parse under florui-style's supported subset");

    let arena = Arena::build(&tree);
    let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
    let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT)
        .expect("layout should not fail for explicitly sized nodes");

    let path = std::env::temp_dir().join("florui-example-app-card.png");
    layout_capture::paint_to_png(
        &path,
        400,
        260,
        Rgba::opaque(0x10, 0x10, 0x14),
        &arena,
        &styles,
        &layouts,
    )
    .expect("writing the capture should not fail");

    println!("wrote {}", path.display());
}

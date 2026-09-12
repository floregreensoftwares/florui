//! Renders this app's actual `Card` component to a PNG through
//! `element_scene`/`capture_to_png` — no window, just proof of what it
//! looks like. Run with: `cargo run --example visual -p florui-example-app`

use florui_devtools::color::Rgba;
use florui_devtools::{capture, element_scene};
use florui_example_app::components::card::{Card, CardProps};

fn main() {
    let tree = Card(CardProps {
        title: "Florui".to_string(),
    });

    let scene = element_scene::from_element(&tree, Rgba::opaque(0, 0, 0));
    let path = std::env::temp_dir().join("florui-example-app-card.png");

    capture::capture_to_png(
        &path,
        400,
        260,
        scene.canvas,
        scene.element.unwrap_or(scene.canvas),
    )
    .expect("writing the capture should not fail");

    println!("wrote {}", path.display());
}

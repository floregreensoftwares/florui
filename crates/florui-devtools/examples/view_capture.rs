//! Builds an element tree with `view!`/`#[component]`, reads its background
//! colors through `element_scene`, and writes an offscreen PNG — proving
//! the path from the component macros to an actual rendered file, end to
//! end, without opening a window.
//!
//! Run with: `cargo run --example view_capture -p florui-devtools`

use florui::prelude::*;
use florui_devtools::color::Rgba;
use florui_devtools::{capture, element_scene};

#[component]
fn Card(background: String, accent: String) -> Element {
    view! {
        <div style={format!("background-color: {background};")}>
            <div style={format!("background-color: {accent};")} />
        </div>
    }
}

fn main() {
    let tree = render_once(|| {
        Card(CardProps {
            background: "#1e1e22".to_string(),
            accent: "#42734f".to_string(),
        })
    });

    let scene = element_scene::from_element(&tree, Rgba::opaque(0, 0, 0));
    let path = std::env::temp_dir().join("florui-view-capture.png");

    capture::capture_to_png(
        &path,
        320,
        240,
        scene.canvas,
        scene.element.unwrap_or(scene.canvas),
    )
    .expect("writing the capture should not fail");

    println!("wrote {}", path.display());
}

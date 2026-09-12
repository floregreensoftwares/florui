//! Opens a real window showing an element tree built with
//! `view!`/`#[component]` — the live counterpart to `view_capture`.
//!
//! Run with: `cargo run --example view_preview -p florui-devtools`

use florui::prelude::*;
use florui_devtools::color::Rgba;
use florui_devtools::{element_preview, element_scene};

#[component]
fn Card(background: String, accent: String) -> Element {
    view! {
        <div style={format!("background-color: {background};")}>
            <div style={format!("background-color: {accent};")} />
        </div>
    }
}

fn main() {
    let tree = Card(CardProps {
        background: "#1e1e22".to_string(),
        accent: "#42734f".to_string(),
    });

    let scene = element_scene::from_element(&tree, Rgba::opaque(0, 0, 0));

    if let Err(err) = element_preview::run(scene) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

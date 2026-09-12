//! A real, multi-module application exercising `view!`, `#[component]`,
//! and `stylesheet!` together, with a `build.rs` that discovers every
//! declared stylesheet across the module graph in deterministic cascade
//! order — proving that pipeline against actual `cargo build`, not just
//! the `florui-build` crate's own unit tests.
//!
//! Components live in the library target (`src/lib.rs`), which is also
//! where `build.rs` roots its module-graph walk; this binary is a thin
//! entry point over it. See `examples/visual.rs` for a rendered capture of
//! the same tree.

use florui_example_app::components::card::{Card, CardProps};

include!(concat!(env!("OUT_DIR"), "/florui_stylesheets.rs"));

fn main() {
    let tree = Card(CardProps {
        title: "Florui".to_string(),
    });
    println!("Element tree:\n{tree:#?}\n");

    println!("Deterministic cascade order:");
    for sheet in FLORUI_STYLESHEETS {
        println!("  {} <- {}", sheet.id, sheet.source_path);
    }
}

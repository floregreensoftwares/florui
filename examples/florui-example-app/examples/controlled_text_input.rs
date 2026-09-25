//! Real single-line text editing, live: caret positioning (click, drag
//! to select, double-click selects a word), Ctrl+A/C/X/V against the
//! real OS clipboard, and Ctrl+Z/Ctrl+Shift+Z undo/redo.
//!
//! Two fields, the two authoring contracts slots-and-bindings.md's
//! "Optional convenience and explicit control" both require: "Name"
//! carries a real `Binding<String>` that rejects anything over 12
//! characters (so a real "the owner rejected this" resync is visibly
//! exercised, not just accepted edits); "Nickname" carries a plain value
//! plus `oninput` instead, with no `Binding` anywhere in its own code.
//! Both sibling `<span>`s below only update once the owner actually
//! accepts a change — never on every keystroke, and never on a rejected
//! one.
//!
//! `cargo run --example controlled_text_input -p florui-example-app`

use florui::prelude::*;
use florui_style::Rgba;

const CONTROLLED_TEXT_INPUT_CSS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/controlled_text_input.css"
);

const MAX_NAME_LEN: usize = 12;

fn main() {
    florui_platform::run_with_css_reload(
        "Florui controlled text input",
        CONTROLLED_TEXT_INPUT_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        app,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn app() -> Element {
    let name = use_signal(|| "Ada".to_string());
    let name_for_binding = name.clone();
    let name_binding = Binding::new(name.get(), move |requested: String| {
        if requested.chars().count() <= MAX_NAME_LEN {
            name_for_binding.set(requested);
        }
    });

    let nickname = use_signal(|| "Grace".to_string());
    let nickname_for_input = nickname.clone();

    view! {
        <div class="page">
            <p class="instructions">
                {"Click to place the caret, drag or double-click to select, Ctrl+A/C/X/V, \
                  Ctrl+Z / Ctrl+Shift+Z to undo/redo."}
            </p>

            <label class="field-label">{"Name (Binding, rejects over 12 characters)"}</label>
            <input
                id="name-input"
                class="text-field"
                type="text"
                value={name_binding}
            />
            <p class="status">{format!("Committed: {:?}", name.get())}</p>

            <label class="field-label">{"Nickname (explicit value + oninput, no Binding)"}</label>
            <input
                id="nickname-input"
                class="text-field"
                type="text"
                value={nickname.get()}
                oninput={move |value: String| nickname_for_input.set(value)}
            />
            <p class="status">{format!("Committed: {:?}", nickname.get())}</p>
        </div>
    }
}

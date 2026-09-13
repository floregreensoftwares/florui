//! Selectors, cascade, specificity, and inheritance against a
//! `florui::Element` tree — the "style" stage between elements and boxes
//! in the engine's pipeline (components → elements → **style** → boxes →
//! layout → text → painting).
//!
//! # Scope
//!
//! Supported, and meant to be complete within this boundary rather than a
//! partial slice of something larger:
//!
//! - Selectors: type (`div`), class (`.card`), ID (`#main`), compound
//!   selectors combining any of those (`button.primary#go`), the
//!   descendant combinator (`.card button`), comma-separated selector
//!   lists (`.a, .b { ... }`), and the state pseudo-classes `:hover`,
//!   `:focus`, `:active`.
//! - Real specificity ((id, class+pseudo-class, type) counts) and real
//!   cascade order (specificity first, then source order).
//! - Ten properties: `background-color`/`color` (colors; `color`
//!   inherits, `background-color` doesn't), `width`/`height` (pixel
//!   lengths or `auto`, initial `auto`, never inherit), `font-size` (pixel
//!   length only, initial `16px`, inherits), and the four
//!   `margin-*`/`padding-*` longhands (pixel lengths; margin also accepts
//!   `auto` for centering, padding does not; initial `0`; never inherit).
//!   All ten understand the `inherit`/`initial` keywords.
//!
//! Explicitly not supported, and rejected with a named error rather than
//! silently accepted or silently mismatched: attribute selectors, child
//! (`>`) and sibling (`+`/`~`) combinators, `:not()`/structural/other
//! pseudo-classes, the universal selector, any property besides the ten
//! above (including shorthand `margin`/`padding`), percentages, `!important`,
//! and CSS `@`-rules.
//!
//! `:hover`/`:focus`/`:active` are matched against an
//! [`InteractionState`](interaction::InteractionState) the caller builds
//! directly (see its docs) — there is no pointer/keyboard event system
//! here to derive it from yet.

mod cascade;
mod color;
mod error;
mod interaction;
mod matching;
mod selector;
mod selector_parse;
mod stylesheet_parse;
mod tree;
mod value;

pub use cascade::{ComputedStyle, Edges, compute};
pub use color::{ColorParseError, Rgba, parse_hex_color};
pub use error::StyleError;
pub use interaction::InteractionState;
pub use selector::{
    CompoundSelector, PseudoClass, Selector, SimpleSelector, Specificity, specificity_of,
};
pub use selector_parse::parse_selector_list;
pub use stylesheet_parse::{Declaration, Rule, parse_stylesheet};
pub use tree::{Arena, NodeId};
pub use value::{Property, Value, ValueKind};

#[cfg(test)]
mod integration_tests {
    use florui::prelude::*;

    use super::*;

    /// The stylesheets.md flagship example, styled for real: a class
    /// selector matching a view!-built button, with a real :hover cascade
    /// on top of it — end to end through this crate's public API only.
    #[test]
    fn styles_the_button_flagship_example_end_to_end() {
        #[component]
        fn Button(label: String) -> Element {
            view! { <button class="button">{label}</button> }
        }

        let css = "
            .button {
                background-color: #42734f;
                color: #ffffff;
            }
            .button:hover {
                background-color: #345c3e;
            }
        ";

        let tree = render_once(|| {
            Button(ButtonProps {
                label: "Open projects".to_string(),
            })
        });
        let arena = Arena::build(&tree);
        let rules = parse_stylesheet(css).unwrap();
        let button = arena.find(|a, id| a.tag(id) == "button").unwrap();

        let idle = compute(&arena, &rules, &InteractionState::new());
        assert_eq!(
            idle[&button].background_color,
            Rgba::opaque(0x42, 0x73, 0x4f)
        );
        assert_eq!(idle[&button].color, Rgba::opaque(0xff, 0xff, 0xff));

        let hovered_state = InteractionState::new().with_hovered(button);
        let hovered = compute(&arena, &rules, &hovered_state);
        assert_eq!(
            hovered[&button].background_color,
            Rgba::opaque(0x34, 0x5c, 0x3e)
        );
        assert_eq!(
            hovered[&button].color,
            Rgba::opaque(0xff, 0xff, 0xff),
            "hover rule doesn't touch color, so it stays"
        );
    }
}

//! Which nodes participate in keyboard focus traversal, and in what order.
//!
//! v1 is deliberately narrow: `<button>` and an editable `<input>`, not
//! disabled, document order, no `tabindex`. `<input type="checkbox"|
//! "radio">` stays out — this slice gives neither any real behavior at
//! all yet (see `crates/florui-platform/src/text_input.rs`'s own doc).
//! Extending this further (`a`/`select`/`textarea`) later is one more
//! clause here, not a redesign.

use florui_style::{Arena, NodeId};

/// `<input>` types this slice gives real text-editing behavior — the
/// same gate [`is_focusable`] and [`crate::text_input::TextInputRegistry`]
/// both check, so a `<input type="checkbox">` (parseable `view!` syntax,
/// but not part of this slice) never gets a focus stop or an editor.
pub(crate) fn is_editable_input_type(input_type: Option<&str>) -> bool {
    matches!(input_type, Some("text") | Some("password"))
}

pub(crate) fn is_focusable(arena: &Arena, id: NodeId) -> bool {
    if arena.is_disabled(id) {
        return false;
    }
    match arena.tag(id) {
        "button" => true,
        "input" => is_editable_input_type(arena.input_type(id)),
        _ => false,
    }
}

/// Every focusable node, in document order — both the tab-traversal
/// sequence and the candidate list [`florui_style::FocusPath::resolve`]
/// needs.
pub(crate) fn focus_order(arena: &Arena) -> Vec<NodeId> {
    arena.find_all(is_focusable)
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;

    #[test]
    fn focus_order_collects_only_buttons_in_document_order() {
        let tree: Element = view! {
            <div>
                <button>{"First"}</button>
                <span>{"Not focusable"}</span>
                <button>{"Second"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let order = focus_order(&arena);
        assert_eq!(order.len(), 2);
        assert_eq!(arena.text_content(order[0]), "First");
        assert_eq!(arena.text_content(order[1]), "Second");
    }

    #[test]
    fn a_span_is_not_focusable() {
        let tree: Element = view! { <span>{"text"}</span> };
        let arena = Arena::build(&tree);
        let span = arena.roots()[0];
        assert!(!is_focusable(&arena, span));
    }

    #[test]
    fn a_disabled_button_is_not_focusable() {
        let tree: Element = view! { <button disabled="true">{"Go"}</button> };
        let arena = Arena::build(&tree);
        assert!(!is_focusable(&arena, arena.roots()[0]));
    }

    #[test]
    fn an_editable_input_is_focusable() {
        let tree: Element = view! { <input type="text" value="hi" /> };
        let arena = Arena::build(&tree);
        assert!(is_focusable(&arena, arena.roots()[0]));
    }

    #[test]
    fn a_checkbox_input_is_not_focusable_in_this_slice() {
        let tree: Element = view! { <input type="checkbox" value="on" /> };
        let arena = Arena::build(&tree);
        assert!(!is_focusable(&arena, arena.roots()[0]));
    }

    #[test]
    fn a_disabled_input_is_not_focusable() {
        let tree: Element = view! { <input type="text" value="hi" disabled="true" /> };
        let arena = Arena::build(&tree);
        assert!(!is_focusable(&arena, arena.roots()[0]));
    }

    #[test]
    fn focus_order_excludes_disabled_buttons() {
        let tree: Element = view! {
            <div>
                <button>{"First"}</button>
                <button disabled="true">{"Second"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let order = focus_order(&arena);
        assert_eq!(order.len(), 1);
        assert_eq!(arena.text_content(order[0]), "First");
    }
}

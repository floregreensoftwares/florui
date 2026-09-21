//! Which nodes participate in keyboard focus traversal, and in what order.
//!
//! v1 is deliberately narrow: `<button>` only, not disabled, document
//! order, no `tabindex` — the only primitive with any real interactive
//! behavior today. Extending this to `a`/`input`/`select`/`textarea`
//! later is one more clause in [`is_focusable`], not a redesign.

use florui_style::{Arena, NodeId};

pub(crate) fn is_focusable(arena: &Arena, id: NodeId) -> bool {
    arena.tag(id) == "button" && !arena.is_disabled(id)
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

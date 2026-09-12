//! Matches a parsed [`Selector`] against a node in the [`Arena`], honoring
//! the descendant combinator and the current [`InteractionState`].

use crate::interaction::InteractionState;
use crate::selector::{CompoundSelector, Selector, SimpleSelector};
use crate::tree::{Arena, NodeId};

pub fn matches_selector(
    arena: &Arena,
    node: NodeId,
    selector: &Selector,
    state: &InteractionState,
) -> bool {
    let mut compounds = selector.0.iter().rev();
    let Some(target) = compounds.next() else {
        return false;
    };
    if !matches_compound(arena, node, target, state) {
        return false;
    }

    let mut current = node;
    for compound in compounds {
        let mut ancestor = arena.parent(current);
        let found = loop {
            match ancestor {
                Some(candidate) if matches_compound(arena, candidate, compound, state) => {
                    break Some(candidate);
                }
                Some(candidate) => ancestor = arena.parent(candidate),
                None => break None,
            }
        };
        match found {
            Some(candidate) => current = candidate,
            None => return false,
        }
    }
    true
}

fn matches_compound(
    arena: &Arena,
    node: NodeId,
    compound: &CompoundSelector,
    state: &InteractionState,
) -> bool {
    compound.0.iter().all(|simple| match simple {
        SimpleSelector::Type(name) => arena.tag(node) == name,
        SimpleSelector::Class(name) => arena.classes(node).iter().any(|c| c == name),
        SimpleSelector::Id(name) => arena.id_attr(node) == Some(name.as_str()),
        SimpleSelector::Pseudo(pseudo) => state.matches(*pseudo, node),
    })
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;
    use crate::selector_parse::parse_selector_list;

    fn selector(text: &str) -> Selector {
        parse_selector_list(text)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn matches_a_type_selector() {
        let tree: Element = view! { <div /> };
        let arena = Arena::build(&tree);
        assert!(matches_selector(
            &arena,
            arena.roots()[0],
            &selector("div"),
            &InteractionState::new()
        ));
        assert!(!matches_selector(
            &arena,
            arena.roots()[0],
            &selector("span"),
            &InteractionState::new()
        ));
    }

    #[test]
    fn matches_a_class_selector() {
        let tree: Element = view! { <div class="card highlighted" /> };
        let arena = Arena::build(&tree);
        let node = arena.roots()[0];
        assert!(matches_selector(
            &arena,
            node,
            &selector(".card"),
            &InteractionState::new()
        ));
        assert!(matches_selector(
            &arena,
            node,
            &selector(".highlighted"),
            &InteractionState::new()
        ));
        assert!(!matches_selector(
            &arena,
            node,
            &selector(".missing"),
            &InteractionState::new()
        ));
    }

    #[test]
    fn descendant_combinator_matches_any_depth_not_just_direct_children() {
        let tree: Element = view! {
            <div class="card">
                <div>
                    <button>{"Go"}</button>
                </div>
            </div>
        };
        let arena = Arena::build(&tree);
        let button = arena.find(|a, id| a.tag(id) == "button").unwrap();
        assert!(matches_selector(
            &arena,
            button,
            &selector(".card button"),
            &InteractionState::new()
        ));
    }

    #[test]
    fn descendant_combinator_requires_the_ancestor_to_exist() {
        let tree: Element = view! {
            <div>
                <button>{"Go"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let button = arena.find(|a, id| a.tag(id) == "button").unwrap();
        assert!(!matches_selector(
            &arena,
            button,
            &selector(".card button"),
            &InteractionState::new()
        ));
    }

    #[test]
    fn pseudo_class_only_matches_when_the_state_says_so() {
        let tree: Element = view! { <button>{"Go"}</button> };
        let arena = Arena::build(&tree);
        let button = arena.roots()[0];
        assert!(!matches_selector(
            &arena,
            button,
            &selector("button:hover"),
            &InteractionState::new()
        ));
        let hovered = InteractionState::new().with_hovered(button);
        assert!(matches_selector(
            &arena,
            button,
            &selector("button:hover"),
            &hovered
        ));
    }
}

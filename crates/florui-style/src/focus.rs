//! Structural identity for "the currently focused element" — stable
//! across renders, unlike [`NodeId`] itself.
//!
//! [`Arena::build`] reruns from scratch on every render and `NodeId` is
//! just preorder position, so it can't survive across renders the way
//! focus needs to (typing, async updates, anything). This mirrors
//! [`crate::animation`]'s own `PathKey` scheme (`{parent, tag, ordinal}`),
//! simplified: there's only ever one focused element at a time, so no
//! interning table or per-call GC is needed — just a path to walk back
//! down next render.

use crate::tree::{Arena, NodeId};

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSegment {
    tag: &'static str,
    ordinal: usize,
}

/// A focused element's identity, independent of any single [`Arena`]
/// generation. See the module doc for why [`NodeId`] alone can't serve
/// this role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusPath(Vec<PathSegment>);

impl FocusPath {
    /// Captures `id`'s path in `arena`, root to leaf.
    pub fn of(arena: &Arena, id: NodeId) -> Self {
        let mut segments = Vec::new();
        let mut current = Some(id);
        while let Some(node) = current {
            segments.push(PathSegment {
                tag: arena.tag(node),
                ordinal: Self::sibling_ordinal(arena, node),
            });
            current = arena.parent(node);
        }
        segments.reverse();
        Self(segments)
    }

    /// Finds the node in a fresh `arena` that this path still identifies,
    /// if any — a linear scan against `candidates` (the focusable set,
    /// not every node), once per render rather than a hot path.
    pub fn resolve(&self, arena: &Arena, candidates: &[NodeId]) -> Option<NodeId> {
        candidates
            .iter()
            .copied()
            .find(|&id| &Self::of(arena, id) == self)
    }

    /// Same tie-breaker [`crate::stylo`]'s `StyloTree::sibling_ordinal`
    /// uses: earlier same-tag siblings only, class-insensitive.
    fn sibling_ordinal(arena: &Arena, id: NodeId) -> usize {
        let siblings: &[NodeId] = match arena.parent(id) {
            Some(parent_id) => arena.children(parent_id),
            None => arena.roots(),
        };
        let tag = arena.tag(id);
        siblings
            .iter()
            .take_while(|&&sibling| sibling != id)
            .filter(|&&sibling| arena.tag(sibling) == tag)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;

    #[test]
    fn of_and_resolve_round_trip_across_a_fresh_arena() {
        let tree: Element = view! {
            <div>
                <button>{"First"}</button>
                <button>{"Second"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let second = arena.find_all(|a, id| a.tag(id) == "button")[1];
        let path = FocusPath::of(&arena, second);

        // A structurally identical, independently rebuilt arena — the
        // same relationship a real render-to-render `update()` has.
        let rebuilt = Arena::build(&tree);
        let candidates = rebuilt.find_all(|a, id| a.tag(id) == "button");
        let resolved = path.resolve(&rebuilt, &candidates).unwrap();
        assert_eq!(rebuilt.text_content(resolved), "Second");
    }

    #[test]
    fn resolve_returns_none_once_the_element_is_gone() {
        let tree: Element = view! {
            <div>
                <button>{"Only"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let button = arena.find_all(|a, id| a.tag(id) == "button")[0];
        let path = FocusPath::of(&arena, button);

        let empty: Element = view! { <div /> };
        let rebuilt = Arena::build(&empty);
        let candidates = rebuilt.find_all(|a, id| a.tag(id) == "button");
        assert_eq!(path.resolve(&rebuilt, &candidates), None);
    }

    #[test]
    fn distinct_positions_produce_distinct_paths() {
        let tree: Element = view! {
            <div>
                <button>{"First"}</button>
                <button>{"Second"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let buttons = arena.find_all(|a, id| a.tag(id) == "button");
        let first_path = FocusPath::of(&arena, buttons[0]);
        let second_path = FocusPath::of(&arena, buttons[1]);
        assert_ne!(first_path, second_path);
    }
}

//! An indexed, parent-linked view of a `florui::Element` tree.
//!
//! `Element` itself has no parent pointers (children are owned by value),
//! but descendant-combinator matching needs to walk upward — this builds
//! that view once, up front, rather than threading parent references
//! through `Element` itself.

use florui::{Element, ElementNode, Handler};

pub type NodeId = usize;

struct ArenaNode {
    tag: &'static str,
    classes: Vec<String>,
    id: Option<String>,
    /// The node's own direct text, for text measurement — not inherited
    /// from or propagated to any other node.
    text: String,
    handlers: Vec<(String, Handler)>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

/// A tree may have more than one root: `view!` can produce an
/// [`Element::Fragment`] at the top level, and a `Text`-only tree has
/// none at all (there is nothing selectable to style).
pub struct Arena {
    nodes: Vec<ArenaNode>,
    roots: Vec<NodeId>,
}

impl Arena {
    pub fn build(root: &Element) -> Self {
        let mut arena = Arena {
            nodes: Vec::new(),
            roots: Vec::new(),
        };
        arena.roots = arena.push(root, None);
        arena
    }

    fn push(&mut self, element: &Element, parent: Option<NodeId>) -> Vec<NodeId> {
        match element {
            Element::Node(node) => vec![self.push_node(node, parent)],
            Element::Fragment(children) => {
                children.iter().flat_map(|c| self.push(c, parent)).collect()
            }
            Element::Text(_) => Vec::new(),
        }
    }

    fn push_node(&mut self, node: &ElementNode, parent: Option<NodeId>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(ArenaNode {
            tag: node.tag,
            classes: class_list(&node.attrs),
            id: attr_value(&node.attrs, "id"),
            text: collect_text(&node.children),
            handlers: node.handlers.clone(),
            parent,
            children: Vec::new(),
        });
        let children: Vec<NodeId> = node
            .children
            .iter()
            .flat_map(|c| self.push(c, Some(id)))
            .collect();
        self.nodes[id].children = children;
        id
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    pub fn tag(&self, id: NodeId) -> &'static str {
        self.nodes[id].tag
    }

    pub fn classes(&self, id: NodeId) -> &[String] {
        &self.nodes[id].classes
    }

    pub fn id_attr(&self, id: NodeId) -> Option<&str> {
        self.nodes[id].id.as_deref()
    }

    /// This node's own direct text, for measurement: its direct
    /// [`Element::Text`] children concatenated in order, flattening
    /// through any [`Element::Fragment`] child but not descending into a
    /// child [`Element::Node`] — that child's text belongs to its own
    /// `NodeId`, not its parent's.
    pub fn text_content(&self, id: NodeId) -> &str {
        &self.nodes[id].text
    }

    /// The handler this node declared for `event` (e.g. `"click"` for an
    /// `onclick={...}` attribute), if any.
    pub fn handler(&self, id: NodeId, event: &str) -> Option<&Handler> {
        self.nodes[id]
            .handlers
            .iter()
            .find(|(name, _)| name == event)
            .map(|(_, handler)| handler)
    }

    /// Depth-first pre-order search across every root, for tests and
    /// callers that need to locate a node before marking it in an
    /// [`crate::InteractionState`].
    pub fn find(&self, mut predicate: impl FnMut(&Self, NodeId) -> bool) -> Option<NodeId> {
        fn walk(
            arena: &Arena,
            id: NodeId,
            predicate: &mut impl FnMut(&Arena, NodeId) -> bool,
        ) -> Option<NodeId> {
            if predicate(arena, id) {
                return Some(id);
            }
            arena
                .children(id)
                .iter()
                .find_map(|&child| walk(arena, child, predicate))
        }
        self.roots
            .iter()
            .find_map(|&root| walk(self, root, &mut predicate))
    }
}

fn class_list(attrs: &[(String, String)]) -> Vec<String> {
    attr_value(attrs, "class")
        .map(|value| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn attr_value(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn collect_text(children: &[Element]) -> String {
    let mut text = String::new();
    for child in children {
        append_text(child, &mut text);
    }
    text
}

fn append_text(element: &Element, out: &mut String) {
    match element {
        Element::Text(value) => out.push_str(value),
        Element::Fragment(children) => {
            for child in children {
                append_text(child, out);
            }
        }
        Element::Node(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;

    #[test]
    fn builds_parent_links_across_the_tree() {
        let tree: Element = view! {
            <div class="card">
                <h2>{"Title"}</h2>
                <button class="primary">{"Go"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        assert_eq!(arena.roots().len(), 1);
        let card = arena.roots()[0];
        assert_eq!(arena.tag(card), "div");
        assert_eq!(arena.classes(card), &["card".to_string()]);
        assert_eq!(arena.children(card).len(), 2);

        let button = arena.children(card)[1];
        assert_eq!(arena.tag(button), "button");
        assert_eq!(arena.parent(button), Some(card));
    }

    #[test]
    fn a_fragment_root_yields_multiple_roots_with_no_shared_parent() {
        let tree: Element = view! {
            <div style="background-color: #111;" />
            <div style="background-color: #222;" />
        };
        let arena = Arena::build(&tree);
        assert_eq!(arena.roots().len(), 2);
        assert_eq!(arena.parent(arena.roots()[0]), None);
    }

    #[test]
    fn text_nodes_are_not_addressable() {
        let tree: Element = view! { <p>{"hello"}</p> };
        let arena = Arena::build(&tree);
        let p = arena.roots()[0];
        assert!(arena.children(p).is_empty(), "text is not a styleable node");
    }

    #[test]
    fn id_attribute_is_read() {
        let tree: Element = view! { <div id="main" /> };
        let arena = Arena::build(&tree);
        assert_eq!(arena.id_attr(arena.roots()[0]), Some("main"));
    }

    #[test]
    fn find_locates_a_descendant_by_predicate() {
        let tree: Element = view! {
            <div>
                <button class="primary">{"Go"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let found = arena.find(|arena, id| arena.tag(id) == "button").unwrap();
        assert_eq!(arena.tag(found), "button");
    }

    #[test]
    fn text_content_concatenates_direct_text_children() {
        let tree: Element = view! { <h2>{"Hello"}</h2> };
        let arena = Arena::build(&tree);
        let h2 = arena.roots()[0];
        assert_eq!(arena.text_content(h2), "Hello");
    }

    #[test]
    fn text_content_is_empty_for_a_node_with_no_text_children() {
        let tree: Element = view! {
            <div>
                <span>{"x"}</span>
            </div>
        };
        let arena = Arena::build(&tree);
        let div = arena.roots()[0];
        assert_eq!(
            arena.text_content(div),
            "",
            "the text belongs to the span, not its ancestor"
        );
    }

    #[test]
    fn handler_finds_the_declared_event_by_name() {
        let tree: Element = view! { <button onclick={|| ()} /> };
        let arena = Arena::build(&tree);
        let button = arena.roots()[0];
        assert!(arena.handler(button, "click").is_some());
        assert!(arena.handler(button, "mouseenter").is_none());
    }

    #[test]
    fn text_content_flattens_through_a_fragment_but_skips_nested_nodes() {
        let tree = Element::node(
            "p",
            vec![],
            vec![Element::Fragment(vec![
                Element::text("Hello, "),
                Element::node("b", vec![], vec![Element::text("world")]),
                Element::text("!"),
            ])],
        );
        let arena = Arena::build(&tree);
        let p = arena.roots()[0];
        assert_eq!(
            arena.text_content(p),
            "Hello, !",
            "the <b>'s own text belongs to its own node, not its parent's"
        );
        let b = arena.children(p)[0];
        assert_eq!(arena.text_content(b), "world");
    }
}

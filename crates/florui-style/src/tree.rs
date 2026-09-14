//! An indexed, parent-linked view of a `florui::Element` tree.
//!
//! `Element` itself has no parent pointers (children are owned by value),
//! but descendant-combinator matching needs to walk upward — this builds
//! that view once, up front, rather than threading parent references
//! through `Element` itself.

use florui::{Element, ElementNode, Handler};

pub type NodeId = usize;

/// One direct child of a node, in original source order, for inline
/// layout: consecutive [`Element::Text`] siblings merge into one
/// [`InlineItem::Text`], the same way a real inline formatting context
/// treats adjacent text as one run rather than artificially splitting it
/// at whatever points the author happened to write separate string
/// literals. See [`Arena::inline_items`].
#[derive(Debug, Clone, PartialEq)]
pub enum InlineItem {
    Text(String),
    Element(NodeId),
}

struct ArenaNode {
    tag: &'static str,
    classes: Vec<String>,
    id: Option<String>,
    /// The node's own direct text, for text measurement — not inherited
    /// from or propagated to any other node.
    text: String,
    /// Same direct children as `text`/`children` below, but preserving the
    /// interleaving between them that both those flattened views lose —
    /// see [`Arena::inline_items`].
    inline_items: Vec<InlineItem>,
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
            inline_items: Vec::new(),
            handlers: node.handlers.clone(),
            parent,
            children: Vec::new(),
        });

        let mut children = Vec::new();
        let mut inline_items = Vec::new();
        self.push_children(&node.children, id, &mut children, &mut inline_items);
        self.nodes[id].children = children;
        self.nodes[id].inline_items = inline_items;
        id
    }

    /// Pushes `elements` (one node's direct children, as literally written)
    /// as this node's own children — flattening an [`Element::Fragment`]
    /// in place, the same way [`Self::push`] already does for a set of
    /// roots — while also building `inline_items` in the same pass, since
    /// only here (not in a separate text-only walk like [`collect_text`])
    /// does a nested [`Element::Node`] already have the [`NodeId`]
    /// `InlineItem::Element` needs to reference.
    fn push_children(
        &mut self,
        elements: &[Element],
        parent: NodeId,
        children: &mut Vec<NodeId>,
        inline_items: &mut Vec<InlineItem>,
    ) {
        for element in elements {
            match element {
                Element::Node(node) => {
                    let child_id = self.push_node(node, Some(parent));
                    children.push(child_id);
                    inline_items.push(InlineItem::Element(child_id));
                }
                Element::Fragment(nested) => {
                    self.push_children(nested, parent, children, inline_items);
                }
                Element::Text(value) => match inline_items.last_mut() {
                    Some(InlineItem::Text(existing)) => existing.push_str(value),
                    _ => inline_items.push(InlineItem::Text(value.clone())),
                },
            }
        }
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

    /// This node's direct children — text runs and element children — in
    /// their original interleaved source order, with an
    /// [`Element::Fragment`] child's own children spliced in at its
    /// position (the same flattening [`Self::text_content`]/
    /// [`Self::children`] already apply, just without losing which parts
    /// are text and which are elements, or their relative order). Unlike
    /// [`Self::text_content`], this is what real mixed inline content
    /// (`"Hello "<b>world</b>"!"`) needs to lay out correctly — see
    /// `florui_layout`'s own inline formatting context, which is the only
    /// consumer so far; nothing else needs to change what it reads.
    pub fn inline_items(&self, id: NodeId) -> &[InlineItem] {
        &self.nodes[id].inline_items
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

    #[test]
    fn inline_items_preserves_the_interleaved_order_text_content_loses() {
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
        let b = arena.children(p)[0];

        assert_eq!(
            arena.inline_items(p),
            &[
                InlineItem::Text("Hello, ".to_string()),
                InlineItem::Element(b),
                InlineItem::Text("!".to_string()),
            ],
            "text_content alone would report \"Hello, !\", losing where <b> sat"
        );
    }

    #[test]
    fn inline_items_merges_consecutive_text_runs_into_one_item() {
        // Two separate string literals that happen to be adjacent siblings
        // (e.g. from two interpolated `{...}` expressions in view!) are one
        // inline run, the same as a real inline formatting context treats
        // adjacent text nodes — not two zero-width-joined items.
        let tree = Element::node(
            "p",
            vec![],
            vec![Element::text("foo"), Element::text("bar")],
        );
        let arena = Arena::build(&tree);
        let p = arena.roots()[0];
        assert_eq!(
            arena.inline_items(p),
            &[InlineItem::Text("foobar".to_string())]
        );
    }

    #[test]
    fn inline_items_is_all_text_for_a_pure_text_node() {
        let tree: Element = view! { <h2>{"Hello"}</h2> };
        let arena = Arena::build(&tree);
        let h2 = arena.roots()[0];
        assert_eq!(
            arena.inline_items(h2),
            &[InlineItem::Text("Hello".to_string())]
        );
    }

    #[test]
    fn inline_items_is_all_elements_for_pure_element_children() {
        let tree: Element = view! {
            <div>
                <span>{"a"}</span>
                <span>{"b"}</span>
            </div>
        };
        let arena = Arena::build(&tree);
        let div = arena.roots()[0];
        let children = arena.children(div);
        assert_eq!(
            arena.inline_items(div),
            &[
                InlineItem::Element(children[0]),
                InlineItem::Element(children[1]),
            ]
        );
    }

    #[test]
    fn inline_items_is_empty_for_a_childless_node() {
        let tree: Element = view! { <div /> };
        let arena = Arena::build(&tree);
        assert!(arena.inline_items(arena.roots()[0]).is_empty());
    }
}

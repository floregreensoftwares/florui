//! An indexed, parent-linked view of a `florui::Element` tree.
//!
//! `Element` itself has no parent pointers (children are owned by value),
//! but descendant-combinator matching needs to walk upward — this builds
//! that view once, up front, rather than threading parent references
//! through `Element` itself.

use florui::{Element, Handler};

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
    /// Whether the `disabled` attribute is present and `"true"` — markup
    /// state, computed once here exactly like `id`/`classes`, not through
    /// [`crate::InteractionState`] (see `stylo.rs`'s own `StyloTree::new`
    /// for why).
    disabled: bool,
    /// The raw, unparsed `style="..."` attribute text, if declared — see
    /// [`Arena::style_attr`].
    style: Option<String>,
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
    /// How many of `roots`' leading entries are document roots — the
    /// rest (built by [`Self::build_with_overlays`], if any) are portal
    /// overlay roots. `roots()` itself stays one flat, ordered list on
    /// purpose: paint (later root paints on top, unclipped) and hit-test
    /// (later root wins) both already treat a later root as "above" an
    /// earlier one with no changes needed — only layout needs to tell
    /// the two groups apart, via [`Self::document_roots`]/
    /// [`Self::overlay_roots`].
    document_root_count: usize,
}

/// One still-unprocessed slice of sibling [`Element`]s, and where their
/// [`NodeId`]s attach — a real parent, or the root list.
struct Frame<'a> {
    elements: &'a [Element],
    index: usize,
    parent: Option<NodeId>,
}

impl Arena {
    pub fn build(root: &Element) -> Self {
        let mut arena = Arena {
            nodes: Vec::new(),
            roots: Vec::new(),
            document_root_count: 0,
        };
        arena.push_all(std::slice::from_ref(root));
        arena.document_root_count = arena.roots.len();
        arena
    }

    /// Like [`Self::build`], but `overlays` becomes a second group of
    /// roots — [`Self::overlay_roots`] — appended after `document`'s own.
    /// A portal-hosting runtime builds `overlays` from whatever its own
    /// portal registry collected this render; every other caller keeps
    /// using [`Self::build`], which leaves [`Self::overlay_roots`] empty.
    pub fn build_with_overlays(document: &Element, overlays: &Element) -> Self {
        let mut arena = Arena {
            nodes: Vec::new(),
            roots: Vec::new(),
            document_root_count: 0,
        };
        arena.push_all(std::slice::from_ref(document));
        arena.document_root_count = arena.roots.len();
        arena.push_all(std::slice::from_ref(overlays));
        arena
    }

    /// Iterative pre-order walk: an explicit stack instead of one call
    /// frame per tree level, so a deep tree can't overflow the stack.
    fn push_all(&mut self, root_elements: &[Element]) {
        let mut stack = vec![Frame {
            elements: root_elements,
            index: 0,
            parent: None,
        }];

        while let Some(frame) = stack.last_mut() {
            let Some(element) = frame.elements.get(frame.index) else {
                stack.pop();
                continue;
            };
            frame.index += 1;
            let parent = frame.parent;

            match element {
                Element::Node(node) => {
                    let id = self.nodes.len();
                    self.nodes.push(ArenaNode {
                        tag: node.tag,
                        classes: class_list(&node.attrs),
                        id: attr_value(&node.attrs, "id"),
                        disabled: attr_bool(&node.attrs, "disabled"),
                        style: attr_value(&node.attrs, "style"),
                        text: collect_text(&node.children),
                        inline_items: Vec::new(),
                        handlers: node.handlers.clone(),
                        parent,
                        children: Vec::new(),
                    });
                    match parent {
                        Some(p) => {
                            self.nodes[p].children.push(id);
                            self.nodes[p].inline_items.push(InlineItem::Element(id));
                        }
                        None => self.roots.push(id),
                    }
                    stack.push(Frame {
                        elements: &node.children,
                        index: 0,
                        parent: Some(id),
                    });
                }
                Element::Fragment(nested) => {
                    stack.push(Frame {
                        elements: nested,
                        index: 0,
                        parent,
                    });
                }
                Element::Text(value) => {
                    if let Some(p) = parent {
                        match self.nodes[p].inline_items.last_mut() {
                            Some(InlineItem::Text(existing)) => existing.push_str(value),
                            _ => self.nodes[p]
                                .inline_items
                                .push(InlineItem::Text(value.clone())),
                        }
                    }
                }
            }
        }
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// The ordinary document roots — everything [`Self::build`] always
    /// produces, and the leading part of [`Self::build_with_overlays`]'s
    /// own result.
    pub fn document_roots(&self) -> &[NodeId] {
        &self.roots[..self.document_root_count]
    }

    /// Portal overlay roots, if any — empty for anything built via
    /// [`Self::build`]. See [`Self::build_with_overlays`].
    pub fn overlay_roots(&self) -> &[NodeId] {
        &self.roots[self.document_root_count..]
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

    /// Whether this node's `disabled` attribute is present and `"true"` —
    /// e.g. `<button disabled={is_disabled}>` where `is_disabled: bool`
    /// lowers via `ToString` (see `view!`'s own attribute lowering).
    /// `false` for any other value or an absent attribute — bare
    /// `<button disabled>` isn't parseable `view!` syntax, so only an
    /// explicit `"true"`/`"false"` is ever seen in practice.
    pub fn is_disabled(&self, id: NodeId) -> bool {
        self.nodes[id].disabled
    }

    /// This node's raw, unparsed `style="..."` attribute text, if it
    /// declared one — a real, cascade-honoring inline declaration (highest
    /// specificity, same as real CSS), not an inert string; see
    /// [`crate::stylo`]'s `style_attribute` for where it's actually
    /// parsed and merged in.
    pub fn style_attr(&self, id: NodeId) -> Option<&str> {
        self.nodes[id].style.as_deref()
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
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if predicate(self, id) {
                return Some(id);
            }
            stack.extend(self.children(id).iter().rev());
        }
        None
    }

    /// Like [`Self::find`], but collects every match in document order
    /// instead of stopping at the first — the shared primitive both tab
    /// order and [`crate::focus::FocusPath::resolve`] need.
    pub fn find_all(&self, mut predicate: impl FnMut(&Self, NodeId) -> bool) -> Vec<NodeId> {
        let mut matches = Vec::new();
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            if predicate(self, id) {
                matches.push(id);
            }
            stack.extend(self.children(id).iter().rev());
        }
        matches
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

fn attr_bool(attrs: &[(String, String)], name: &str) -> bool {
    attr_value(attrs, name).as_deref() == Some("true")
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
    fn build_alone_reports_every_root_as_a_document_root() {
        let tree: Element = view! {
            <div />
            <div />
        };
        let arena = Arena::build(&tree);
        assert_eq!(arena.document_roots().len(), 2);
        assert!(arena.overlay_roots().is_empty());
    }

    #[test]
    fn build_with_overlays_partitions_document_and_overlay_roots_in_order() {
        let document: Element = view! {
            <div class="doc-a" />
            <div class="doc-b" />
        };
        let overlays: Element = view! { <div class="overlay-a" /> };
        let arena = Arena::build_with_overlays(&document, &overlays);

        assert_eq!(
            arena.roots().len(),
            3,
            "roots() stays one flat, ordered list"
        );
        assert_eq!(arena.document_roots().len(), 2);
        assert_eq!(arena.overlay_roots().len(), 1);
        assert_eq!(
            arena.classes(arena.overlay_roots()[0]),
            &["overlay-a".to_string()]
        );
        assert_eq!(
            arena.roots()[2],
            arena.overlay_roots()[0],
            "overlay roots come after every document root in roots()"
        );
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
    fn disabled_attribute_true_is_read() {
        let tree: Element = view! { <button disabled="true" /> };
        let arena = Arena::build(&tree);
        assert!(arena.is_disabled(arena.roots()[0]));
    }

    #[test]
    fn disabled_attribute_false_is_read_as_not_disabled() {
        let tree: Element = view! { <button disabled="false" /> };
        let arena = Arena::build(&tree);
        assert!(!arena.is_disabled(arena.roots()[0]));
    }

    #[test]
    fn disabled_attribute_absent_defaults_to_not_disabled() {
        let tree: Element = view! { <button /> };
        let arena = Arena::build(&tree);
        assert!(!arena.is_disabled(arena.roots()[0]));
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
    fn find_all_collects_every_match_in_document_order() {
        let tree: Element = view! {
            <div>
                <button>{"First"}</button>
                <span>{"Not a match"}</span>
                <button>{"Second"}</button>
            </div>
        };
        let arena = Arena::build(&tree);
        let buttons = arena.find_all(|arena, id| arena.tag(id) == "button");
        assert_eq!(buttons.len(), 2);
        assert_eq!(arena.text_content(buttons[0]), "First");
        assert_eq!(arena.text_content(buttons[1]), "Second");
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

    /// `build`/`find` used to recurse once per tree level and overflow
    /// the stack well before this depth — an explicit stack fixed that.
    #[test]
    fn build_and_find_survive_a_tree_far_deeper_than_the_old_recursion_limit() {
        let depth = 20_000;
        let mut tree = Element::node("div", vec![("class".into(), "leaf".into())], vec![]);
        for _ in 0..depth {
            tree = Element::node("div", vec![], vec![tree]);
        }

        let arena = Arena::build(&tree);
        let leaf = arena
            .find(|a, id| a.classes(id).iter().any(|c| c == "leaf"))
            .expect("the leaf must still be reachable at full depth");
        assert!(arena.children(leaf).is_empty());

        let mut depth_from_leaf = 0;
        let mut current = leaf;
        while let Some(parent) = arena.parent(current) {
            current = parent;
            depth_from_leaf += 1;
        }
        assert_eq!(depth_from_leaf, depth);
    }
}

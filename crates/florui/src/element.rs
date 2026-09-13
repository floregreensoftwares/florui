//! The element tree produced by `view!` and by components.
//!
//! This is a plain data tree with no identity, styling, layout, or paint
//! attached yet — those are separate, not-yet-built subsystems. An `Element`
//! only records what was written: tags, string attributes, event handlers,
//! text, and children.

use crate::Handler;

/// A node produced by `view!`: a tagged element, a text run, or a fragment
/// (a sequence of siblings with no wrapping box of their own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
    Node(ElementNode),
    Text(String),
    Fragment(Vec<Element>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementNode {
    pub tag: &'static str,
    pub attrs: Vec<(String, String)>,
    /// Callbacks from `on*` attributes (`onclick={...}`), keyed by event
    /// name with the leading `on` stripped (`"click"`).
    pub handlers: Vec<(String, Handler)>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn node(tag: &'static str, attrs: Vec<(String, String)>, children: Vec<Element>) -> Self {
        Self::node_with_handlers(tag, attrs, Vec::new(), children)
    }

    pub fn node_with_handlers(
        tag: &'static str,
        attrs: Vec<(String, String)>,
        handlers: Vec<(String, Handler)>,
        children: Vec<Element>,
    ) -> Self {
        Element::Node(ElementNode {
            tag,
            attrs,
            handlers,
            children,
        })
    }

    pub fn text(text: impl Into<String>) -> Self {
        Element::Text(text.into())
    }
}

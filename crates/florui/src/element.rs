//! The element tree produced by `view!` and by components.
//!
//! This is a plain data tree with no identity, styling, layout, or paint
//! attached yet — those are separate, not-yet-built subsystems. An `Element`
//! only records what was written: tags, string attributes, text, and
//! children.

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
    pub children: Vec<Element>,
}

impl Element {
    pub fn node(tag: &'static str, attrs: Vec<(String, String)>, children: Vec<Element>) -> Self {
        Element::Node(ElementNode {
            tag,
            attrs,
            children,
        })
    }

    pub fn text(text: impl Into<String>) -> Self {
        Element::Text(text.into())
    }
}

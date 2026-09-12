//! Converting a `view!` child expression into sibling elements.

use crate::children::Children;
use crate::element::Element;

/// Converts a `view!` child expression into zero or more sibling elements.
///
/// A single [`Element`] contributes itself; [`Children`] and `Vec<Element>`
/// flatten into their contained elements (this is how a component forwards
/// its own `children` without an extra wrapping node); text-like values
/// become a single text element.
pub trait IntoNodes {
    fn into_nodes(self) -> Vec<Element>;
}

impl IntoNodes for Element {
    fn into_nodes(self) -> Vec<Element> {
        vec![self]
    }
}

impl IntoNodes for Children {
    fn into_nodes(self) -> Vec<Element> {
        self.0
    }
}

impl IntoNodes for Vec<Element> {
    fn into_nodes(self) -> Vec<Element> {
        self
    }
}

impl IntoNodes for String {
    fn into_nodes(self) -> Vec<Element> {
        vec![Element::text(self)]
    }
}

impl IntoNodes for &str {
    fn into_nodes(self) -> Vec<Element> {
        vec![Element::text(self)]
    }
}

/// Conditional rendering: `None` contributes nothing, `Some(value)` behaves
/// like `value` was written directly — e.g. `{condition.then(|| el)}`.
impl<T: IntoNodes> IntoNodes for Option<T> {
    fn into_nodes(self) -> Vec<Element> {
        match self {
            Some(value) => value.into_nodes(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_flattens_to_itself() {
        let el = Element::text("hi");
        assert_eq!(el.clone().into_nodes(), vec![el]);
    }

    #[test]
    fn children_flatten_into_their_elements() {
        let children = Children::from(vec![Element::text("a"), Element::text("b")]);
        assert_eq!(
            children.into_nodes(),
            vec![Element::text("a"), Element::text("b")]
        );
    }

    #[test]
    fn strings_become_text_elements() {
        assert_eq!("hi".into_nodes(), vec![Element::text("hi")]);
        assert_eq!(String::from("hi").into_nodes(), vec![Element::text("hi")]);
    }
}

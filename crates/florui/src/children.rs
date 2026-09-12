//! The nested content passed into a component between its opening and
//! closing tag in `view!`.

use crate::element::Element;

/// An ordered list of elements passed into a component, typically the
/// nested content between its opening and closing tag in `view!`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Children(pub Vec<Element>);

impl From<Vec<Element>> for Children {
    fn from(elements: Vec<Element>) -> Self {
        Children(elements)
    }
}

impl IntoIterator for Children {
    type Item = Element;
    type IntoIter = std::vec::IntoIter<Element>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

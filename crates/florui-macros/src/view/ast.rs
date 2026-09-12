//! The parsed shape of a `view!` body, before codegen turns it into Rust
//! expressions building an `Element` tree.

use syn::{Expr, Ident, LitStr};

#[cfg_attr(test, derive(Debug))]
pub enum Node {
    /// A `{expr}` child: an arbitrary Rust expression contributing zero or
    /// more sibling elements through `IntoNodes`.
    Expr(Expr),
    /// A run of bare text between tags, already reconstructed into a single
    /// string by [`super::text`].
    Text(String),
    /// A tag: lowercase is a primitive HTML-like element, capitalized is a
    /// component call. `self_closing` tags never carry a `children` value.
    Element {
        tag: Ident,
        attrs: Vec<(Ident, AttrValue)>,
        children: Vec<Node>,
        self_closing: bool,
    },
}

#[cfg_attr(test, derive(Debug))]
pub enum AttrValue {
    Lit(LitStr),
    Expr(Expr),
}

impl Node {
    pub fn is_component(tag: &Ident) -> bool {
        tag.to_string()
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase())
    }
}

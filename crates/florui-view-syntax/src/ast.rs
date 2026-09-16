//! The parsed shape of a `view!` body — shared by `florui-macros`'s real
//! codegen and `florui-fmt`'s own formatter (see this crate's own module
//! doc for why that split exists).

use syn::{Expr, Ident, LitStr};

#[cfg_attr(test, derive(Debug))]
pub enum Node {
    /// A `{expr}` child: an arbitrary Rust expression contributing zero or
    /// more sibling elements through `IntoNodes`.
    Expr(Expr),
    /// A run of bare text between tags, already reconstructed into a single
    /// string by [`crate::text`].
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

// `Expr` genuinely is much larger than `LitStr`, but this tree is parsed
// once per `view!` invocation (or once per real formatter run), never a
// hot allocation path boxing would meaningfully help.
#[allow(clippy::large_enum_variant)]
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

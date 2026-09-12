//! `#[component]`: turns a function with typed positional parameters into a
//! generated props struct plus a function taking that struct by value.
//!
//! `fn Card(title: String, children: Children) -> Element { ... }` becomes
//! a `CardProps { title: String, children: Children }` struct and a `Card`
//! function that destructures it and runs the original body. This is what
//! lets `view!` call components with named fields the way it calls them
//! with tag attributes.

mod codegen;
mod parse;

use proc_macro2::TokenStream;
use syn::{ItemFn, Result};

pub fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[component]` does not take arguments",
        ));
    }

    let func: ItemFn = syn::parse2(item)?;
    let parsed = parse::parse(func)?;
    Ok(codegen::expand(parsed))
}

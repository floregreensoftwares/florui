//! `stylesheet_scoped!("./button.css")`: like `stylesheet!`, but opts the
//! declared CSS into local, `view!`-visible scoping instead of the global
//! cascade `stylesheet!` always joins.
//!
//! Generates the same `__FLORUI_STYLESHEET: florui::StylesheetSource` as
//! `stylesheet!`, but with `scope: Some(..)` set, plus a second constant,
//! `SCOPE: florui::StyleScope`, computed from the *identical*
//! `concat!`-built id string — an author references `SCOPE` from a
//! `scope={SCOPE}` directive on `view!` elements in the same
//! module. Both constants hash that id through
//! [`florui::style_scope_hash`] at the declaring crate's own compile time
//! (this macro, like `stylesheet!`, never resolves `file!()`/`env!()`
//! itself — see `stylesheet.rs`'s module doc for why), so the macro-side
//! scope and the `florui-build`-collected `StylesheetSource.scope` for the
//! same declaration are guaranteed to agree: they call the same `const fn`
//! over the same id text, not two independently maintained computations.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{LitStr, Result};

pub fn expand(input: TokenStream) -> Result<TokenStream> {
    let path: LitStr = syn::parse2(input)?;

    Ok(quote! {
        #[doc(hidden)]
        pub const __FLORUI_STYLESHEET: ::florui::StylesheetSource = ::florui::StylesheetSource {
            id: ::std::concat!(::std::env!("CARGO_PKG_NAME"), ":", ::std::file!(), ":", #path),
            source_path: #path,
            css: ::std::include_str!(#path),
            scope: ::std::option::Option::Some(::florui::StyleScope::new(
                ::std::concat!(::std::env!("CARGO_PKG_NAME"), ":", ::std::file!(), ":", #path),
            )),
        };

        /// This module's local style scope — pass to a `scope={...}`
        /// directive on a `view!` element to apply it (and, by inheritance,
        /// all of that element's own literal descendants).
        pub const SCOPE: ::florui::StyleScope = ::florui::StyleScope::new(
            ::std::concat!(::std::env!("CARGO_PKG_NAME"), ":", ::std::file!(), ":", #path),
        );
    })
}

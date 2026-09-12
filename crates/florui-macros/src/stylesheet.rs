//! `stylesheet!("./button.css")`: declares a module-owned CSS dependency.
//!
//! Generates a `const __FLORUI_STYLESHEET: florui::StylesheetSource` at the
//! call site. Resolves the path relative to the declaring file (not the
//! process working directory) by re-emitting the caller's own string
//! literal token into `include_str!`, so rustc resolves it exactly as it
//! would resolve an `include_str!` written directly in that file — this
//! macro never reads the file itself or otherwise needs to know where the
//! caller lives. The canonical identity combines the package name, the
//! declaring file's path, and the literal path text via the stable
//! `file!()`/`concat!()` builtins, all evaluated at the call site once
//! compiled, not by this macro: two different files that happen to write
//! the same relative path still get distinct identities.
//!
//! Only one `stylesheet!` per module is supported for now: a second call
//! in the same module fails with a duplicate-definition error rather than
//! silently overwriting or merging the first. Discovering every
//! `stylesheet!` declaration across a crate's module graph, in the
//! deterministic cascade order the real feature needs, is not implemented
//! here — see `florui::StylesheetSource`.

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
        };
    })
}

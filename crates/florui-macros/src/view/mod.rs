mod ast;
mod codegen;
mod parse;
mod text;

use proc_macro2::TokenStream;
use syn::Result;

pub fn expand(input: TokenStream) -> Result<TokenStream> {
    let nodes: parse::Nodes = syn::parse2(input)?;
    Ok(codegen::expand(nodes.0))
}

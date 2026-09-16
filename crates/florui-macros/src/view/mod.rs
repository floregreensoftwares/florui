mod codegen;

use florui_view_syntax::Nodes;
use proc_macro2::TokenStream;
use syn::Result;

pub fn expand(input: TokenStream) -> Result<TokenStream> {
    let nodes: Nodes = syn::parse2(input)?;
    Ok(codegen::expand(nodes.0))
}

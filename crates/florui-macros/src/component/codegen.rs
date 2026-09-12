//! Generates the props struct and wrapper function for a parsed
//! `#[component]` function.

use proc_macro2::TokenStream;
use quote::quote;

use super::parse::ParsedComponent;

pub fn expand(component: ParsedComponent) -> TokenStream {
    let ParsedComponent {
        vis,
        attrs,
        name,
        props_ident,
        field_names,
        field_types,
        return_type,
        block,
    } = component;

    quote! {
        #vis struct #props_ident {
            #( pub #field_names: #field_types, )*
        }

        #(#attrs)*
        // Component names are capitalized so `view!` can tell a component
        // call apart from a lowercase primitive tag.
        #[allow(non_snake_case)]
        #vis fn #name(__props: #props_ident) -> #return_type {
            let #props_ident { #(#field_names),* } = __props;
            #block
        }
    }
}

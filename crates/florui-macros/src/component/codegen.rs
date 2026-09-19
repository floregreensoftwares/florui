//! Generates the props struct and wrapper function for a parsed
//! `#[component]` function.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

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

    // `view!` calls this instead of `#name` when the call site declares a
    // `key={...}` attribute — see `view::codegen::component_call`. Doc-hidden:
    // this is a codegen detail, not part of a component's own public API.
    let keyed_name = format_ident!("__florui_keyed_{name}");

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
            // Every component gets its own persistent hook state, keyed to
            // this call site — requires an active `florui_reactive::ComponentScope`
            // (see `ComponentScope::render`) somewhere up the call stack, even for
            // the outermost/root component. Wrapping in `trace::with_component`
            // attributes any `Signal::set` this render (or an effect it
            // queues) performs to this component's name — see
            // `florui_reactive::trace`.
            ::florui::reactive::trace::with_component(stringify!(#name), || {
                ::florui::reactive::use_child_scope(move || #block)
            })
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #keyed_name(
            __key: ::florui::reactive::Key,
            __props: #props_ident,
        ) -> #return_type {
            let #props_ident { #(#field_names),* } = __props;
            // Same body as #name, but addressed by the caller's own key
            // instead of call position — see `use_child_scope_keyed`.
            ::florui::reactive::trace::with_component(stringify!(#name), || {
                ::florui::reactive::use_child_scope_keyed(__key, move || #block)
            })
        }
    }
}

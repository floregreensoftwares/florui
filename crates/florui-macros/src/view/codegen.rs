//! Turns a parsed `view!` body into an expression that builds an
//! `::florui::Element` tree at runtime.
//!
//! A lowercase tag becomes `Element::node(...)`. A capitalized tag is a
//! component call: it builds `<Tag>Props { ... }` from its attributes
//! (passed through exactly as written, with no implicit conversion — the
//! author writes `.into()` when a field needs it) and, for a non-self-closing
//! tag, a `children` field collected from its nested content.
//!
//! Every child — a nested element or a `{expr}` — is routed through
//! `IntoNodes::into_nodes` so a single element, a fragment, `Children`, or
//! text all compose the same way.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::Ident;

use super::ast::{AttrValue, Node};

/// `onclick`, `onmouseenter`, ... — any attribute in this shape names an
/// event handler rather than a plain string attribute; `view!` has no
/// fixed list of recognized event names; it's needed once a host actually
/// dispatches one.
fn event_name(attr_name: &str) -> Option<&str> {
    attr_name.strip_prefix("on").filter(|rest| !rest.is_empty())
}

pub fn expand(nodes: Vec<Node>) -> TokenStream {
    if nodes.is_empty() {
        return quote! {
            compile_error!("view! requires at least one element or expression")
        };
    }

    let values = nodes.iter().map(child_value);
    quote! {
        {
            let mut __roots: ::std::vec::Vec<::florui::Element> = ::std::vec::Vec::new();
            #( __roots.extend(::florui::IntoNodes::into_nodes(#values)); )*
            if __roots.len() == 1 {
                __roots.pop().expect("just checked len() == 1")
            } else {
                ::florui::Element::Fragment(__roots)
            }
        }
    }
}

fn child_value(node: &Node) -> TokenStream {
    match node {
        Node::Expr(expr) => quote! { (#expr) },
        Node::Text(text) => quote! { #text },
        Node::Element {
            tag,
            attrs,
            children,
            self_closing,
        } => {
            if Node::is_component(tag) {
                component_call(tag, attrs, children, *self_closing)
            } else {
                primitive_element(tag, attrs, children)
            }
        }
    }
}

fn children_vec(children: &[Node]) -> TokenStream {
    let values = children.iter().map(child_value);
    quote! {
        {
            let mut __children: ::std::vec::Vec<::florui::Element> = ::std::vec::Vec::new();
            #( __children.extend(::florui::IntoNodes::into_nodes(#values)); )*
            __children
        }
    }
}

fn primitive_element(tag: &Ident, attrs: &[(Ident, AttrValue)], children: &[Node]) -> TokenStream {
    let tag_str = tag.to_string();
    let mut attr_pairs = Vec::new();
    let mut handler_pairs = Vec::new();

    for (name, value) in attrs {
        let name_str = name.to_string();
        if let Some(event) = event_name(&name_str) {
            handler_pairs.push(match value {
                AttrValue::Expr(expr) => {
                    quote! { (#event.to_string(), ::florui::Handler::new(#expr)) }
                }
                AttrValue::Lit(lit) => {
                    let message = format!(
                        "event handler attribute `{name_str}` needs a Rust expression in \
                         braces, e.g. `{name_str}={{move || ...}}`, not a string literal"
                    );
                    quote_spanned! { lit.span() => compile_error!(#message) }
                }
            });
        } else {
            let value_expr = match value {
                AttrValue::Lit(lit) => quote! { (#lit).to_string() },
                AttrValue::Expr(expr) => quote! { (#expr).to_string() },
            };
            attr_pairs.push(quote! { (#name_str.to_string(), #value_expr) });
        }
    }
    let children = children_vec(children);

    if handler_pairs.is_empty() {
        quote! {
            ::florui::Element::node(#tag_str, ::std::vec![ #(#attr_pairs),* ], #children)
        }
    } else {
        quote! {
            ::florui::Element::node_with_handlers(
                #tag_str,
                ::std::vec![ #(#attr_pairs),* ],
                ::std::vec![ #(#handler_pairs),* ],
                #children,
            )
        }
    }
}

/// `key={...}` on a component call is its caller-assigned identity (see
/// `florui_reactive::use_child_scope_keyed`), not a prop of the component
/// itself — it never becomes a `Props` field.
fn is_key_attr(name: &Ident) -> bool {
    name == "key"
}

fn component_call(
    tag: &Ident,
    attrs: &[(Ident, AttrValue)],
    children: &[Node],
    self_closing: bool,
) -> TokenStream {
    let props_ident = format_ident!("{tag}Props");
    let key_attr = attrs.iter().find(|(name, _)| is_key_attr(name));
    let field_inits = attrs
        .iter()
        .filter(|(name, _)| !is_key_attr(name))
        .map(|(name, value)| {
            let value_expr = match value {
                AttrValue::Lit(lit) => quote! { #lit },
                AttrValue::Expr(expr) => quote! { #expr },
            };
            quote! { #name: #value_expr, }
        });

    let children_field = if self_closing {
        TokenStream::new()
    } else {
        let children = children_vec(children);
        quote! { children: ::florui::Children::from(#children), }
    };

    let props = quote! { #props_ident { #(#field_inits)* #children_field } };

    match key_attr {
        Some((_, value)) => {
            let key_expr = match value {
                AttrValue::Lit(lit) => quote! { #lit },
                AttrValue::Expr(expr) => quote! { #expr },
            };
            let keyed_tag = format_ident!("__florui_keyed_{tag}");
            quote! { #keyed_tag(::florui::reactive::Key::from(#key_expr), #props) }
        }
        None => quote! { #tag(#props) },
    }
}

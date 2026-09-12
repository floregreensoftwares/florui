//! Validates a `#[component]` function and extracts what its generated
//! props struct and wrapper function need, without deciding how that code
//! looks (see `codegen`).

use quote::format_ident;
use syn::{FnArg, Ident, ItemFn, Pat, Result, ReturnType, Type, Visibility};

pub struct ParsedComponent {
    pub vis: Visibility,
    pub attrs: Vec<syn::Attribute>,
    pub name: Ident,
    pub props_ident: Ident,
    pub field_names: Vec<Ident>,
    pub field_types: Vec<Type>,
    pub return_type: Type,
    pub block: Box<syn::Block>,
}

pub fn parse(func: ItemFn) -> Result<ParsedComponent> {
    if !func.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &func.sig.generics,
            "`#[component]` does not support generic functions yet",
        ));
    }

    let mut field_names = Vec::new();
    let mut field_types = Vec::new();
    for input in &func.sig.inputs {
        match input {
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "`#[component]` functions cannot take `self`",
                ));
            }
            FnArg::Typed(pat_type) => {
                let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
                    return Err(syn::Error::new_spanned(
                        &pat_type.pat,
                        "`#[component]` parameters must be simple names, not patterns",
                    ));
                };
                field_names.push(pat_ident.ident.clone());
                field_types.push(pat_type.ty.as_ref().clone());
            }
        }
    }

    let return_type = match func.sig.output {
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                &func.sig,
                "`#[component]` functions must return an element",
            ));
        }
        ReturnType::Type(_, ty) => *ty,
    };

    let name = func.sig.ident.clone();
    let props_ident = format_ident!("{name}Props");

    Ok(ParsedComponent {
        vis: func.vis,
        attrs: func.attrs,
        name,
        props_ident,
        field_names,
        field_types,
        return_type,
        block: func.block,
    })
}

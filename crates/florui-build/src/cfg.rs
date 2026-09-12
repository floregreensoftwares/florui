//! Evaluates the `#[cfg(...)]` predicates this collector understands well
//! enough to trust: Cargo feature checks and `not`/`any`/`all` over them.
//!
//! Anything else (`target_os`, bare `unix`, `test`, ...) is reported as
//! unsupported rather than guessed at, since guessing wrong would silently
//! include or exclude a module's stylesheet — the collector must match
//! "the selected Rust build configuration," not approximate it.

use proc_macro2::Ident;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::{Attribute, LitStr, Meta, Result, Token};

pub enum Cfg {
    Feature(String),
    Not(Box<Cfg>),
    Any(Vec<Cfg>),
    All(Vec<Cfg>),
    Unsupported(String),
}

/// Whether `attrs` (from a `mod` item) enable that module, given
/// `is_feature_enabled` to answer whether a named Cargo feature is active.
/// Takes that as a plain function rather than reading `CARGO_FEATURE_*`
/// itself, so evaluation has no shared process-global state to race on —
/// see [`cargo_feature_enabled`] for the real check a build script wants.
///
/// Returns `Err` naming a `#[cfg(...)]` predicate this collector does not
/// understand.
pub fn module_enabled(
    attrs: &[Attribute],
    is_feature_enabled: &dyn Fn(&str) -> bool,
) -> std::result::Result<bool, String> {
    let mut enabled = true;
    for attr in attrs {
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        if !list.path.is_ident("cfg") {
            continue;
        }
        let cfg = syn::parse2(list.tokens.clone())
            .map_err(|err| format!("could not parse #[cfg(...)]: {err}"))?;
        enabled &= evaluate(&cfg, is_feature_enabled)?;
    }
    Ok(enabled)
}

fn evaluate(
    cfg: &Cfg,
    is_feature_enabled: &dyn Fn(&str) -> bool,
) -> std::result::Result<bool, String> {
    match cfg {
        Cfg::Feature(name) => Ok(is_feature_enabled(name)),
        Cfg::Not(inner) => evaluate(inner, is_feature_enabled).map(|value| !value),
        Cfg::Any(items) => {
            for item in items {
                if evaluate(item, is_feature_enabled)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Cfg::All(items) => {
            for item in items {
                if !evaluate(item, is_feature_enabled)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Cfg::Unsupported(description) => Err(format!(
            "unsupported #[cfg({description})]: only feature/not/any/all are understood"
        )),
    }
}

/// Whether Cargo enabled `name` for the crate currently being built,
/// per the `CARGO_FEATURE_*` environment variables Cargo sets for build
/// scripts.
pub fn cargo_feature_enabled(name: &str) -> bool {
    let env_name = format!(
        "CARGO_FEATURE_{}",
        name.to_uppercase().replace(['-', '.'], "_")
    );
    std::env::var(env_name).is_ok()
}

impl syn::parse::Parse for Cfg {
    fn parse(input: ParseStream) -> Result<Self> {
        let name: Ident = input.parse()?;
        let name_str = name.to_string();

        if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            let value: LitStr = input.parse()?;
            return Ok(if name_str == "feature" {
                Cfg::Feature(value.value())
            } else {
                Cfg::Unsupported(format!("{name_str} = \"{}\"", value.value()))
            });
        }

        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let items: Vec<Cfg> = Punctuated::<Cfg, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();
            return Ok(match name_str.as_str() {
                "not" if items.len() == 1 => Cfg::Not(Box::new(items.into_iter().next().unwrap())),
                "not" => Cfg::Unsupported("not(...) must take exactly one predicate".to_string()),
                "any" => Cfg::Any(items),
                "all" => Cfg::All(items),
                _ => Cfg::Unsupported(format!("{name_str}(...)")),
            });
        }

        Ok(Cfg::Unsupported(name_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_attrs(predicate: &str) -> Vec<Attribute> {
        let item: syn::ItemMod = syn::parse_str(&format!("#[cfg({predicate})] mod m;")).unwrap();
        item.attrs
    }

    fn only(enabled: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |name| enabled.contains(&name)
    }

    #[test]
    fn feature_predicate_checks_the_given_lookup() {
        assert!(module_enabled(&cfg_attrs("feature = \"fancy\""), &only(&["fancy"])).unwrap());
        assert!(!module_enabled(&cfg_attrs("feature = \"missing\""), &only(&["fancy"])).unwrap());
    }

    #[test]
    fn not_negates_the_inner_predicate() {
        assert!(module_enabled(&cfg_attrs("not(feature = \"missing\")"), &only(&[])).unwrap());
    }

    #[test]
    fn any_matches_when_one_branch_matches() {
        assert!(
            module_enabled(
                &cfg_attrs("any(feature = \"a\", feature = \"b\")"),
                &only(&["a"])
            )
            .unwrap()
        );
    }

    #[test]
    fn all_requires_every_branch() {
        assert!(
            !module_enabled(
                &cfg_attrs("all(feature = \"a\", feature = \"b\")"),
                &only(&["a"])
            )
            .unwrap()
        );
        assert!(
            module_enabled(
                &cfg_attrs("all(feature = \"a\", feature = \"b\")"),
                &only(&["a", "b"])
            )
            .unwrap()
        );
    }

    #[test]
    fn unsupported_predicates_are_reported_not_guessed() {
        assert!(module_enabled(&cfg_attrs("target_os = \"windows\""), &only(&[])).is_err());
        assert!(module_enabled(&cfg_attrs("unix"), &only(&[])).is_err());
    }

    #[test]
    fn no_cfg_attribute_means_always_enabled() {
        let item: syn::ItemMod = syn::parse_str("mod m;").unwrap();
        assert!(module_enabled(&item.attrs, &only(&[])).unwrap());
    }
}

//! Finds `stylesheet!("...")` invocations among a module's own top-level
//! items. Nested `mod` blocks are not descended into here — the traversal
//! in [`crate::collect`] visits those separately, in the order the
//! deterministic cascade needs.

use syn::{Item, LitStr};

pub struct StylesheetInvocation {
    pub literal_path: String,
    /// Whether this came from `stylesheet_scoped!` rather than plain
    /// `stylesheet!`.
    pub scoped: bool,
}

#[derive(Debug)]
pub struct ScanError {
    pub message: String,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ScanError {}

pub fn find_stylesheet_invocations(items: &[Item]) -> Result<Vec<StylesheetInvocation>, ScanError> {
    let mut found = Vec::new();
    for item in items {
        let Item::Macro(item_macro) = item else {
            continue;
        };
        let Some(scoped) = stylesheet_macro_kind(&item_macro.mac.path) else {
            continue;
        };
        let path: LitStr = syn::parse2(item_macro.mac.tokens.clone()).map_err(|err| ScanError {
            message: format!("could not parse stylesheet!(...) arguments: {err}"),
        })?;
        found.push(StylesheetInvocation {
            literal_path: path.value(),
            scoped,
        });
    }
    Ok(found)
}

/// `Some(false)` for `stylesheet!`, `Some(true)` for `stylesheet_scoped!`,
/// `None` for anything else.
fn stylesheet_macro_kind(path: &syn::Path) -> Option<bool> {
    let last = path.segments.last()?;
    if last.ident == "stylesheet" {
        Some(false)
    } else if last.ident == "stylesheet_scoped" {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(source: &str) -> Vec<Item> {
        syn::parse_str::<syn::File>(source).unwrap().items
    }

    #[test]
    fn finds_a_bare_invocation() {
        let found = find_stylesheet_invocations(&items(r#"stylesheet!("./button.css");"#)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].literal_path, "./button.css");
        assert!(!found[0].scoped);
    }

    #[test]
    fn finds_a_scoped_invocation() {
        let found =
            find_stylesheet_invocations(&items(r#"stylesheet_scoped!("./card.css");"#)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].literal_path, "./card.css");
        assert!(found[0].scoped);
    }

    #[test]
    fn finds_a_qualified_scoped_invocation() {
        let found =
            find_stylesheet_invocations(&items(r#"florui::stylesheet_scoped!("./card.css");"#))
                .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].scoped);
    }

    #[test]
    fn finds_a_qualified_invocation() {
        let found =
            find_stylesheet_invocations(&items(r#"florui::stylesheet!("./button.css");"#)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].literal_path, "./button.css");
    }

    #[test]
    fn ignores_unrelated_macros_and_items() {
        let found = find_stylesheet_invocations(&items(
            r#"
            println!("hi");
            fn f() {}
            "#,
        ))
        .unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn reports_a_non_literal_argument() {
        let result = find_stylesheet_invocations(&items("stylesheet!(some_expr);"));
        assert!(result.is_err());
    }
}

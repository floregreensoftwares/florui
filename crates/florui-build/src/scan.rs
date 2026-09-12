//! Finds `stylesheet!("...")` invocations among a module's own top-level
//! items. Nested `mod` blocks are not descended into here — the traversal
//! in [`crate::collect`] visits those separately, in the order the
//! deterministic cascade needs.

use syn::{Item, LitStr};

pub struct StylesheetInvocation {
    pub literal_path: String,
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
        if !is_stylesheet_macro(&item_macro.mac.path) {
            continue;
        }
        let path: LitStr = syn::parse2(item_macro.mac.tokens.clone()).map_err(|err| ScanError {
            message: format!("could not parse stylesheet!(...) arguments: {err}"),
        })?;
        found.push(StylesheetInvocation {
            literal_path: path.value(),
        });
    }
    Ok(found)
}

fn is_stylesheet_macro(path: &syn::Path) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == "stylesheet")
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

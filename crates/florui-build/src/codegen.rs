//! Generates the Rust source for a build-script `OUT_DIR` file embedding
//! the collected, ordered stylesheet manifest as `florui::StylesheetSource`
//! values, ready for `include!(concat!(env!("OUT_DIR"), "/...rs"))`.

use crate::collect::CollectedStylesheet;

pub const MANIFEST_ITEM_NAME: &str = "FLORUI_STYLESHEETS";

/// Renders `stylesheets` (already in the order they should cascade in) as
/// a `pub static` slice of `florui::StylesheetSource`.
pub fn generate_manifest(stylesheets: &[CollectedStylesheet]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "pub static {MANIFEST_ITEM_NAME}: &[::florui::StylesheetSource] = &[\n"
    ));
    for sheet in stylesheets {
        out.push_str("    ::florui::StylesheetSource {\n");
        out.push_str(&format!("        id: {:?},\n", sheet.id));
        out.push_str(&format!("        source_path: {:?},\n", sheet.literal_path));
        out.push_str(&format!("        css: {:?},\n", sheet.css));
        if sheet.scoped {
            out.push_str(&format!(
                "        scope: ::std::option::Option::Some(::florui::StyleScope::new({:?})),\n",
                sheet.id
            ));
        } else {
            out.push_str("        scope: ::std::option::Option::None,\n");
        }
        out.push_str("    },\n");
    }
    out.push_str("];\n");
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn sheet(id: &str, literal_path: &str, css: &str) -> CollectedStylesheet {
        scoped_sheet(id, literal_path, css, false)
    }

    fn scoped_sheet(id: &str, literal_path: &str, css: &str, scoped: bool) -> CollectedStylesheet {
        CollectedStylesheet {
            id: id.to_string(),
            declared_at: PathBuf::from("src/lib.rs"),
            literal_path: literal_path.to_string(),
            css_path: PathBuf::from(literal_path),
            css: css.to_string(),
            scoped,
        }
    }

    #[test]
    fn generates_valid_rust_source_for_an_empty_manifest() {
        let source = generate_manifest(&[]);
        let parsed: syn::File = syn::parse_str(&source).expect("generated source must parse");
        assert_eq!(parsed.items.len(), 1);
    }

    #[test]
    fn generates_valid_rust_source_with_entries() {
        let stylesheets = vec![
            sheet("pkg:src/lib.rs:./a.css", "./a.css", ".a { color: red; }"),
            sheet("pkg:src/lib.rs:./b.css", "./b.css", ".b { color: blue; }"),
        ];
        let source = generate_manifest(&stylesheets);
        let parsed: syn::File = syn::parse_str(&source).expect("generated source must parse");
        assert_eq!(parsed.items.len(), 1);
        assert!(source.contains("pkg:src/lib.rs:./a.css"));
        assert!(source.contains(".a { color: red; }"));
    }

    #[test]
    fn a_scoped_entry_generates_a_style_class_scope_construction() {
        let stylesheets = vec![scoped_sheet(
            "pkg:src/card.rs:./card.css",
            "./card.css",
            ".box {}",
            true,
        )];
        let source = generate_manifest(&stylesheets);
        let parsed: syn::File = syn::parse_str(&source).expect("generated source must parse");
        assert_eq!(parsed.items.len(), 1);
        assert!(source.contains("::florui::StyleScope::new(\"pkg:src/card.rs:./card.css\")"));
    }

    #[test]
    fn an_unscoped_entry_generates_a_plain_none() {
        let stylesheets = vec![sheet("pkg:src/lib.rs:./a.css", "./a.css", ".a {}")];
        let source = generate_manifest(&stylesheets);
        assert!(source.contains("scope: ::std::option::Option::None,"));
    }

    #[test]
    fn escapes_content_that_would_otherwise_break_the_string_literal() {
        let stylesheets = vec![sheet(
            "id",
            "./x.css",
            "content: \"quoted\";\n.b { color: red; }",
        )];
        let source = generate_manifest(&stylesheets);
        syn::parse_str::<syn::File>(&source)
            .expect("generated source must parse even with quotes/newlines in CSS");
    }
}

//! Builds a real directory tree with several modules (inline and
//! file-based, one `mod.rs`, one feature-gated) and verifies the collector
//! reproduces the exact deterministic order: an enabled module's children
//! before its own declarations, in source order, skipping disabled ones,
//! and rejecting `@import`.

use std::fs;
use std::path::PathBuf;

use florui_build::collect_stylesheets;

struct TempCrate {
    root: PathBuf,
}

impl TempCrate {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "florui-build-integration-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        TempCrate { root }
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

impl Drop for TempCrate {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn no_features(_: &str) -> bool {
    false
}

#[test]
fn collects_in_deterministic_depth_first_post_order() {
    let krate = TempCrate::new("order");
    krate.write(
        "src/lib.rs",
        r#"
        mod alpha;
        mod beta {
            mod gamma;
            stylesheet!("./beta.css");
        }
        #[cfg(feature = "disabled")]
        mod disabled_mod;
        stylesheet!("./root.css");
        "#,
    );
    krate.write("src/alpha.rs", r#"stylesheet!("./alpha.css");"#);
    krate.write("src/beta/gamma.rs", r#"stylesheet!("./gamma.css");"#);
    krate.write("src/alpha.css", ".alpha { color: red; }");
    // `beta`'s stylesheet! is inline in lib.rs, so its literal path
    // resolves relative to lib.rs's own directory, not a `beta/`
    // subdirectory — inline module content has no file of its own for
    // `include_str!` to resolve against.
    krate.write("src/beta.css", ".beta { color: green; }");
    krate.write("src/beta/gamma.css", ".gamma { color: blue; }");
    krate.write("src/root.css", ".root { color: black; }");
    // disabled_mod.rs deliberately does not exist: a disabled module must
    // never even be resolved, let alone parsed.

    let result = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap();

    let paths: Vec<&str> = result
        .stylesheets
        .iter()
        .map(|s| s.literal_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["./alpha.css", "./gamma.css", "./beta.css", "./root.css"]
    );
}

#[test]
fn a_cfg_enabled_module_is_included() {
    let krate = TempCrate::new("cfg-enabled");
    krate.write(
        "src/lib.rs",
        r#"
        #[cfg(feature = "extra")]
        mod extra;
        "#,
    );
    krate.write("src/extra.rs", r#"stylesheet!("./extra.css");"#);
    krate.write("src/extra.css", ".extra {}");

    let result = collect_stylesheets("demo", &krate.root, &krate.root.join("src/lib.rs"), &|f| {
        f == "extra"
    })
    .unwrap();

    assert_eq!(result.stylesheets.len(), 1);
    assert_eq!(result.stylesheets[0].literal_path, "./extra.css");
}

#[test]
fn repeated_file_declarations_via_path_attribute_collect_once() {
    let krate = TempCrate::new("dedup");
    krate.write(
        "src/lib.rs",
        r#"
        #[path = "shared.rs"]
        mod a;
        #[path = "shared.rs"]
        mod b;
        "#,
    );
    krate.write("src/shared.rs", r#"stylesheet!("./shared.css");"#);
    krate.write("src/shared.css", ".shared {}");

    let result = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap();

    assert_eq!(
        result.stylesheets.len(),
        1,
        "the same declaring file reached twice must collect once"
    );
}

#[test]
fn unsupported_at_import_is_rejected_with_a_clear_error() {
    let krate = TempCrate::new("import");
    krate.write("src/lib.rs", r#"stylesheet!("./x.css");"#);
    krate.write(
        "src/x.css",
        "@import url(\"other.css\");\n.a { color: red; }",
    );

    let err = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap_err();

    assert!(err.to_string().contains("@import"));
}

#[test]
fn visited_rust_files_include_every_reachable_module_for_rerun_if_changed() {
    let krate = TempCrate::new("visited");
    krate.write("src/lib.rs", "mod alpha;");
    krate.write("src/alpha.rs", "");

    let result = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap();

    assert!(
        result
            .visited_rust_files
            .iter()
            .any(|p| p.ends_with("lib.rs"))
    );
    assert!(
        result
            .visited_rust_files
            .iter()
            .any(|p| p.ends_with("alpha.rs"))
    );
}

#[test]
fn an_unsupported_cfg_with_no_stylesheet_underneath_is_tolerated() {
    let krate = TempCrate::new("cfg-test-inline");
    krate.write(
        "src/lib.rs",
        r#"
        stylesheet!("./root.css");

        #[cfg(test)]
        mod tests {
            #[test]
            fn some_unit_test() {
                assert_eq!(1 + 1, 2);
            }
        }
        "#,
    );
    krate.write("src/root.css", ".root {}");

    let result = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap();

    assert_eq!(result.stylesheets.len(), 1);
    assert_eq!(result.stylesheets[0].literal_path, "./root.css");
}

#[test]
fn an_unsupported_cfg_with_no_stylesheet_underneath_is_tolerated_for_a_file_module() {
    let krate = TempCrate::new("cfg-test-file");
    krate.write("src/lib.rs", "#[cfg(test)]\nmod tests;");
    krate.write("src/tests.rs", "fn some_helper() {}");

    let result = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap();

    assert!(result.stylesheets.is_empty());
}

#[test]
fn an_unsupported_cfg_hiding_a_real_stylesheet_is_still_rejected() {
    let krate = TempCrate::new("cfg-test-hides-stylesheet");
    krate.write(
        "src/lib.rs",
        r#"
        #[cfg(test)]
        mod tests {
            stylesheet!("./hidden.css");
        }
        "#,
    );
    krate.write("src/hidden.css", ".hidden {}");

    let err = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("test"),
        "the error should still name the unsupported predicate: {err}"
    );
}

#[test]
fn a_missing_module_file_is_a_clear_error_not_a_panic() {
    let krate = TempCrate::new("missing");
    krate.write("src/lib.rs", "mod ghost;");

    let err = collect_stylesheets(
        "demo",
        &krate.root,
        &krate.root.join("src/lib.rs"),
        &no_features,
    )
    .unwrap_err();

    assert!(err.to_string().contains("ghost"));
}

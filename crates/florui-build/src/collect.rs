//! Walks a crate's module graph from its root file, in the deterministic
//! order the cascade needs: for each module, its enabled child modules
//! first (depth-first, source order), then that module's own
//! `stylesheet!` declarations (source order) — so a component's own styles
//! always precede the module that uses it.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::{Item, ItemMod};

use crate::cfg;
use crate::module_graph::{self, ModuleGraphError};
use crate::scan::{self, ScanError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedStylesheet {
    /// A reproducible identity: package name, the declaring file's
    /// package-relative path, and the literal path as written. Two
    /// declarations only share an identity if they are the exact same
    /// `stylesheet!` call site.
    pub id: String,
    /// The declaring `.rs` file, relative to the package root.
    pub declared_at: PathBuf,
    /// The literal path argument, unresolved (for diagnostics).
    pub literal_path: String,
    /// The resolved, absolute path to the CSS file.
    pub css_path: PathBuf,
    pub css: String,
    /// Whether this came from `stylesheet_scoped!` rather than plain
    /// `stylesheet!` — controls whether [`crate::codegen`] emits a
    /// `StylesheetSource.scope` for it.
    pub scoped: bool,
}

#[derive(Debug)]
pub enum CollectError {
    ModuleGraph(ModuleGraphError),
    Scan {
        file: PathBuf,
        source: ScanError,
    },
    UnsupportedCfg {
        module: String,
        file: PathBuf,
        message: String,
    },
    ReadCss {
        path: PathBuf,
        source: std::io::Error,
    },
    UnsupportedImport {
        css_path: PathBuf,
    },
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectError::ModuleGraph(err) => write!(f, "{err}"),
            CollectError::Scan { file, source } => write!(f, "{}: {source}", file.display()),
            CollectError::UnsupportedCfg {
                module,
                file,
                message,
            } => {
                write!(f, "{}: module `{module}`: {message}", file.display())
            }
            CollectError::ReadCss { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            CollectError::UnsupportedImport { css_path } => write!(
                f,
                "{}: CSS @import is not supported yet; inline the rules instead",
                css_path.display()
            ),
        }
    }
}

impl std::error::Error for CollectError {}

impl From<ModuleGraphError> for CollectError {
    fn from(err: ModuleGraphError) -> Self {
        CollectError::ModuleGraph(err)
    }
}

/// Every file visited while collecting, so a caller can tell Cargo to
/// rerun the build script when any of them changes — "track Rust
/// declarations, CSS ... as build inputs."
#[derive(Debug, Default)]
pub struct CollectResult {
    pub stylesheets: Vec<CollectedStylesheet>,
    pub visited_rust_files: Vec<PathBuf>,
}

/// Collects every enabled `stylesheet!` declaration reachable from
/// `crate_root_file` (typically `src/lib.rs` or `src/main.rs`), in
/// deterministic cascade order, deduplicated by identity (first occurrence
/// wins; distinct files with identical content stay distinct).
pub fn collect_stylesheets(
    package_name: &str,
    package_root: &Path,
    crate_root_file: &Path,
    is_feature_enabled: &dyn Fn(&str) -> bool,
) -> Result<CollectResult, CollectError> {
    let root_ast = module_graph::parse(crate_root_file)?;
    let root_children_dir = crate_root_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut result = CollectResult {
        stylesheets: Vec::new(),
        visited_rust_files: vec![crate_root_file.to_path_buf()],
    };
    visit(
        &root_ast.items,
        crate_root_file,
        &root_children_dir,
        package_name,
        package_root,
        is_feature_enabled,
        &mut result,
    )?;

    result.stylesheets = dedup(result.stylesheets);
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn visit(
    items: &[Item],
    declaring_file: &Path,
    children_dir: &Path,
    package_name: &str,
    package_root: &Path,
    is_feature_enabled: &dyn Fn(&str) -> bool,
    result: &mut CollectResult,
) -> Result<(), CollectError> {
    // Enabled child modules first, depth-first, in source order.
    for item in items {
        let Item::Mod(module) = item else { continue };
        match module_enabled(module, declaring_file, is_feature_enabled) {
            Ok(true) => {}
            Ok(false) => continue,
            // A predicate this collector cannot evaluate (`test`,
            // `target_os`, ...) only matters if a real `stylesheet!` could
            // be hiding under it — e.g. an inline `#[cfg(test)] mod tests`
            // full of ordinary unit tests never contains one. Checking
            // first, rather than failing on sight, is what makes writing
            // tests anywhere in a `stylesheet!`-using crate possible at
            // all; still refuse to guess when it could actually matter.
            Err(err) => {
                if module_subtree_has_stylesheet(module, declaring_file, children_dir)? {
                    return Err(err);
                }
                continue;
            }
        }

        match &module.content {
            Some((_, inline_items)) => {
                let name = module.ident.to_string();
                let inline_children_dir =
                    module_graph::children_dir_for(declaring_file, children_dir, &name);
                visit(
                    inline_items,
                    declaring_file,
                    &inline_children_dir,
                    package_name,
                    package_root,
                    is_feature_enabled,
                    result,
                )?;
            }
            None => {
                let name = module.ident.to_string();
                let child_file = module_graph::resolve_child_path(children_dir, module)?;
                let child_children_dir =
                    module_graph::children_dir_for(&child_file, children_dir, &name);
                let child_ast = module_graph::parse(&child_file)?;
                result.visited_rust_files.push(child_file.clone());
                visit(
                    &child_ast.items,
                    &child_file,
                    &child_children_dir,
                    package_name,
                    package_root,
                    is_feature_enabled,
                    result,
                )?;
            }
        }
    }

    // Then this module's own declarations, in source order.
    let invocations =
        scan::find_stylesheet_invocations(items).map_err(|source| CollectError::Scan {
            file: declaring_file.to_path_buf(),
            source,
        })?;
    let declaring_dir = declaring_file.parent().unwrap_or_else(|| Path::new("."));
    for invocation in invocations {
        result.stylesheets.push(build_entry(
            package_name,
            package_root,
            declaring_file,
            declaring_dir,
            &invocation.literal_path,
            invocation.scoped,
        )?);
    }

    Ok(())
}

fn module_enabled(
    module: &ItemMod,
    declaring_file: &Path,
    is_feature_enabled: &dyn Fn(&str) -> bool,
) -> Result<bool, CollectError> {
    cfg::module_enabled(&module.attrs, is_feature_enabled).map_err(|message| {
        CollectError::UnsupportedCfg {
            module: module.ident.to_string(),
            file: declaring_file.to_path_buf(),
            message,
        }
    })
}

/// Resolves `module`'s own content (inline or a child file, same as the
/// normal traversal) and checks it with [`subtree_has_stylesheet`].
fn module_subtree_has_stylesheet(
    module: &ItemMod,
    declaring_file: &Path,
    children_dir: &Path,
) -> Result<bool, CollectError> {
    match &module.content {
        Some((_, inline_items)) => {
            let name = module.ident.to_string();
            let inline_children_dir =
                module_graph::children_dir_for(declaring_file, children_dir, &name);
            subtree_has_stylesheet(inline_items, declaring_file, &inline_children_dir)
        }
        None => {
            let name = module.ident.to_string();
            let child_file = module_graph::resolve_child_path(children_dir, module)?;
            let child_children_dir =
                module_graph::children_dir_for(&child_file, children_dir, &name);
            let child_ast = module_graph::parse(&child_file)?;
            subtree_has_stylesheet(&child_ast.items, &child_file, &child_children_dir)
        }
    }
}

/// Whether a `stylesheet!` declaration could be reached from `items`,
/// ignoring every `#[cfg(...)]` gate along the way — used only to decide
/// whether an unsupported cfg predicate is safe to ignore (nothing
/// stylesheet-shaped lives under it, under any possible resolution of that
/// predicate) or must be reported instead.
fn subtree_has_stylesheet(
    items: &[Item],
    declaring_file: &Path,
    children_dir: &Path,
) -> Result<bool, CollectError> {
    let invocations =
        scan::find_stylesheet_invocations(items).map_err(|source| CollectError::Scan {
            file: declaring_file.to_path_buf(),
            source,
        })?;
    if !invocations.is_empty() {
        return Ok(true);
    }

    for item in items {
        let Item::Mod(module) = item else { continue };
        if module_subtree_has_stylesheet(module, declaring_file, children_dir)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn build_entry(
    package_name: &str,
    package_root: &Path,
    declaring_file: &Path,
    declaring_dir: &Path,
    literal_path: &str,
    scoped: bool,
) -> Result<CollectedStylesheet, CollectError> {
    let css_path = declaring_dir.join(literal_path);
    let css = fs::read_to_string(&css_path).map_err(|source| CollectError::ReadCss {
        path: css_path.clone(),
        source,
    })?;
    if css.to_ascii_lowercase().contains("@import") {
        return Err(CollectError::UnsupportedImport { css_path });
    }

    let declared_at = declaring_file
        .strip_prefix(package_root)
        .unwrap_or(declaring_file)
        .to_path_buf();
    let id = format!("{package_name}:{}:{literal_path}", declared_at.display());

    Ok(CollectedStylesheet {
        id,
        declared_at,
        literal_path: literal_path.to_string(),
        css_path,
        css,
        scoped,
    })
}

fn dedup(sources: Vec<CollectedStylesheet>) -> Vec<CollectedStylesheet> {
    let mut seen = HashSet::new();
    sources
        .into_iter()
        .filter(|s| seen.insert(s.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "florui-build-collect-test-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// `CollectedStylesheet::declared_at` is formatted with `Path::display`,
    /// which uses the platform's native separator — exactly what a real
    /// `file!()` invocation at that same source location also produces
    /// (confirmed empirically for this Windows toolchain: `file!()` in a
    /// nested module and `Path::join(...).display()` for that identical
    /// file agree byte-for-byte, both using `\`). This equality is now a
    /// real contract, not a historical curiosity — explicit style scoping
    /// derives a runtime scope identity from the macro-side `file!()`-based
    /// id and matches it against this build-side id, so the two formatting
    /// schemes must keep agreeing. Pin the platform-native-separator
    /// behavior here so a future change to this formatting doesn't silently
    /// break that match.
    #[test]
    fn declared_at_uses_the_platform_native_separator() {
        let dir = TempDir::new("separator");
        dir.write("src/main.rs", "mod components;");
        dir.write("src/components/mod.rs", "mod button;");
        dir.write(
            "src/components/button.rs",
            r#"florui::stylesheet!("./button.css");"#,
        );
        dir.write("src/components/button.css", ".button { color: red; }");

        let result = collect_stylesheets("pkg", &dir.0, &dir.0.join("src/main.rs"), &|_| false)
            .expect("collection should succeed");

        assert_eq!(result.stylesheets.len(), 1);
        let sep = std::path::MAIN_SEPARATOR;
        let expected = format!("src{sep}components{sep}button.rs");
        assert_eq!(
            result.stylesheets[0].declared_at.display().to_string(),
            expected
        );
    }

    #[test]
    fn distinguishes_scoped_from_global_declarations() {
        let dir = TempDir::new("scoped-flag");
        dir.write("src/main.rs", "mod a;\nmod b;");
        dir.write("src/a.rs", r#"florui::stylesheet!("./a.css");"#);
        dir.write("src/a.css", ".a {}");
        dir.write("src/b.rs", r#"florui::stylesheet_scoped!("./b.css");"#);
        dir.write("src/b.css", ".b {}");

        let result = collect_stylesheets("pkg", &dir.0, &dir.0.join("src/main.rs"), &|_| false)
            .expect("collection should succeed");

        assert_eq!(result.stylesheets.len(), 2);
        let a = result
            .stylesheets
            .iter()
            .find(|s| s.literal_path == "./a.css")
            .unwrap();
        let b = result
            .stylesheets
            .iter()
            .find(|s| s.literal_path == "./b.css")
            .unwrap();
        assert!(!a.scoped);
        assert!(b.scoped);
    }
}

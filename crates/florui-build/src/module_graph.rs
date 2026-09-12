//! Resolves `mod name;` declarations to the files Rust's own module system
//! would load, so callers can walk a crate's real module tree instead of
//! guessing at directory conventions.
//!
//! Deliberately low-level: this only answers "what file/directory does
//! this declaration mean", not "walk the whole tree" — [`crate::collect`]
//! owns the traversal, since it needs to treat a `mod name;` (a separate
//! file) and a `mod name { ... }` (inline) differently: an inline module's
//! items live in the same file as its declaration, but its own children
//! still resolve into a same-named subdirectory, exactly like a file
//! module's would.

use std::fs;
use std::path::{Path, PathBuf};

use syn::{Attribute, File, ItemMod};

#[derive(Debug)]
pub enum ModuleGraphError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: syn::Error,
    },
    AmbiguousModule {
        name: String,
        candidates: Vec<PathBuf>,
    },
    MissingModule {
        name: String,
        candidates: Vec<PathBuf>,
    },
}

impl std::fmt::Display for ModuleGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleGraphError::Read { path, source } => {
                write!(f, "could not read {}: {source}", path.display())
            }
            ModuleGraphError::Parse { path, source } => {
                write!(f, "could not parse {}: {source}", path.display())
            }
            ModuleGraphError::AmbiguousModule { name, candidates } => write!(
                f,
                "module `{name}` resolves to more than one file: {}",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ModuleGraphError::MissingModule { name, candidates } => write!(
                f,
                "module `{name}` does not resolve to any file; tried: {}",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for ModuleGraphError {}

/// Parses a source file into its AST.
pub fn parse(path: &Path) -> Result<File, ModuleGraphError> {
    let source = fs::read_to_string(path).map_err(|source| ModuleGraphError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    syn::parse_file(&source).map_err(|source| ModuleGraphError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolves the file a `mod name;` declaration refers to, given the
/// directory such declarations resolve against (a `#[path]` attribute
/// still overrides this).
pub fn resolve_child_path(
    children_dir: &Path,
    module: &ItemMod,
) -> Result<PathBuf, ModuleGraphError> {
    let name = module.ident.to_string();

    if let Some(explicit) = path_attribute(&module.attrs) {
        return Ok(children_dir.join(explicit));
    }

    let as_file = children_dir.join(format!("{name}.rs"));
    let as_dir = children_dir.join(&name).join("mod.rs");
    match (as_file.is_file(), as_dir.is_file()) {
        (true, true) => Err(ModuleGraphError::AmbiguousModule {
            name,
            candidates: vec![as_file, as_dir],
        }),
        (true, false) => Ok(as_file),
        (false, true) => Ok(as_dir),
        (false, false) => Err(ModuleGraphError::MissingModule {
            name,
            candidates: vec![as_file, as_dir],
        }),
    }
}

/// Where `resolved_file`'s own child `mod name;` declarations (without
/// `#[path]`) resolve: a `mod.rs` file (however it was reached) owns its
/// containing directory; any other file owns a same-named subdirectory of
/// `containing_dir` (the directory its *own* parent module's children
/// resolve under) — matching rustc even when `resolved_file` came from a
/// `#[path]` override, which does not change the implicit subdirectory
/// name for descendants.
pub fn children_dir_for(resolved_file: &Path, containing_dir: &Path, module_name: &str) -> PathBuf {
    if resolved_file.file_name().and_then(|n| n.to_str()) == Some("mod.rs") {
        resolved_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        containing_dir.join(module_name)
    }
}

fn path_attribute(attrs: &[Attribute]) -> Option<String> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("path"))
        .and_then(|a| {
            let syn::Meta::NameValue(nv) = &a.meta else {
                return None;
            };
            let syn::Expr::Lit(expr_lit) = &nv.value else {
                return None;
            };
            let syn::Lit::Str(lit) = &expr_lit.lit else {
                return None;
            };
            Some(lit.value())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("florui-build-test-{name}-{}", std::process::id()));
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

    fn only_mod<'a>(ast: &'a File, name: &str) -> &'a ItemMod {
        ast.items
            .iter()
            .find_map(|item| match item {
                syn::Item::Mod(m) if m.ident == name => Some(m),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no `mod {name}` found"))
    }

    #[test]
    fn resolves_a_sibling_file_module() {
        let dir = TempDir::new("sibling");
        dir.write("foo.rs", "pub fn f() {}");

        let path = resolve_child_path(
            &dir.0,
            only_mod(&syn::parse_str("mod foo;").unwrap(), "foo"),
        )
        .unwrap();
        assert!(path.ends_with("foo.rs"));
    }

    #[test]
    fn resolves_a_directory_module_via_mod_rs() {
        let dir = TempDir::new("dirmod");
        dir.write("foo/mod.rs", "");

        let path = resolve_child_path(
            &dir.0,
            only_mod(&syn::parse_str("mod foo;").unwrap(), "foo"),
        )
        .unwrap();
        assert!(path.ends_with("mod.rs"));

        let children_dir = children_dir_for(&path, &dir.0, "foo");
        assert_eq!(children_dir, dir.0.join("foo"));
    }

    #[test]
    fn a_plain_file_modules_children_live_in_a_same_named_subdirectory() {
        let dir = TempDir::new("plainfile");
        let path = dir.write("foo.rs", "");
        let children_dir = children_dir_for(&path, &dir.0, "foo");
        assert_eq!(children_dir, dir.0.join("foo"));
    }

    #[test]
    fn respects_an_explicit_path_attribute() {
        let dir = TempDir::new("explicit-path");
        dir.write("actual.rs", "pub fn f() {}");

        let ast: File = syn::parse_str("#[path = \"actual.rs\"] mod foo;").unwrap();
        let path = resolve_child_path(&dir.0, only_mod(&ast, "foo")).unwrap();
        assert!(path.ends_with("actual.rs"));
    }

    #[test]
    fn reports_a_missing_module_file() {
        let dir = TempDir::new("missing");
        let err = resolve_child_path(
            &dir.0,
            only_mod(&syn::parse_str("mod ghost;").unwrap(), "ghost"),
        )
        .unwrap_err();
        assert!(matches!(err, ModuleGraphError::MissingModule { .. }));
    }

    #[test]
    fn reports_an_ambiguous_module() {
        let dir = TempDir::new("ambiguous");
        dir.write("foo.rs", "");
        dir.write("foo/mod.rs", "");

        let err = resolve_child_path(
            &dir.0,
            only_mod(&syn::parse_str("mod foo;").unwrap(), "foo"),
        )
        .unwrap_err();
        assert!(matches!(err, ModuleGraphError::AmbiguousModule { .. }));
    }
}

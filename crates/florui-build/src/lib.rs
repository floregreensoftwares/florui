//! Build-time discovery of `stylesheet!` declarations across a crate's
//! module graph, in the deterministic order the cascade needs.
//!
//! A proc macro cannot see a crate's whole module tree from one
//! invocation, so this instead statically parses the crate's own source
//! files (starting from its root, following `mod` declarations the way
//! rustc itself would) with `syn`, entirely independent of macro
//! expansion, link order, or filesystem enumeration order.
//!
//! # Using this from a `build.rs`
//!
//! ```no_run
//! let package_root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
//! let package_name = std::env::var("CARGO_PKG_NAME").unwrap();
//! let crate_root = package_root.join("src/lib.rs");
//!
//! let result = florui_build::collect_stylesheets(
//!     &package_name,
//!     &package_root,
//!     &crate_root,
//!     &florui_build::cargo_feature_enabled,
//! )
//! .expect("collecting stylesheets should not fail");
//!
//! for file in &result.visited_rust_files {
//!     println!("cargo:rerun-if-changed={}", file.display());
//! }
//! for sheet in &result.stylesheets {
//!     println!("cargo:rerun-if-changed={}", sheet.css_path.display());
//! }
//!
//! let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
//! let manifest = florui_build::generate_manifest(&result.stylesheets);
//! std::fs::write(out_dir.join("florui_stylesheets.rs"), manifest).unwrap();
//! ```
//!
//! The generated file declares `pub static FLORUI_STYLESHEETS: &[florui::StylesheetSource]`;
//! bring it in with `include!(concat!(env!("OUT_DIR"), "/florui_stylesheets.rs"));`.

mod cfg;
mod codegen;
mod collect;
mod module_graph;
mod scan;

pub use cfg::cargo_feature_enabled;
pub use codegen::{MANIFEST_ITEM_NAME, generate_manifest};
pub use collect::{CollectError, CollectResult, CollectedStylesheet, collect_stylesheets};
pub use module_graph::ModuleGraphError;
pub use scan::ScanError;

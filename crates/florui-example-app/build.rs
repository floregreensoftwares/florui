//! Discovers this crate's `stylesheet!` declarations at build time and
//! writes them, in deterministic cascade order, to a generated module the
//! binary includes — see `florui_build`'s own docs for the general recipe.

fn main() {
    let package_root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let package_name = std::env::var("CARGO_PKG_NAME").unwrap();
    let crate_root = package_root.join("src/main.rs");

    let result = florui_build::collect_stylesheets(
        &package_name,
        &package_root,
        &crate_root,
        &florui_build::cargo_feature_enabled,
    )
    .expect("collecting this crate's stylesheet! declarations should not fail");

    for file in &result.visited_rust_files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    for sheet in &result.stylesheets {
        println!("cargo:rerun-if-changed={}", sheet.css_path.display());
    }

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest = florui_build::generate_manifest(&result.stylesheets);
    std::fs::write(out_dir.join("florui_stylesheets.rs"), manifest)
        .expect("writing the generated manifest should not fail");
}

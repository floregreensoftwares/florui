//! Discovers this crate's `stylesheet!` declarations at build time and
//! writes them, in deterministic cascade order, to a generated module the
//! binary includes — see `florui_build`'s own docs for the general recipe.
//! Also embeds the `two_windows` example's second-window icon (a literal
//! asset, independent of `florui.config.toml`) the same way, so the
//! shipped binary needs neither `resvg` nor the source SVG at runtime —
//! see `florui_icon::embed`'s own doc.

fn main() {
    let package_root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let package_name = std::env::var("CARGO_PKG_NAME").unwrap();
    // Components live in the library target; that is where the actual
    // module graph (and every stylesheet! declaration) is reachable from.
    let crate_root = package_root.join("src/lib.rs");

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

    let second_window_icon_svg = package_root.join("assets/icons/second-window.svg");
    println!(
        "cargo:rerun-if-changed={}",
        second_window_icon_svg.display()
    );
    florui_icon::embed::embed_icon_from_file(
        &out_dir,
        "second_window_icon",
        &second_window_icon_svg,
        florui_icon::DEFAULT_ICON_SIZE,
    )
    .expect("embedding the two_windows example's second-window icon should not fail");
}

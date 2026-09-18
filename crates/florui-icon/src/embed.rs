//! Embeds an already-decoded icon into a generated Rust source file at
//! build time, so a shipped binary never needs the source SVG/PNG on
//! disk -- the same `cargo:rerun-if-changed` + generated-file-under-
//! `OUT_DIR` idiom `florui-build`'s own stylesheet manifest already uses,
//! though none of that crate's actual code is reusable here.

use crate::{IconError, RawIcon, decode::load_icon_at_size};
use std::path::{Path, PathBuf};

/// Writes `icon`'s pixels to `{out_dir}/{name}.rgba` and a small generated
/// module at `{out_dir}/{name}.rs` declaring `pub static {NAME}_RGBA: &[u8]`
/// (via `include_bytes!`, not a printed numeric array -- cheaper to
/// compile for icon-sized pixel counts) plus `pub const {NAME}_WIDTH`/
/// `{NAME}_HEIGHT`. A consuming `build.rs` calls this; the consuming
/// binary then does `include!(concat!(env!("OUT_DIR"), "/{name}.rs"))`.
pub fn write_embedded_icon(out_dir: &Path, name: &str, icon: &RawIcon) -> std::io::Result<PathBuf> {
    let bin_path = out_dir.join(format!("{name}.rgba"));
    std::fs::write(&bin_path, &icon.rgba)?;

    let rs_path = out_dir.join(format!("{name}.rs"));
    let upper = name.to_ascii_uppercase();
    let module = format!(
        "pub static {upper}_RGBA: &[u8] = include_bytes!({bin_path:?});\n\
         pub const {upper}_WIDTH: u32 = {};\n\
         pub const {upper}_HEIGHT: u32 = {};\n",
        icon.width, icon.height,
    );
    std::fs::write(&rs_path, module)?;
    Ok(rs_path)
}

/// Decodes `source_path` once (see [`load_icon_at_size`]) and embeds the
/// result via [`write_embedded_icon`] -- the one call a `build.rs` needs.
pub fn embed_icon_from_file(
    out_dir: &Path,
    name: &str,
    source_path: &Path,
    size: u32,
) -> Result<PathBuf, IconError> {
    let icon = load_icon_at_size(source_path, size)?;
    write_embedded_icon(out_dir, name, &icon).map_err(|source| IconError::Io {
        path: out_dir.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <rect width="32" height="32" fill="#ff0000"/>
</svg>"##;

    #[test]
    fn write_embedded_icon_generates_rust_source_that_parses() {
        let dir = tempfile::tempdir().unwrap();
        let icon = crate::decode_svg(RED_SQUARE_SVG, 16).unwrap();
        let rs_path = write_embedded_icon(dir.path(), "test_icon", &icon).unwrap();
        let source = std::fs::read_to_string(rs_path).unwrap();
        syn::parse_str::<syn::File>(&source).expect("generated module should be valid Rust");
    }

    #[test]
    fn write_embedded_icon_constants_match_the_source_icons_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let icon = crate::decode_svg(RED_SQUARE_SVG, 16).unwrap();
        let rs_path = write_embedded_icon(dir.path(), "test_icon", &icon).unwrap();
        let source = std::fs::read_to_string(rs_path).unwrap();
        assert!(source.contains("TEST_ICON_WIDTH: u32 = 16"));
        assert!(source.contains("TEST_ICON_HEIGHT: u32 = 16"));
    }

    #[test]
    fn write_embedded_icon_writes_a_binary_sidecar_matching_the_icons_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let icon = crate::decode_svg(RED_SQUARE_SVG, 16).unwrap();
        write_embedded_icon(dir.path(), "test_icon", &icon).unwrap();
        let bytes = std::fs::read(dir.path().join("test_icon.rgba")).unwrap();
        assert_eq!(bytes, icon.rgba);
    }

    #[test]
    fn embed_icon_from_file_decodes_and_embeds_in_one_call() {
        let dir = tempfile::tempdir().unwrap();
        let svg_path = dir.path().join("source.svg");
        std::fs::write(&svg_path, RED_SQUARE_SVG).unwrap();
        let rs_path = embed_icon_from_file(dir.path(), "from_file", &svg_path, 8).unwrap();
        let source = std::fs::read_to_string(rs_path).unwrap();
        assert!(source.contains("FROM_FILE_WIDTH: u32 = 8"));
    }
}

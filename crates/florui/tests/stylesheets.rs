use florui::prelude::*;

stylesheet!("./fixtures/button.css");

#[test]
fn embeds_the_css_content_at_compile_time() {
    assert!(__FLORUI_STYLESHEET.css.contains("background: #42734f"));
}

#[test]
fn records_the_literal_path_for_diagnostics() {
    assert_eq!(__FLORUI_STYLESHEET.source_path, "./fixtures/button.css");
}

#[test]
fn identity_is_reproducible_and_scoped_to_the_declaring_file() {
    assert!(__FLORUI_STYLESHEET.id.starts_with("florui:"));
    assert!(__FLORUI_STYLESHEET.id.contains("stylesheets.rs"));
    assert!(__FLORUI_STYLESHEET.id.ends_with("./fixtures/button.css"));
}

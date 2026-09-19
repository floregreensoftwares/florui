//! Rewrites a `stylesheet_scoped!` declaration's CSS text before Stylo
//! ever parses it: every class selector's ident gets its scope's
//! [`StyleScope::suffix`] appended (`.box` -> `.box--a1b2c3d4`), so
//! two components that both declare `.box` compile to distinct, non-
//! colliding selectors, matching how `view!`'s `css_scope={...}` directive
//! suffixes the *elements'* own `class` attribute (`florui`'s
//! `apply_scope_to_class_attr`) — both sides apply the exact same suffix
//! format, so they agree without either depending on the other's code.
//!
//! `:global(<selector>)` unwraps to `<selector>`, copied through
//! unmodified — the documented escape hatch out of scoping.
//!
//! First documented subset (declared explicitly, not silently assumed):
//! only simple class selectors (`.name`) are rewritten. Combinators and
//! pseudo-classes compose for free since this only ever touches
//! `Delim('.')` immediately followed by an `Ident`, with everything else
//! copied through unchanged — including an attribute selector's value
//! string (`[href=".pdf"]`), which real tokenization (not a naive text
//! search) never misreads as a class selector. Contents of
//! `:is()`/`:where()`/`:not()` are copied through unrewritten in this
//! slice, a known, narrower-than-spec gap for a later slice — critically,
//! that narrowness is *safe* (proven by a throwaway `cssparser` round-trip
//! experiment against real Stylo before this was written): it does not
//! corrupt the selector, it just leaves those nested class names global.
//!
//! Uses `cssparser` directly (the same tokenizer Stylo itself is built
//! on), the same approach `height_media_adapter` uses for its own
//! pre-parse text rewrite. Parenthesized content that must be *removed*
//! (the `:global(...)` wrapper) needs `Parser::parse_nested_block` to find
//! the true matching close paren — a flat token loop that tries to strip
//! a wrapper by hand desyncs the moment cssparser auto-skips an unentered
//! nested block, exactly the bug the throwaway experiment caught before
//! this landed.

use std::borrow::Cow;

use cssparser::{Parser, ParserInput, Token};
use florui::StyleScope;

/// See the module doc. Returns `css` unchanged (borrowed, no allocation)
/// when it contains no class selector or `:global(` to rewrite.
pub(crate) fn scope_class_selectors(css: &str, scope: StyleScope) -> Cow<'_, str> {
    let suffix = scope.suffix();
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    scan(&mut parser, &suffix, &mut replacements);

    if replacements.is_empty() {
        return Cow::Borrowed(css);
    }
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0;
    for (start, end, replacement) in replacements {
        out.push_str(&css[cursor..start]);
        out.push_str(&replacement);
        cursor = end;
    }
    out.push_str(&css[cursor..]);
    Cow::Owned(out)
}

/// Walks `parser`'s token stream at whatever nesting level it was called
/// at, recording each rewrite as a `(start, end, replacement)` span. A
/// zero-width span (`start == end`) is a pure insertion (the scope suffix
/// right after a class ident); a non-empty span with an empty replacement
/// is a deletion (the `:global(` wrapper and its matching `)`).
fn scan(parser: &mut Parser, suffix: &str, replacements: &mut Vec<(usize, usize, String)>) {
    let mut prev_was_dot = false;
    let mut colon_start: Option<usize> = None;

    loop {
        let start = parser.position().byte_index();
        let Ok(token) = parser.next_including_whitespace_and_comments().cloned() else {
            break;
        };

        match &token {
            Token::Colon => {
                colon_start = Some(start);
                prev_was_dot = false;
                continue;
            }
            Token::Function(name)
                if name.eq_ignore_ascii_case("global") && colon_start.is_some() =>
            {
                let wrapper_start = colon_start.expect("just checked is_some");
                let function_end = parser.position().byte_index();
                // Contents are copied through unmodified — not recursed
                // into with `scan`, since `:global(...)`'s whole point is
                // to leave everything inside it untouched.
                let _ = parser.parse_nested_block::<_, (), ()>(|_inner| Ok(()));
                let after_close_paren = parser.position().byte_index();
                replacements.push((wrapper_start, function_end, String::new()));
                replacements.push((after_close_paren - 1, after_close_paren, String::new()));
                prev_was_dot = false;
            }
            Token::Delim('.') => {
                prev_was_dot = true;
            }
            Token::Ident(_) if prev_was_dot => {
                let end = parser.position().byte_index();
                replacements.push((end, end, suffix.to_string()));
                prev_was_dot = false;
            }
            _ => {
                prev_was_dot = false;
            }
        }
        colon_start = None;
    }
}

#[cfg(test)]
mod tests {
    use florui::StyleScope;

    use super::*;

    const SCOPE: StyleScope = StyleScope(0xa1b2c3d4);

    #[test]
    fn a_stylesheet_with_no_class_selector_is_returned_unchanged_and_unallocated() {
        let css = "* { color: red; }";
        let result = scope_class_selectors(css, SCOPE);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, css);
    }

    #[test]
    fn a_simple_class_selector_gets_the_suffix() {
        let result = scope_class_selectors(".box { color: red; }", SCOPE);
        assert_eq!(&*result, ".box--a1b2c3d4 { color: red; }");
    }

    #[test]
    fn a_pseudo_class_composes_correctly() {
        let result = scope_class_selectors(".box:hover { color: red; }", SCOPE);
        assert_eq!(&*result, ".box--a1b2c3d4:hover { color: red; }");
    }

    #[test]
    fn a_descendant_combinator_scopes_both_sides() {
        let result = scope_class_selectors(".card .box { color: red; }", SCOPE);
        assert_eq!(&*result, ".card--a1b2c3d4 .box--a1b2c3d4 { color: red; }");
    }

    #[test]
    fn multiple_classes_on_one_compound_selector_all_get_scoped() {
        let result = scope_class_selectors(".box.active { color: red; }", SCOPE);
        assert_eq!(&*result, ".box--a1b2c3d4.active--a1b2c3d4 { color: red; }");
    }

    #[test]
    fn global_unwraps_its_selector_untouched() {
        let result = scope_class_selectors(":global(.escape-hatch) { color: green; }", SCOPE);
        assert_eq!(&*result, ".escape-hatch { color: green; }");
    }

    #[test]
    fn global_leaves_a_multi_class_selector_inside_it_untouched() {
        let result = scope_class_selectors(":global(.a .b) { color: green; }", SCOPE);
        assert_eq!(&*result, ".a .b { color: green; }");
    }

    #[test]
    fn an_attribute_selector_value_containing_a_dot_is_never_misread_as_a_class() {
        let css = "[href=\".pdf\"] { color: black; }";
        let result = scope_class_selectors(css, SCOPE);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, css);
    }

    #[test]
    fn two_different_scopes_never_produce_the_same_selector_for_the_same_class() {
        let other = StyleScope(0xdeadbeef);
        let a = scope_class_selectors(".box { color: red; }", SCOPE);
        let b = scope_class_selectors(".box { color: red; }", other);
        assert_ne!(a, b);
    }
}

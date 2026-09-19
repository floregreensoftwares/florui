//! Servo's Stylo engine mode has no `prefers-reduced-motion` media feature
//! at all (only `width`, `scan`, `resolution`, `device-pixel-ratio`,
//! `prefers-color-scheme` -- see `height_media_adapter`'s own module doc
//! for the identical situation `height` was in). This rewrites each
//! `(prefers-reduced-motion: reduce)` / `(prefers-reduced-motion:
//! no-preference)` test inside an `@media` prelude into a `width`-based
//! stand-in Stylo does support -- `(width)` is always true for a real
//! screen, `(not (width))` always false -- before Stylo ever parses the
//! text. Only the matched test is replaced; `not`/`and`/`or`, nesting, and
//! every other feature are untouched.
//!
//! Uses `cssparser` (the same tokenizer Stylo itself is built on) so
//! comments/strings/escapes elsewhere in the stylesheet can't be misread as
//! a media condition.
//!
//! Supported syntax: the classic `(prefers-reduced-motion: reduce)` /
//! `(prefers-reduced-motion: no-preference)` colon form only. The valueless
//! boolean form (`(prefers-reduced-motion)`) isn't recognized and passes
//! through untouched -- Stylo's own missing-feature behavior applies to it
//! exactly as before. Anything this module can't confidently parse is left
//! alone rather than guessed at.

use std::borrow::Cow;

use cssparser::{BasicParseError, Delimiter, Parser, ParserInput, Token};

/// See the module doc. Returns `css` unchanged (borrowed, no allocation)
/// when it contains no "prefers-reduced-motion" text at all.
pub(crate) fn substitute_reduced_motion_feature(
    css: &str,
    prefers_reduced_motion: bool,
) -> Cow<'_, str> {
    if !css.to_ascii_lowercase().contains("prefers-reduced-motion") {
        return Cow::Borrowed(css);
    }

    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut replacements: Vec<(usize, usize, bool)> = Vec::new();

    loop {
        let token = match parser.next_including_whitespace() {
            Ok(token) => token.clone(),
            Err(_) => break,
        };
        if let Token::AtKeyword(name) = &token
            && name.eq_ignore_ascii_case("media")
        {
            let _: Result<(), cssparser::ParseError<'_, ()>> =
                parser.parse_until_before(Delimiter::CurlyBracketBlock, |prelude| {
                    scan_prelude(prelude, prefers_reduced_motion, &mut replacements);
                    Ok(())
                });
        }
        // Any block not explicitly entered is auto-skipped whole by the
        // next call (`Parser::next`'s own contract).
    }

    if replacements.is_empty() {
        return Cow::Borrowed(css);
    }
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0;
    for (start, end, is_true) in replacements {
        out.push_str(&css[cursor..start]);
        // Both forms stay wrapped in their own `(...)`: the span being
        // replaced was always itself a `<media-in-parens>` (an `and`/`or`
        // operand, or a `not (...)`'s own operand), a position that
        // requires outer parens even around a `not (width)` condition.
        out.push_str(if is_true { "(width)" } else { "(not (width))" });
        cursor = end;
    }
    out.push_str(&css[cursor..]);
    Cow::Owned(out)
}

/// Walks one `@media` prelude (or a group nested inside one), recording
/// each `(prefers-reduced-motion: ...)`-shaped leaf group found. A group
/// that isn't one of those is recursed into instead, in case it contains
/// one.
fn scan_prelude<'i>(
    parser: &mut Parser<'i, '_>,
    prefers_reduced_motion: bool,
    replacements: &mut Vec<(usize, usize, bool)>,
) {
    loop {
        let start = parser.position().byte_index();
        let token = match parser.next_including_whitespace() {
            Ok(token) => token.clone(),
            Err(_) => break,
        };
        let is_group = matches!(
            token,
            Token::ParenthesisBlock | Token::Function(_) | Token::SquareBracketBlock
        );
        if !is_group {
            continue;
        }

        let mut leaf: Option<bool> = None;
        let mut nested: Vec<(usize, usize, bool)> = Vec::new();
        let _: Result<(), cssparser::ParseError<'_, ()>> = parser.parse_nested_block(|inner| {
            let attempt: Result<bool, cssparser::ParseError<'_, ()>> =
                inner.try_parse(|leaf_parser| {
                    parse_reduced_motion_feature_leaf(leaf_parser, prefers_reduced_motion)
                });
            match attempt {
                Ok(is_true) if inner.expect_exhausted().is_ok() => {
                    leaf = Some(is_true);
                }
                _ => {
                    scan_prelude(inner, prefers_reduced_motion, &mut nested);
                }
            }
            Ok(())
        });

        let end = parser.position().byte_index();
        match leaf {
            Some(is_true) => replacements.push((start, end, is_true)),
            None => replacements.extend(nested),
        }
    }
}

/// Parses `prefers-reduced-motion : reduce|no-preference` and evaluates it.
/// Fails for any ident other than `prefers-reduced-motion`, or a value
/// other than `reduce`/`no-preference`.
fn parse_reduced_motion_feature_leaf<'i>(
    parser: &mut Parser<'i, '_>,
    prefers_reduced_motion: bool,
) -> Result<bool, cssparser::ParseError<'i, ()>> {
    let ident = parser.expect_ident().map_err(basic_to_custom)?.clone();
    if !ident.eq_ignore_ascii_case("prefers-reduced-motion") {
        return Err(parser
            .new_basic_unexpected_token_error(Token::Ident(ident))
            .into());
    }
    parser.expect_colon().map_err(basic_to_custom)?;
    let value = parser.expect_ident().map_err(basic_to_custom)?.clone();
    if value.eq_ignore_ascii_case("reduce") {
        Ok(prefers_reduced_motion)
    } else if value.eq_ignore_ascii_case("no-preference") {
        Ok(!prefers_reduced_motion)
    } else {
        Err(parser
            .new_basic_unexpected_token_error(Token::Ident(value))
            .into())
    }
}

fn basic_to_custom(error: BasicParseError<'_>) -> cssparser::ParseError<'_, ()> {
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stylesheet_with_no_reduced_motion_text_at_all_is_returned_unchanged_and_unallocated() {
        let css = ".card { background-color: #ff0000; }";
        let result = substitute_reduced_motion_feature(css, true);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, css);
    }

    #[test]
    fn reduce_matches_when_the_os_prefers_reduced_motion() {
        let css = "@media (prefers-reduced-motion: reduce) { .card { color: red; } }";
        let result = substitute_reduced_motion_feature(css, true);
        assert_eq!(&*result, "@media (width) { .card { color: red; } }");
    }

    #[test]
    fn reduce_does_not_match_when_the_os_has_no_preference() {
        let css = "@media (prefers-reduced-motion: reduce) { .card { color: red; } }";
        let result = substitute_reduced_motion_feature(css, false);
        assert_eq!(&*result, "@media (not (width)) { .card { color: red; } }");
    }

    #[test]
    fn no_preference_is_the_exact_inverse_of_reduce() {
        let matches_reduced = substitute_reduced_motion_feature(
            "@media (prefers-reduced-motion: no-preference) {}",
            true,
        );
        assert_eq!(&*matches_reduced, "@media (not (width)) {}");

        let matches_no_preference = substitute_reduced_motion_feature(
            "@media (prefers-reduced-motion: no-preference) {}",
            false,
        );
        assert_eq!(&*matches_no_preference, "@media (width) {}");
    }

    #[test]
    fn a_reduced_motion_test_composed_with_and_or_not_only_substitutes_the_leaf() {
        let css = "@media not ((prefers-reduced-motion: reduce) and (min-width: 400px)) { .a {} }";
        let result = substitute_reduced_motion_feature(css, true);
        assert_eq!(
            &*result,
            "@media not ((width) and (min-width: 400px)) { .a {} }"
        );
    }

    #[test]
    fn a_width_only_media_query_is_left_completely_untouched() {
        let css = "@media (min-width: 700px) { .card { color: red; } }";
        let result = substitute_reduced_motion_feature(css, true);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, css);
    }

    #[test]
    fn a_reduced_motion_mention_inside_a_declaration_value_is_not_touched() {
        let css = ".card { --label: \"prefers-reduced-motion: reduce wins\"; }";
        let result = substitute_reduced_motion_feature(css, true);
        assert_eq!(&*result, css);
    }

    #[test]
    fn multiple_media_blocks_each_get_their_own_independent_substitution() {
        let css = "@media (prefers-reduced-motion: reduce) { .a {} } \
                   @media (prefers-reduced-motion: no-preference) { .b {} }";
        let result = substitute_reduced_motion_feature(css, true);
        assert_eq!(
            &*result,
            "@media (width) { .a {} } @media (not (width)) { .b {} }"
        );
    }

    #[test]
    fn the_valueless_boolean_form_is_not_recognized_and_passes_through() {
        let css = "@media (prefers-reduced-motion) { .a {} }";
        let result = substitute_reduced_motion_feature(css, true);
        assert_eq!(&*result, css);
    }
}

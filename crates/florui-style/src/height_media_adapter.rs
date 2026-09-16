//! Stylo's own "servo" engine mode has no `height` media feature at
//! all (only `width`, `scan`, `resolution`, `device-pixel-ratio`,
//! `prefers-color-scheme`). This rewrites each `min-height`/
//! `max-height`/`height` test inside an `@media` prelude into a
//! `width`-based stand-in Stylo does support -- `(width)` is always
//! true for a real screen, `(not (width))` always false -- before Stylo
//! ever parses the text. Only the matched test is replaced; `not`/
//! `and`/`or`, nesting, and every other feature are untouched.
//!
//! Uses `cssparser` (the same tokenizer Stylo itself is built on) so
//! comments/strings/escapes elsewhere in the stylesheet can't be
//! misread as a media condition.
//!
//! Supported syntax: the classic `(min-height: <px>)` colon form only.
//! The newer range syntax (`height < 600px`) isn't recognized and
//! passes through untouched -- Stylo's own missing-feature behavior
//! applies to it exactly as before. Anything this module can't
//! confidently parse is left alone rather than guessed at.

use std::borrow::Cow;

use cssparser::{BasicParseError, Delimiter, Parser, ParserInput, Token};

/// See the module doc. Returns `css` unchanged (borrowed, no
/// allocation) when it contains no "height" text at all.
pub(crate) fn substitute_height_features(css: &str, viewport_height: f32) -> Cow<'_, str> {
    if !css.to_ascii_lowercase().contains("height") {
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
                    scan_prelude(prelude, viewport_height, &mut replacements);
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
/// each `(min-height: ...)`-shaped leaf group found. A group that isn't
/// one of those is recursed into instead, in case it contains one.
fn scan_prelude<'i>(
    parser: &mut Parser<'i, '_>,
    viewport_height: f32,
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
            let attempt: Result<bool, cssparser::ParseError<'_, ()>> = inner
                .try_parse(|leaf_parser| parse_height_feature_leaf(leaf_parser, viewport_height));
            match attempt {
                Ok(is_true) if inner.expect_exhausted().is_ok() => {
                    leaf = Some(is_true);
                }
                _ => {
                    scan_prelude(inner, viewport_height, &mut nested);
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

/// Parses `<ident> : <length>` and evaluates it. Fails for any ident
/// other than `height`/`min-height`/`max-height`.
fn parse_height_feature_leaf<'i>(
    parser: &mut Parser<'i, '_>,
    viewport_height: f32,
) -> Result<bool, cssparser::ParseError<'i, ()>> {
    let ident = parser.expect_ident().map_err(basic_to_custom)?.clone();
    let lower = ident.to_ascii_lowercase();
    if lower != "height" && lower != "min-height" && lower != "max-height" {
        return Err(parser
            .new_basic_unexpected_token_error(Token::Ident(ident))
            .into());
    }
    parser.expect_colon().map_err(basic_to_custom)?;
    let px = parse_px_length(parser)?;

    Ok(match lower.as_str() {
        "min-height" => viewport_height >= px,
        "max-height" => viewport_height <= px,
        _ => (viewport_height - px).abs() < 0.01,
    })
}

fn parse_px_length<'i>(parser: &mut Parser<'i, '_>) -> Result<f32, cssparser::ParseError<'i, ()>> {
    let token = parser.next().map_err(basic_to_custom)?.clone();
    match &token {
        Token::Dimension { value, unit, .. } if unit.eq_ignore_ascii_case("px") => Ok(*value),
        Token::Number { value, .. } if *value == 0.0 => Ok(0.0),
        _ => Err(parser.new_unexpected_token_error(token)),
    }
}

fn basic_to_custom(error: BasicParseError<'_>) -> cssparser::ParseError<'_, ()> {
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stylesheet_with_no_height_text_at_all_is_returned_unchanged_and_unallocated() {
        let css = ".card { background-color: #ff0000; }";
        let result = substitute_height_features(css, 600.0);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, css);
    }

    #[test]
    fn a_satisfied_min_height_becomes_an_always_true_width_test() {
        let css = "@media (min-height: 500px) { .card { color: red; } }";
        let result = substitute_height_features(css, 600.0);
        assert_eq!(&*result, "@media (width) { .card { color: red; } }");
    }

    #[test]
    fn an_unsatisfied_min_height_becomes_an_always_false_width_test() {
        let css = "@media (min-height: 500px) { .card { color: red; } }";
        let result = substitute_height_features(css, 400.0);
        assert_eq!(&*result, "@media (not (width)) { .card { color: red; } }");
    }

    #[test]
    fn max_height_and_exact_height_both_resolve_correctly() {
        let satisfied_max = substitute_height_features("@media (max-height: 500px) {}", 400.0);
        assert_eq!(&*satisfied_max, "@media (width) {}");

        let unsatisfied_max = substitute_height_features("@media (max-height: 500px) {}", 600.0);
        assert_eq!(&*unsatisfied_max, "@media (not (width)) {}");

        let exact = substitute_height_features("@media (height: 500px) {}", 500.0);
        assert_eq!(&*exact, "@media (width) {}");
    }

    #[test]
    fn a_height_test_composed_with_and_or_not_only_substitutes_the_height_leaf() {
        let css = "@media not ((min-height: 600px) and (min-width: 400px)) { .a {} }";
        let result = substitute_height_features(css, 300.0);
        assert_eq!(
            &*result,
            "@media not ((not (width)) and (min-width: 400px)) { .a {} }"
        );
    }

    #[test]
    fn a_width_only_media_query_is_left_completely_untouched() {
        let css = "@media (min-width: 700px) { .card { color: red; } }";
        let result = substitute_height_features(css, 600.0);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, css);
    }

    #[test]
    fn a_height_mention_inside_a_declaration_value_is_not_touched() {
        let css = ".card { --label: \"max-height wins\"; }";
        let result = substitute_height_features(css, 600.0);
        assert_eq!(&*result, css);
    }

    #[test]
    fn multiple_media_blocks_each_get_their_own_independent_substitution() {
        let css = "@media (min-height: 100px) { .a {} } @media (min-height: 900px) { .b {} }";
        let result = substitute_height_features(css, 500.0);
        assert_eq!(
            &*result,
            "@media (width) { .a {} } @media (not (width)) { .b {} }"
        );
    }
}

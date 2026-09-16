//! Bare text between tags: `<h2>Open source UI</h2>` instead of requiring
//! `<h2>{"Open source UI"}</h2>`.
//!
//! Rust's lexer runs before this macro ever sees a token, so bare text can
//! only contain characters that already form valid Rust tokens on their
//! own. An identifier directly followed by `'` with no space — exactly the
//! shape of an English contraction or possessive, `don't` or `Ada's` — is a
//! hard lex error on this edition: Rust has reserved `ident'` as an unknown
//! token prefix since the 2021 edition (rejected before any macro runs, so
//! there is no token stream for this module to fix up). An unterminated or
//! non-ASCII typographic quote (`“ ”`) fails the same way. A literal `<`
//! also still needs `{"..."}`, since that is how a nested tag starts —
//! mirroring HTML's own requirement to escape `<` in text.
//!
//! `proc-macro2` does not expose the original source spacing on stable
//! Rust, so consecutive words are joined with a single space — matching
//! CSS's own default collapsing of whitespace in text content — except for
//! a small set of leading punctuation that reads better attached to the
//! word before it (`Hello, world!` rather than `Hello , world !`) and `-`,
//! which glues on both sides (`well-known`, not `well - known`). Brackets,
//! braces, and parentheses always arrive as a single balanced [`TokenTree::Group`],
//! never as standalone punctuation, so a parenthesized aside is just one
//! token as far as this is concerned and gets a normal leading space.
//!
//! This same reconstruction is what `florui-fmt` re-emits for a text node
//! it reformats — not a new formatting decision of its own, just echoing
//! back the identical normalization this module already performs, so a
//! text run round-trips through formatting exactly as it already does
//! through real macro expansion.

use proc_macro2::{Delimiter, TokenTree};
use syn::Result;
use syn::buffer::Cursor;
use syn::parse::ParseStream;

/// No space before this token; it attaches to whatever came before it.
const ATTACHES_TO_PREVIOUS: &[&str] = &[",", ".", "!", "?", ";", ":"];
/// No space on either side (`well-known`, not `well - known`).
const GLUES_BOTH_SIDES: &[&str] = &["-"];

/// Consumes tokens up to the next `<` or `{` and reconstructs them into one
/// text run. Only called when the caller already knows the next token is
/// neither, so this always makes progress.
pub fn parse_text_run(input: ParseStream) -> Result<String> {
    let tokens = input.step(|cursor| {
        let mut collected = Vec::new();
        let mut rest = *cursor;
        while !is_boundary_cursor(rest) {
            match rest.token_tree() {
                Some((tt, next)) => {
                    collected.push(tt);
                    rest = next;
                }
                None => break,
            }
        }
        Ok((collected, rest))
    })?;
    Ok(reconstruct(&tokens))
}

fn is_boundary_cursor(cursor: Cursor) -> bool {
    if cursor.eof() {
        return true;
    }
    if let Some((punct, _)) = cursor.punct()
        && punct.as_char() == '<'
    {
        return true;
    }
    cursor.group(Delimiter::Brace).is_some()
}

fn reconstruct(tokens: &[TokenTree]) -> String {
    let mut text = String::new();
    let mut suppress_leading_space = true;

    for tt in tokens {
        let piece = tt.to_string();
        let glues = GLUES_BOTH_SIDES.contains(&piece.as_str());
        let attaches = glues || ATTACHES_TO_PREVIOUS.contains(&piece.as_str());

        if !suppress_leading_space && !attaches {
            text.push(' ');
        }
        text.push_str(&piece);
        suppress_leading_space = glues;
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn reconstruct_tokens(input: proc_macro2::TokenStream) -> String {
        reconstruct(&input.into_iter().collect::<Vec<_>>())
    }

    #[test]
    fn joins_bare_words_with_a_single_space() {
        assert_eq!(
            reconstruct_tokens(quote! { Open source UI }),
            "Open source UI"
        );
    }

    #[test]
    fn attaches_trailing_punctuation() {
        assert_eq!(
            reconstruct_tokens(quote! { Hello , world ! }),
            "Hello, world!"
        );
    }

    #[test]
    fn glues_hyphens_on_both_sides() {
        assert_eq!(
            reconstruct_tokens(quote! { a well - known example }),
            "a well-known example"
        );
    }

    #[test]
    fn parenthesized_asides_are_a_single_token() {
        // "(this)" always arrives as one Group token, never as separate
        // punctuation, so it is just another word with a leading space.
        assert_eq!(reconstruct_tokens(quote! { say (this) }), "say (this)");
    }
}

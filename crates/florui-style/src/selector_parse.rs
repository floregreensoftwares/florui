//! Parses the restricted selector grammar `crate::selector` describes: a
//! comma-separated list of descendant chains of compound selectors.
//!
//! This is a hand-written scanner, not a general CSS selector parser: it
//! is only ever asked to accept `type`, `.class`, `#id`, `:hover` /
//! `:focus` / `:active`, and whitespace as the descendant combinator.
//! Identifiers are only checked for non-emptiness, not validated against
//! CSS's full identifier grammar (escapes, unicode ranges, etc.).

use crate::error::StyleError;
use crate::selector::{CompoundSelector, PseudoClass, Selector, SimpleSelector};

/// Parses a comma-separated selector list into one [`Selector`] per
/// comma-separated entry, in the order written.
pub fn parse_selector_list(text: &str) -> Result<Vec<Selector>, StyleError> {
    text.split(',')
        .map(|part| parse_selector(part.trim()))
        .collect()
}

fn parse_selector(text: &str) -> Result<Selector, StyleError> {
    let compounds: Result<Vec<CompoundSelector>, StyleError> =
        text.split_whitespace().map(parse_compound).collect();
    let compounds = compounds?;
    if compounds.is_empty() {
        return Err(StyleError::EmptySelector);
    }
    Ok(Selector(compounds))
}

fn parse_compound(token: &str) -> Result<CompoundSelector, StyleError> {
    let mut simples = Vec::new();
    let mut cursor = 0;

    if !token.starts_with(['.', '#', ':']) {
        let end = token[cursor..]
            .find(['.', '#', ':'])
            .map(|i| cursor + i)
            .unwrap_or(token.len());
        if end == cursor {
            return Err(StyleError::EmptyCompoundSelector);
        }
        simples.push(SimpleSelector::Type(token[cursor..end].to_string()));
        cursor = end;
    }

    while cursor < token.len() {
        let marker = token[cursor..].chars().next().expect("cursor < len");
        let rest = &token[cursor + marker.len_utf8()..];
        let end = rest.find(['.', '#', ':']).unwrap_or(rest.len());
        let name = &rest[..end];
        if name.is_empty() {
            return Err(StyleError::EmptyCompoundSelector);
        }
        simples.push(match marker {
            '.' => SimpleSelector::Class(name.to_string()),
            '#' => SimpleSelector::Id(name.to_string()),
            ':' => SimpleSelector::Pseudo(parse_pseudo_class(name)?),
            _ => unreachable!("loop only advances to a `.`, `#`, or `:` marker"),
        });
        cursor += marker.len_utf8() + end;
    }

    if simples.is_empty() {
        return Err(StyleError::EmptyCompoundSelector);
    }
    Ok(CompoundSelector(simples))
}

fn parse_pseudo_class(name: &str) -> Result<PseudoClass, StyleError> {
    match name {
        "hover" => Ok(PseudoClass::Hover),
        "focus" => Ok(PseudoClass::Focus),
        "active" => Ok(PseudoClass::Active),
        other => Err(StyleError::UnsupportedPseudoClass(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_bare_type_selector() {
        let list = parse_selector_list("div").unwrap();
        assert_eq!(
            list,
            vec![Selector(vec![CompoundSelector(vec![
                SimpleSelector::Type("div".into())
            ])])]
        );
    }

    #[test]
    fn parses_a_compound_selector() {
        let list = parse_selector_list("button.primary:hover").unwrap();
        assert_eq!(
            list,
            vec![Selector(vec![CompoundSelector(vec![
                SimpleSelector::Type("button".into()),
                SimpleSelector::Class("primary".into()),
                SimpleSelector::Pseudo(PseudoClass::Hover),
            ])])]
        );
    }

    #[test]
    fn parses_a_descendant_chain() {
        let list = parse_selector_list("div .card").unwrap();
        assert_eq!(
            list,
            vec![Selector(vec![
                CompoundSelector(vec![SimpleSelector::Type("div".into())]),
                CompoundSelector(vec![SimpleSelector::Class("card".into())]),
            ])]
        );
    }

    #[test]
    fn parses_a_comma_separated_list_into_separate_selectors() {
        let list = parse_selector_list(".a, .b").unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn rejects_an_unsupported_pseudo_class() {
        assert!(parse_selector_list(":not(.a)").is_err());
        assert!(parse_selector_list("a:visited").is_err());
    }

    #[test]
    fn rejects_an_empty_selector_component() {
        assert!(parse_selector_list(".").is_err());
        assert!(parse_selector_list("#").is_err());
    }

    #[test]
    fn id_selector_alone_has_no_type() {
        let list = parse_selector_list("#main").unwrap();
        assert_eq!(
            list,
            vec![Selector(vec![CompoundSelector(vec![SimpleSelector::Id(
                "main".into()
            )])])]
        );
    }
}

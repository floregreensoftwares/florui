//! Parses CSS text into [`Rule`]s: `selector-list { declaration; ... }`,
//! repeated. No at-rules, no nesting, no strings — comments are stripped
//! before anything else runs.

use crate::color::parse_hex_color;
use crate::error::StyleError;
use crate::selector::Selector;
use crate::selector_parse::parse_selector_list;
use crate::value::{Property, Value, ValueKind};

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: Property,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
    /// Position among all rules from this stylesheet, in source order —
    /// the cascade's tie-breaker when specificity is equal.
    pub source_order: usize,
}

pub fn parse_stylesheet(css: &str) -> Result<Vec<Rule>, StyleError> {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    let mut rest = css.as_str();
    let mut source_order = 0;

    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let open = rest.find('{').ok_or(StyleError::UnclosedBlock)?;
        let selector_text = &rest[..open];
        let close = rest[open + 1..]
            .find('}')
            .ok_or(StyleError::UnclosedBlock)?;
        let body = &rest[open + 1..open + 1 + close];

        let declarations = parse_declarations(body)?;
        for selector in parse_selector_list(selector_text)? {
            rules.push(Rule {
                selector,
                declarations: declarations.clone(),
                source_order,
            });
        }
        source_order += 1;

        rest = &rest[open + 1 + close + 1..];
    }

    Ok(rules)
}

fn parse_declarations(body: &str) -> Result<Vec<Declaration>, StyleError> {
    body.split(';')
        .map(str::trim)
        .filter(|decl| !decl.is_empty())
        .map(parse_declaration)
        .collect()
}

fn parse_declaration(text: &str) -> Result<Declaration, StyleError> {
    let (name, value_text) = text.split_once(':').ok_or(StyleError::EmptyDeclaration)?;
    let name = name.trim();
    let property =
        Property::parse(name).ok_or_else(|| StyleError::UnsupportedProperty(name.to_string()))?;

    let value_text = value_text.trim();
    let value_text = match value_text.strip_suffix("!important") {
        Some(_) => {
            return Err(StyleError::UnsupportedImportant {
                property: name.to_string(),
            });
        }
        None => value_text,
    };

    let value =
        match value_text {
            "inherit" => Value::Inherit,
            "initial" => Value::Initial,
            "auto" if property.value_kind() == ValueKind::LengthOrAuto => Value::Auto,
            other => match property.value_kind() {
                ValueKind::Color => parse_hex_color(other).map(Value::Color).map_err(|_| {
                    StyleError::InvalidValue {
                        property: name.to_string(),
                        value: other.to_string(),
                    }
                })?,
                ValueKind::Length | ValueKind::LengthOrAuto => parse_length(other)
                    .map(Value::Length)
                    .ok_or_else(|| StyleError::InvalidValue {
                        property: name.to_string(),
                        value: other.to_string(),
                    })?,
            },
        };

    Ok(Declaration { property, value })
}

/// `Npx` or the unitless `0` CSS allows for lengths.
fn parse_length(text: &str) -> Option<f32> {
    if text == "0" {
        return Some(0.0);
    }
    text.strip_suffix("px")?.parse().ok()
}

/// Strips `/* ... */` comments. Not CSS-string-aware (our value grammar
/// has no strings), so `/*` inside a color literal would be mishandled —
/// acceptable since hex colors never contain it.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    #[test]
    fn parses_a_single_rule() {
        let rules = parse_stylesheet(".button { background-color: #42734f; }").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].declarations,
            vec![Declaration {
                property: Property::BackgroundColor,
                value: Value::Color(Rgba::opaque(0x42, 0x73, 0x4f))
            }]
        );
    }

    #[test]
    fn expands_a_comma_separated_selector_list_into_one_rule_per_selector() {
        let rules = parse_stylesheet(".a, .b { color: #fff; }").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].source_order, rules[1].source_order);
    }

    #[test]
    fn tracks_source_order_across_multiple_rules() {
        let rules = parse_stylesheet(".a { color: #000; } .b { color: #fff; }").unwrap();
        assert_eq!(rules[0].source_order, 0);
        assert_eq!(rules[1].source_order, 1);
    }

    #[test]
    fn strips_comments_before_parsing() {
        let rules = parse_stylesheet("/* note */ .a { /* inline */ color: #fff; }").unwrap();
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn rejects_an_unsupported_property() {
        assert!(parse_stylesheet(".a { border: 1px solid black; }").is_err());
    }

    #[test]
    fn rejects_important() {
        assert!(parse_stylesheet(".a { color: #fff !important; }").is_err());
    }

    #[test]
    fn rejects_an_unclosed_block() {
        assert!(parse_stylesheet(".a { color: #fff;").is_err());
    }

    #[test]
    fn accepts_inherit_and_initial_keywords() {
        let rules = parse_stylesheet(".a { color: inherit; background-color: initial; }").unwrap();
        assert_eq!(rules[0].declarations[0].value, Value::Inherit);
        assert_eq!(rules[0].declarations[1].value, Value::Initial);
    }

    #[test]
    fn parses_pixel_lengths_for_layout_properties() {
        let rules = parse_stylesheet(".a { width: 200px; margin-top: 0; }").unwrap();
        assert_eq!(
            rules[0].declarations[0],
            Declaration {
                property: Property::Width,
                value: Value::Length(200.0)
            }
        );
        assert_eq!(
            rules[0].declarations[1],
            Declaration {
                property: Property::MarginTop,
                value: Value::Length(0.0)
            }
        );
    }

    #[test]
    fn accepts_auto_for_width_height_and_margin_but_not_padding() {
        let rules = parse_stylesheet(".a { width: auto; margin-left: auto; }").unwrap();
        assert_eq!(rules[0].declarations[0].value, Value::Auto);
        assert_eq!(rules[0].declarations[1].value, Value::Auto);

        assert!(parse_stylesheet(".a { padding-left: auto; }").is_err());
    }

    #[test]
    fn rejects_a_color_value_for_a_length_property() {
        assert!(parse_stylesheet(".a { width: #ff0000; }").is_err());
    }

    #[test]
    fn rejects_a_length_without_a_unit() {
        assert!(parse_stylesheet(".a { width: 10; }").is_err());
    }
}

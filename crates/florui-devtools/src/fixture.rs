//! Loads the single-element CSS fixture used by the native preview host.
//!
//! Scope is deliberately tiny: one `body { background-color: ... }` rule.
//! There is no selector matching, cascade, or inheritance here yet; this is
//! a placeholder for real module-owned stylesheet collection and computed
//! style.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::color::{ColorParseError, Rgba};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixture {
    pub background: Rgba,
}

#[derive(Debug)]
pub enum FixtureError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    MissingRule {
        path: PathBuf,
    },
    InvalidColor {
        path: PathBuf,
        source: ColorParseError,
    },
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixtureError::Read { path, source } => {
                write!(f, "could not read fixture {}: {source}", path.display())
            }
            FixtureError::MissingRule { path } => write!(
                f,
                "fixture {} has no `body {{ background-color: ... }}` rule",
                path.display()
            ),
            FixtureError::InvalidColor { path, source } => {
                write!(f, "fixture {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for FixtureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FixtureError::Read { source, .. } => Some(source),
            FixtureError::InvalidColor { source, .. } => Some(source),
            FixtureError::MissingRule { .. } => None,
        }
    }
}

/// Reads `path` and extracts the `body` rule's `background-color`.
///
/// This scans for the literal text `body` followed by a `{ ... }` block and
/// a `background-color:` declaration inside it. It is not a CSS tokenizer:
/// comments, nested braces, strings containing `{`/`}`, and any selector
/// other than a bare `body` are unsupported and rejected rather than guessed.
pub fn load_fixture(path: &Path) -> Result<Fixture, FixtureError> {
    let source = std::fs::read_to_string(path).map_err(|source| FixtureError::Read {
        path: path.to_owned(),
        source,
    })?;

    let block = extract_body_block(&source).ok_or_else(|| FixtureError::MissingRule {
        path: path.to_owned(),
    })?;

    let value = extract_declaration_value(block, "background-color").ok_or_else(|| {
        FixtureError::MissingRule {
            path: path.to_owned(),
        }
    })?;

    let background =
        crate::color::parse_hex_color(value).map_err(|source| FixtureError::InvalidColor {
            path: path.to_owned(),
            source,
        })?;

    Ok(Fixture { background })
}

fn extract_body_block(source: &str) -> Option<&str> {
    let body_index = source.find("body")?;
    let after_selector = &source[body_index + "body".len()..];
    let open = after_selector.find('{')?;
    let close = after_selector[open..].find('}')?;
    Some(&after_selector[open + 1..open + close])
}

fn extract_declaration_value<'a>(block: &'a str, property: &str) -> Option<&'a str> {
    for declaration in block.split(';') {
        let (name, value) = declaration.split_once(':')?;
        if name.trim() == property {
            return Some(value.trim());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_background_color() {
        let css = "body {\n  background-color: #42734f;\n}\n";
        let block = extract_body_block(css).unwrap();
        assert_eq!(
            extract_declaration_value(block, "background-color"),
            Some("#42734f")
        );
    }

    #[test]
    fn missing_rule_is_reported() {
        assert!(extract_body_block("div { color: red; }").is_none());
    }
}

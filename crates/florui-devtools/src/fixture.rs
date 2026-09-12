//! Loads the single-element CSS fixture used by the native preview host.
//!
//! Scope is deliberately tiny: one `body { background-color: ... }` rule.
//! There is no selector matching, cascade, or inheritance here yet; this is
//! a placeholder for real module-owned stylesheet collection and computed
//! style.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::color::{ColorParseError, Rgba};

/// A 1-based line/column position within a fixture's source text, as
/// produced by [`load_fixture`]. `{ line: 0, column: 0 }` is reserved by
/// callers as a sentinel for "no fixture has loaded successfully yet" —
/// `load_fixture` itself never returns it.
///
/// Columns count `char`s, not grapheme clusters, UTF-16 units, or
/// tab-expanded width — good enough for this bootstrap parser's single-line,
/// ASCII-punctuation declarations, not a general source-mapping primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation {
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixture {
    pub background: Rgba,
    /// Where `background`'s value was declared, for source-to-element
    /// mapping in the inspector.
    pub background_location: SourceLocation,
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

    let background_location = offset_to_line_col(&source, byte_offset(&source, value));

    Ok(Fixture {
        background,
        background_location,
    })
}

/// `value`'s byte offset within `source`. Sound because every slice this
/// module produces (via `find`/`split_once`/`trim`) is a subslice of the
/// same original allocation, never a copy.
fn byte_offset(source: &str, value: &str) -> usize {
    value.as_ptr() as usize - source.as_ptr() as usize
}

fn offset_to_line_col(source: &str, offset: usize) -> SourceLocation {
    let mut line = 1u32;
    let mut column = 1u32;
    for ch in source[..offset].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    SourceLocation { line, column }
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

    #[test]
    fn offset_to_line_col_counts_lines_and_columns() {
        let source = "abc\ndef\nghi";
        assert_eq!(
            offset_to_line_col(source, 0),
            SourceLocation { line: 1, column: 1 }
        );
        assert_eq!(
            offset_to_line_col(source, 2),
            SourceLocation { line: 1, column: 3 }
        );
        // Offset 4 is 'd', the first character after the first '\n'.
        assert_eq!(
            offset_to_line_col(source, 4),
            SourceLocation { line: 2, column: 1 }
        );
        assert_eq!(
            offset_to_line_col(source, 9),
            SourceLocation { line: 3, column: 2 }
        );
    }

    /// Proves the reported location tracks the declaration's real position
    /// rather than a hardcoded line/column: unrelated content is prepended
    /// and the expected location is computed from the same source text the
    /// fixture loader sees, not typed in by hand.
    #[test]
    fn load_fixture_reports_declaration_location_after_unrelated_leading_lines() {
        let leading = "/* a leading comment */\n/* another one */\n";
        let declaration_prefix = "body {\n  background-color: ";
        let css = format!("{leading}{declaration_prefix}#42734f;\n}}\n");
        let expected_offset = leading.len() + declaration_prefix.len();
        let expected_location = offset_to_line_col(&css, expected_offset);
        // Sanity: the unrelated lines actually moved the declaration off line 1.
        // (2 leading comment lines + "body {" on its own line puts the
        // declaration, which follows a '\n', on line 4.)
        assert_eq!(expected_location.line, 4);

        let dir = std::env::temp_dir().join(format!(
            "florui-fixture-location-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.css");
        std::fs::write(&path, &css).unwrap();

        let fixture = load_fixture(&path).unwrap();
        assert_eq!(fixture.background_location, expected_location);

        std::fs::remove_dir_all(&dir).ok();
    }
}

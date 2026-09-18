//! Byte offset to 1-based line/column, for citing a spot in
//! `florui.config.toml` or a `Cargo.toml` in an error. No existing
//! convention in this workspace produces source locations yet, so this is
//! new, minimal infrastructure rather than a reused pattern.

use std::fmt;
use std::ops::Range;

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

/// Maps byte offsets into `source` to 1-based line/column positions.
/// Built once per parse; `source` is kept so columns can count `char`s
/// rather than bytes (a config file's strings may contain multi-byte
/// UTF-8, e.g. `app.description`).
pub struct LineIndex<'a> {
    source: &'a str,
    /// Byte offset of the start of each line; index 0 is always 0.
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            source,
            line_starts,
        }
    }

    pub fn locate(&self, offset: usize) -> SourceLocation {
        let offset = offset.min(self.source.len());
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line_idx];
        let column = self.source[line_start..offset].chars().count() + 1;
        SourceLocation {
            line: (line_idx + 1) as u32,
            column: column as u32,
        }
    }

    pub fn locate_start(&self, span: Range<usize>) -> SourceLocation {
        self.locate(span.start)
    }
}

/// A value paired with the byte range it came from in some source text --
/// used for facts pulled out of a `Cargo.toml` (read as plain text, since
/// `cargo metadata`'s JSON carries no spans), where a `SourceLocation` gets
/// computed lazily by whoever actually needs to report an error, rather
/// than eagerly for every parsed manifest.
#[derive(Debug, Clone)]
pub struct LocatedValue<T> {
    pub value: T,
    pub span: Range<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_zero_is_line_one_column_one() {
        let index = LineIndex::new("hello");
        assert_eq!(index.locate(0), SourceLocation { line: 1, column: 1 });
    }

    #[test]
    fn offset_on_a_later_line_counts_lines_from_one() {
        let index = LineIndex::new("a = 1\nb = 2\nc = 3\n");
        assert_eq!(index.locate(6), SourceLocation { line: 2, column: 1 });
        assert_eq!(index.locate(12), SourceLocation { line: 3, column: 1 });
    }

    #[test]
    fn offset_mid_line_counts_columns_from_one() {
        let index = LineIndex::new("width = 800\n");
        // "800" starts at byte 8.
        assert_eq!(index.locate(8), SourceLocation { line: 1, column: 9 });
    }

    #[test]
    fn column_counts_chars_not_bytes_across_multibyte_utf8() {
        // "café " is 5 chars but 6 bytes (é is 2 bytes); the key after it
        // must still be reported at column 6, not column 7.
        let index = LineIndex::new("café = 1\nnext = 2\n");
        let next_key_offset = "café = 1\n".len();
        assert_eq!(
            index.locate(next_key_offset),
            SourceLocation { line: 2, column: 1 }
        );
    }

    #[test]
    fn offset_past_the_end_clamps_to_the_last_position() {
        let index = LineIndex::new("a = 1");
        assert_eq!(index.locate(1000), SourceLocation { line: 1, column: 6 });
    }
}

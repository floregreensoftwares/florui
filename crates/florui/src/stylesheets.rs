//! Module-owned stylesheet declarations: `stylesheet!("./button.css")`.
//!
//! This covers path resolution relative to the declaring file, a
//! reproducible canonical identity, compile-time embedding, and
//! identity-based deduplication — each independently correct and tested.
//!
//! Automatically discovering every `stylesheet!` declaration across a
//! crate's module graph, in the deterministic, depth-first,
//! source-order-respecting sequence the cascade needs, is not this
//! module's job either: a proc macro cannot see a crate's whole module
//! tree from a single invocation. That collection lives in the separate
//! `florui-build` crate, which statically parses a crate's own source
//! tree from a `build.rs` instead. [`dedup`] here only removes repeated
//! identities from whatever order it is given — `florui-build` is what
//! decides that order.

use std::collections::HashSet;

/// One declared stylesheet: a canonical identity, the path as written (for
/// diagnostics), its CSS content embedded at compile time, and — for a
/// `stylesheet_scoped!` declaration only — the scope its class selectors
/// and matching elements must agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StylesheetSource {
    pub id: &'static str,
    pub source_path: &'static str,
    pub css: &'static str,
    pub scope: Option<StyleScope>,
}

/// FNV-1a over `input`'s bytes, truncated to 32 bits. `const fn` so a
/// `concat!`-built identity string (package name, `file!()`, literal path —
/// see [`StylesheetSource::id`]) can be hashed at the *final* crate's
/// compile time, by generated code, exactly like `id` itself is computed
/// there: neither `florui-macros` (a proc macro, which only ever sees
/// syntax) nor `florui-build` (a separate build-script crate) can evaluate
/// `file!()`/`env!()` themselves, so both instead emit a call to this same
/// function over the same id-shaped string — guaranteeing byte-identical
/// hashing without either side needing to reimplement or agree on a
/// separate hash function.
pub const fn style_scope_hash(input: &str) -> u32 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let bytes = input.as_bytes();
    let mut hash = FNV_OFFSET;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash as u32
}

/// A `stylesheet_scoped!` declaration's identity, applied to both its own
/// selectors (by `florui-style`'s rewrite pass) and the elements its
/// `view!` template renders (via [`apply_scope_to_class_attr`]) so the two
/// sides always agree on the same literal suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleScope(pub u32);

impl StyleScope {
    pub const fn new(id: &str) -> Self {
        StyleScope(style_scope_hash(id))
    }

    /// The literal text appended to every locally-scoped class token, e.g.
    /// `.box` becomes `.box--a1b2c3d4`. Eight lowercase hex digits: always
    /// a valid (if unusual-looking) trailing segment of a CSS identifier,
    /// regardless of the scope value.
    pub fn suffix(&self) -> String {
        format!("--{:08x}", self.0)
    }
}

/// Appends [`StyleScope::suffix`] to every whitespace-separated token
/// in a `class="..."` attribute value — used by `view!`-generated code for
/// an element under a `css_scope={...}` directive.
pub fn apply_scope_to_class_attr(value: &str, scope: StyleScope) -> String {
    let suffix = scope.suffix();
    value
        .split_whitespace()
        .map(|token| format!("{token}{suffix}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Keeps the first occurrence of each `id`, preserving the input order.
///
/// Two different files whose contents happen to match keep separate
/// entries, since dedup is by identity, not by content.
pub fn dedup(sources: &[StylesheetSource]) -> Vec<StylesheetSource> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .filter(|source| seen.insert(source.id))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: StylesheetSource = StylesheetSource {
        id: "pkg:a.rs:./x.css",
        source_path: "./x.css",
        css: "a {}",
        scope: None,
    };
    const A_AGAIN: StylesheetSource = StylesheetSource {
        id: "pkg:a.rs:./x.css",
        source_path: "./x.css",
        css: "a {}",
        scope: None,
    };
    const B_SAME_CONTENT: StylesheetSource = StylesheetSource {
        id: "pkg:b.rs:./y.css",
        source_path: "./y.css",
        css: "a {}",
        scope: None,
    };

    #[test]
    fn keeps_first_occurrence_of_each_identity() {
        assert_eq!(dedup(&[A, A_AGAIN]), vec![A]);
    }

    #[test]
    fn distinct_identities_with_identical_content_both_survive() {
        assert_eq!(dedup(&[A, B_SAME_CONTENT]), vec![A, B_SAME_CONTENT]);
    }

    #[test]
    fn first_occurrence_wins_regardless_of_which_copy_is_kept() {
        // A and A_AGAIN are distinct values with the same id; dedup must
        // keep the one that appeared first, not merge or prefer either
        // arbitrarily.
        let result = dedup(&[A_AGAIN, A]);
        assert_eq!(result, vec![A_AGAIN]);
    }

    #[test]
    fn scope_hash_is_deterministic_for_the_same_id() {
        assert_eq!(
            style_scope_hash("pkg:src/x.rs:./x.css"),
            style_scope_hash("pkg:src/x.rs:./x.css")
        );
    }

    #[test]
    fn scope_hash_differs_for_different_ids() {
        assert_ne!(
            style_scope_hash("pkg:src/a.rs:./a.css"),
            style_scope_hash("pkg:src/b.rs:./b.css")
        );
    }

    #[test]
    fn suffix_is_always_eight_lowercase_hex_digits() {
        for id in [
            "pkg:a.rs:./x.css",
            "",
            "pkg:very/deeply/nested/mod.rs:./y.css",
        ] {
            let suffix = StyleScope::new(id).suffix();
            let hex = suffix.strip_prefix("--").expect("suffix starts with --");
            assert_eq!(hex.len(), 8);
            assert!(
                hex.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn apply_scope_suffixes_every_class_token() {
        let scope = StyleScope::new("pkg:a.rs:./x.css");
        let rewritten = apply_scope_to_class_attr("box card", scope);
        let suffix = scope.suffix();
        assert_eq!(rewritten, format!("box{suffix} card{suffix}"));
    }

    #[test]
    fn two_different_scopes_never_collide_for_the_same_class_name() {
        let a = StyleScope::new("pkg:a.rs:./a.css");
        let b = StyleScope::new("pkg:b.rs:./b.css");
        assert_ne!(
            apply_scope_to_class_attr("box", a),
            apply_scope_to_class_attr("box", b)
        );
    }
}

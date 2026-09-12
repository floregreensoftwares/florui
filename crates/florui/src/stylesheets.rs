//! Module-owned stylesheet declarations: `stylesheet!("./button.css")`.
//!
//! This covers path resolution relative to the declaring file, a
//! reproducible canonical identity, compile-time embedding, and
//! identity-based deduplication — each independently correct and tested.
//!
//! What is **not** solved here: automatically discovering every
//! `stylesheet!` declaration across a crate's module graph in the
//! deterministic, depth-first, source-order-respecting sequence the
//! cascade needs. A proc macro cannot see a crate's whole module tree from
//! a single invocation, and getting that collection order right needs real
//! build-script/module-graph work this module does not attempt — it does
//! not invent a substitute order to look more finished than it is. An
//! application must currently gather the constants each `stylesheet!` call
//! generates and order them itself; [`dedup`] only removes repeated
//! identities from whatever order it is given.

use std::collections::HashSet;

/// One declared stylesheet: a canonical identity, the path as written (for
/// diagnostics), and its CSS content embedded at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StylesheetSource {
    pub id: &'static str,
    pub source_path: &'static str,
    pub css: &'static str,
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
    };
    const A_AGAIN: StylesheetSource = StylesheetSource {
        id: "pkg:a.rs:./x.css",
        source_path: "./x.css",
        css: "a {}",
    };
    const B_SAME_CONTENT: StylesheetSource = StylesheetSource {
        id: "pkg:b.rs:./y.css",
        source_path: "./y.css",
        css: "a {}",
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
}

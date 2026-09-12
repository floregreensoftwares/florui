//! A "did you mean" suggestion for an unrecognized tag, so a typo like
//! `<divv>` points at `div` instead of just failing.

use crate::registry::PRIMITIVES;

const MAX_SUGGESTION_DISTANCE: usize = 2;

/// The closest known tag to `tag` by edit distance, or `None` when nothing
/// is close enough to be a plausible typo rather than an unrelated word.
pub fn suggest(tag: &str) -> Option<&'static str> {
    PRIMITIVES
        .iter()
        .map(|p| (p.tag, levenshtein(tag, p.tag)))
        .filter(|(_, distance)| *distance <= MAX_SUGGESTION_DISTANCE)
        .min_by_key(|(_, distance)| *distance)
        .map(|(tag, _)| tag)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();

    for (i, &ca) in a.iter().enumerate() {
        let mut prev_diag = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let temp = row[j + 1];
            row[j + 1] = if ca == cb {
                prev_diag
            } else {
                1 + prev_diag.min(row[j]).min(row[j + 1])
            };
            prev_diag = temp;
        }
    }

    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_a_close_misspelling() {
        assert_eq!(suggest("divv"), Some("div"));
        assert_eq!(suggest("buttn"), Some("button"));
    }

    #[test]
    fn suggests_nothing_for_unrelated_input() {
        assert_eq!(suggest("xyzxyzxyz"), None);
    }

    #[test]
    fn levenshtein_distance_matches_known_values() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("div", "div"), 0);
    }
}

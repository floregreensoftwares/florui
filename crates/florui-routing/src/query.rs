//! Minimal, dependency-free query-string helpers -- not every route has
//! a query string, and the ones that do rarely need more than this: a
//! full query-string crate would be more machinery than any route in
//! this codebase actually needs. A [`crate::Routable`] impl that needs
//! something richer is free to ignore these and parse its own.

/// Splits `raw` on its first `?`, if any. `path` never includes the
/// `?` itself; `query` is `None` when there wasn't one at all
/// (distinct from `Some("")`, an explicit empty query string).
pub fn split_query(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (raw, None),
    }
}

/// Decodes `query` (the part after `?`, without it) into `key=value`
/// pairs, `&`-separated, `+` treated as a space, and both key and value
/// percent-decoded. A pair with no `=` decodes to an empty value. An
/// empty segment (a leading/trailing/doubled `&`) is skipped rather than
/// producing a spurious empty pair.
pub fn decode_query_pairs(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 3 <= bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    // Not valid hex after all -- keep the '%' literally
                    // rather than silently eating the following bytes.
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_query_with_no_question_mark_has_no_query() {
        assert_eq!(split_query("/users/1"), ("/users/1", None));
    }

    #[test]
    fn split_query_splits_on_the_first_question_mark_only() {
        assert_eq!(split_query("/search?q=a?b"), ("/search", Some("q=a?b")));
    }

    #[test]
    fn split_query_keeps_an_explicit_empty_query_distinct_from_none() {
        assert_eq!(split_query("/page?"), ("/page", Some("")));
    }

    #[test]
    fn decode_query_pairs_reads_ordinary_pairs() {
        let pairs: Vec<_> = decode_query_pairs("a=1&b=2").collect();
        assert_eq!(
            pairs,
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned())
            ]
        );
    }

    #[test]
    fn decode_query_pairs_defaults_a_missing_value_to_empty() {
        let pairs: Vec<_> = decode_query_pairs("flag").collect();
        assert_eq!(pairs, vec![("flag".to_owned(), String::new())]);
    }

    #[test]
    fn decode_query_pairs_skips_empty_segments() {
        let pairs: Vec<_> = decode_query_pairs("a=1&&b=2&").collect();
        assert_eq!(
            pairs,
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned())
            ]
        );
    }

    #[test]
    fn decode_query_pairs_decodes_plus_as_space() {
        let pairs: Vec<_> = decode_query_pairs("q=a+b").collect();
        assert_eq!(pairs, vec![("q".to_owned(), "a b".to_owned())]);
    }

    #[test]
    fn decode_query_pairs_percent_decodes_both_key_and_value() {
        let pairs: Vec<_> = decode_query_pairs("a%20b=c%2Fd").collect();
        assert_eq!(pairs, vec![("a b".to_owned(), "c/d".to_owned())]);
    }

    #[test]
    fn decode_query_pairs_keeps_a_trailing_lone_percent_literal() {
        let pairs: Vec<_> = decode_query_pairs("a=100%").collect();
        assert_eq!(pairs, vec![("a".to_owned(), "100%".to_owned())]);
    }

    #[test]
    fn decode_query_pairs_keeps_a_malformed_escape_literal() {
        let pairs: Vec<_> = decode_query_pairs("a=100%zz").collect();
        assert_eq!(pairs, vec![("a".to_owned(), "100%zz".to_owned())]);
    }
}

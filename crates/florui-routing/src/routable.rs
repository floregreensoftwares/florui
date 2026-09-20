//! [`Routable`]: the typed route contract every app-defined route enum
//! implements. Deliberately not a derive macro -- a route enum is
//! ordinary application code, and its parsing/formatting rules are
//! specific enough (which segments are literal, which are typed
//! parameters, what an optional one defaults to) that hand-writing both
//! halves is clearer than fighting a macro's generated code to get an
//! edge case right.

/// A route value that can be recovered from, and rendered back to, a
/// path string. `format` then `parse` must round-trip: `Self::parse(&self.format())`
/// is `Ok(value)` where `value == *self`, for every value the app's own
/// enum can construct. Route matching is ordinary Rust `match` against
/// the parsed value -- this trait only owns the string boundary.
///
/// A route nested inside another (e.g. a variant holding its own
/// sub-route enum) formats with a leading `/`, the same as a top-level
/// route -- a parent composes `format!("/settings{}", sub.format())`
/// rather than inventing its own separator convention.
pub trait Routable: Clone + PartialEq + 'static {
    /// `path` is a single raw string -- no query string has been split
    /// off for you. A route with query parameters is responsible for
    /// finding and decoding its own (see [`crate::query`] for a small,
    /// optional helper), since not every route has one and the exact
    /// shape (which keys are recognized, how a duplicate is handled) is
    /// route-specific.
    fn parse(path: &str) -> Result<Self, RouteError>;

    /// The canonical string form of this route -- also used as this
    /// route's disposal identity by [`crate::route_outlet`], so two
    /// values that are `==` must format identically and vice versa.
    fn format(&self) -> String;
}

/// Why [`Routable::parse`] failed. Kept as two distinct shapes because
/// they call for different UX: an unrecognized path is usually a "page
/// not found" fallback route, while a recognized-but-malformed one is
/// usually a "that link looks broken" message -- an app that doesn't
/// need the distinction can still match both the same way.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteError {
    /// Nothing in this route enum matches `path`'s shape at all.
    Unknown { path: String },
    /// `path` matched a known route's shape, but a parameter inside it
    /// failed to parse -- `reason` is for diagnostics, not shown to a
    /// user verbatim.
    Invalid { path: String, reason: String },
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::Unknown { path } => write!(f, "unknown route: {path}"),
            RouteError::Invalid { path, reason } => {
                write!(f, "invalid route {path}: {reason}")
            }
        }
    }
}

impl std::error::Error for RouteError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::assert_route_round_trips;

    #[derive(Debug, Clone, PartialEq)]
    enum TestRoute {
        Home,
        User { id: u32 },
    }

    impl Routable for TestRoute {
        fn parse(path: &str) -> Result<Self, RouteError> {
            if path == "/" {
                return Ok(TestRoute::Home);
            }
            let Some(id_str) = path.strip_prefix("/users/") else {
                return Err(RouteError::Unknown {
                    path: path.to_owned(),
                });
            };
            let id = id_str.parse::<u32>().map_err(|error| RouteError::Invalid {
                path: path.to_owned(),
                reason: error.to_string(),
            })?;
            Ok(TestRoute::User { id })
        }

        fn format(&self) -> String {
            match self {
                TestRoute::Home => "/".to_owned(),
                TestRoute::User { id } => format!("/users/{id}"),
            }
        }
    }

    #[test]
    fn home_round_trips() {
        assert_route_round_trips(&TestRoute::Home);
    }

    #[test]
    fn a_user_route_round_trips() {
        assert_route_round_trips(&TestRoute::User { id: 42 });
    }

    #[test]
    fn an_unrecognized_shape_is_unknown() {
        assert_eq!(
            TestRoute::parse("/frobnicate"),
            Err(RouteError::Unknown {
                path: "/frobnicate".to_owned()
            })
        );
    }

    #[test]
    fn a_recognized_shape_with_a_bad_parameter_is_invalid_not_unknown() {
        let result = TestRoute::parse("/users/not-a-number");
        assert!(
            matches!(result, Err(RouteError::Invalid { .. })),
            "a malformed parameter inside a known route shape must be Invalid, not Unknown: {result:?}"
        );
    }

    #[test]
    fn route_error_display_never_panics() {
        let _ = RouteError::Unknown {
            path: "/x".to_owned(),
        }
        .to_string();
        let _ = RouteError::Invalid {
            path: "/x".to_owned(),
            reason: "bad".to_owned(),
        }
        .to_string();
    }
}

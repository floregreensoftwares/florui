//! A small test helper for [`crate::Routable`]'s round-trip law -- not a
//! property-testing framework, just the one assertion every route enum
//! needs repeated for each of its own variants.

use crate::routable::Routable;

/// Asserts `route` survives `format` then `parse` unchanged. Panics with
/// the formatted path and the parse error/mismatch on failure.
pub fn assert_route_round_trips<R: Routable + std::fmt::Debug>(route: &R) {
    let formatted = route.format();
    let parsed = R::parse(&formatted).unwrap_or_else(|error| {
        panic!("{route:?} formatted to {formatted:?}, which failed to parse back: {error}")
    });
    assert_eq!(
        &parsed, route,
        "{route:?} formatted to {formatted:?}, which parsed back to a different route: {parsed:?}"
    );
}

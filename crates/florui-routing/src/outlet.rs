//! [`route_outlet`]: the disposal boundary a matched route mounts under.

use florui::Element;
use florui_reactive::use_child_scope_keyed;

use crate::routable::Routable;

/// Mounts `render(current)` in a scope keyed on `current.format()` --
/// [`florui_reactive::use_child_scope_keyed`] disposes the previous
/// scope (every signal, effect, and nested outlet it owns) the instant
/// the key changes, which is exactly "leaving a route disposes it by
/// default": there is no separate imperative unmount step, only calling
/// this again with a different `current` value.
///
/// Reusable at any nesting depth for any [`Routable`] type -- a route
/// enum variant holding its own sub-route enum calls this again, from
/// inside the outer call's own `render` closure, keyed on the sub-route
/// instead. Not tied to [`crate::Router`]'s own top-level history; a
/// nested outlet just needs *some* [`Routable`] value to key on, however
/// its parent obtained it.
pub fn route_outlet<R: Routable>(current: &R, render: impl FnOnce(&R) -> Element) -> Element {
    let key = current.format();
    let current = current.clone();
    use_child_scope_keyed(key, move || render(&current))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use florui_reactive::{Cleanup, ComponentScope, use_effect};

    use super::*;
    use crate::routable::RouteError;

    #[derive(Debug, Clone, PartialEq)]
    enum TestRoute {
        A,
        B,
    }

    impl Routable for TestRoute {
        fn parse(path: &str) -> Result<Self, RouteError> {
            match path {
                "/a" => Ok(TestRoute::A),
                "/b" => Ok(TestRoute::B),
                _ => Err(RouteError::Unknown {
                    path: path.to_owned(),
                }),
            }
        }

        fn format(&self) -> String {
            match self {
                TestRoute::A => "/a".to_owned(),
                TestRoute::B => "/b".to_owned(),
            }
        }
    }

    fn mount_with_cleanup_flag(disposed: &Rc<Cell<bool>>) -> Element {
        let disposed = Rc::clone(disposed);
        use_effect((), move || {
            Some(Box::new(move || disposed.set(true)) as Cleanup)
        });
        Element::Fragment(Vec::new())
    }

    #[test]
    fn the_same_route_across_renders_stays_mounted() {
        let (scope, _dirty) = ComponentScope::new();
        let disposed = Rc::new(Cell::new(false));

        scope.render(|| route_outlet(&TestRoute::A, |_| mount_with_cleanup_flag(&disposed)));
        scope.render(|| route_outlet(&TestRoute::A, |_| mount_with_cleanup_flag(&disposed)));

        assert!(
            !disposed.get(),
            "rendering the same route again must not dispose its subtree"
        );
    }

    #[test]
    fn leaving_a_route_disposes_its_mounted_subtree() {
        let (scope, _dirty) = ComponentScope::new();
        let disposed = Rc::new(Cell::new(false));

        scope.render(|| route_outlet(&TestRoute::A, |_| mount_with_cleanup_flag(&disposed)));
        assert!(!disposed.get());

        // A different route this render -- the previous key is no longer
        // touched, so its whole scope (and this effect's cleanup) drops.
        scope.render(|| route_outlet(&TestRoute::B, |_| Element::Fragment(Vec::new())));

        assert!(
            disposed.get(),
            "leaving a route must dispose the subtree it owned"
        );
    }
}

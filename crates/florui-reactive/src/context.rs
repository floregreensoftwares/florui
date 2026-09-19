//! Provide/read a value by type via [`provide_context`]/[`use_context`].

use std::any::TypeId;

use crate::scope::ACTIVE_SCOPES;

/// Makes `value` available to [`use_context`] calls in this render and any
/// scope nested inside it via [`ComponentScope::render`](crate::ComponentScope::render) —
/// not to the scope's own future renders, which must provide it again.
///
/// # Panics
///
/// Panics if called outside a [`ComponentScope::render`](crate::ComponentScope::render) pass.
pub fn provide_context<T: 'static>(value: T) {
    ACTIVE_SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        let scope = scopes.last().unwrap_or_else(|| {
            panic!(
                "provide_context called outside of ComponentScope::render — hooks must run \
                 during a component tree's render pass"
            )
        });
        scope
            .context
            .borrow_mut()
            .insert(TypeId::of::<T>(), Box::new(value));
    })
}

/// Reads the nearest value of type `T` provided by [`provide_context`],
/// searching outward from the current scope through every scope it is
/// nested inside. `None` if nothing enclosing provided one.
///
/// # Panics
///
/// Panics if called outside a [`ComponentScope::render`](crate::ComponentScope::render) pass.
pub fn use_context<T: Clone + 'static>() -> Option<T> {
    ACTIVE_SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        if scopes.is_empty() {
            panic!(
                "use_context called outside of ComponentScope::render — hooks must run \
                 during a component tree's render pass"
            );
        }
        let type_id = TypeId::of::<T>();
        scopes.iter().rev().find_map(|scope| {
            scope
                .context
                .borrow()
                .get(&type_id)?
                .downcast_ref::<T>()
                .cloned()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComponentScope;

    #[test]
    fn use_context_returns_none_when_nothing_provided() {
        let (scope, _dirty) = ComponentScope::new();
        let value = scope.render(use_context::<i32>);
        assert_eq!(value, None);
    }

    #[test]
    fn use_context_sees_a_value_provided_earlier_in_the_same_render() {
        let (scope, _dirty) = ComponentScope::new();
        let value = scope.render(|| {
            provide_context(42_i32);
            use_context::<i32>()
        });
        assert_eq!(value, Some(42));
    }

    #[test]
    fn a_nested_scope_sees_the_outer_scopes_context() {
        let (outer, _) = ComponentScope::new();
        let (inner, _) = ComponentScope::new();
        outer.render(|| {
            provide_context("from outer");
            inner.render(|| {
                assert_eq!(use_context::<&str>(), Some("from outer"));
            });
        });
    }

    #[test]
    fn the_nearest_provider_wins_over_an_outer_one() {
        let (outer, _) = ComponentScope::new();
        let (inner, _) = ComponentScope::new();
        outer.render(|| {
            provide_context("outer");
            inner.render(|| {
                provide_context("inner");
                assert_eq!(use_context::<&str>(), Some("inner"));
            });
        });
    }

    #[test]
    fn a_later_render_must_reprovide_context_or_it_is_gone() {
        let (scope, _dirty) = ComponentScope::new();
        scope.render(|| provide_context(1_i32));
        let value = scope.render(use_context::<i32>);
        assert_eq!(
            value, None,
            "context from a previous render must not leak into one that never provided it"
        );
    }

    #[test]
    #[should_panic(expected = "use_context called outside of ComponentScope::render")]
    fn use_context_outside_a_render_panics() {
        use_context::<i32>();
    }

    #[test]
    #[should_panic(expected = "provide_context called outside of ComponentScope::render")]
    fn provide_context_outside_a_render_panics() {
        provide_context(1);
    }
}

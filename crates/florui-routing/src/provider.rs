//! [`provide_router`]: what an app calls inside its own `root()` to
//! create and provide a [`Router`] -- the same "plain generic fn an
//! app's own `#[component]` calls into" shape as
//! [`florui_reactive::loading_boundary`]/[`florui_reactive::error_boundary`],
//! not a `#[component]` itself.

use std::rc::Rc;

use florui::Element;
use florui_reactive::executor::Executor;
use florui_reactive::{provide_context, use_context, use_effect, use_ref, use_signal};

use crate::routable::Routable;
use crate::router::{Guard, HistoryEntry, NavKind, Router};

/// Creates (on first render) or resumes (on every later render) this
/// route type's [`Router`] and provides it via context for
/// [`use_router`]/[`use_route`]/[`route_outlet`](crate::route_outlet) to
/// read, then renders `children`. `provide_context` is re-run
/// unconditionally on every call -- a provided value is cleared at the
/// start of every render, so this must never be skipped or gated behind
/// a condition.
///
/// # Panics
///
/// Panics if no [`Executor`] is reachable via
/// [`florui_reactive::use_context`] -- the same requirement
/// [`florui_reactive::use_resource`] already has; provide one (e.g.
/// `Rc::new(florui_reactive::executor::LocalExecutor::new()) as Rc<dyn Executor>`)
/// once at the app's render root.
pub fn provide_router<R: Routable>(
    initial: R,
    guards: Vec<Guard<R>>,
    children: impl FnOnce() -> Element,
) -> Element {
    let entries = use_signal(|| {
        vec![HistoryEntry {
            route: initial,
            scroll_anchor: None,
        }]
    });
    let index = use_signal(|| 0usize);
    let generation = use_ref(|| 0u64);
    let transition_generation = use_signal(|| 0u64);
    let last_transition_kind = use_ref(|| NavKind::Replace);
    let executor = use_context::<Rc<dyn Executor>>().unwrap_or_else(|| {
        panic!(
            "provide_router needs an Executor reachable via use_context -- provide one \
             (e.g. Rc::new(florui_reactive::executor::LocalExecutor::new()) as Rc<dyn Executor>) \
             at the app's render root, the same one use_resource itself requires"
        )
    });
    let router = Router::new(
        entries,
        index,
        Rc::new(guards),
        generation,
        executor,
        transition_generation,
        last_transition_kind,
    );
    provide_context(router);
    children()
}

/// Reads the [`Router`] a [`provide_router`] above this component
/// provided.
///
/// # Panics
///
/// Panics if there is no matching `provide_router::<R>` anywhere above
/// this call in the tree.
pub fn use_router<R: Routable>() -> Router<R> {
    use_context::<Router<R>>()
        .unwrap_or_else(|| panic!("use_router::<R> called with no provide_router::<R> above it"))
}

/// The current route -- shorthand for `use_router::<R>().current()`.
pub fn use_route<R: Routable>() -> R {
    use_router::<R>().current()
}

/// `on_committed` runs once after every navigation that actually commits
/// (including the very first render, reported as [`NavKind::Replace`] --
/// there is no prior navigation to distinguish it from). Use this for
/// app-driven focus/scroll restoration: [`crate::Router`] has no
/// viewport or window handle of its own to act on either directly.
pub fn use_route_transition<R: Routable>(on_committed: impl Fn(NavKind) + 'static) {
    let router = use_router::<R>();
    let generation = router.transition_generation_value();
    let router_for_effect = router;
    use_effect(generation, move || {
        on_committed(router_for_effect.last_transition_kind());
        None
    });
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use florui_reactive::ComponentScope;
    use florui_reactive::executor::LocalExecutor;

    use super::*;
    use crate::routable::RouteError;

    #[derive(Debug, Clone, PartialEq)]
    enum TestRoute {
        Home,
        About,
    }

    impl Routable for TestRoute {
        fn parse(path: &str) -> Result<Self, RouteError> {
            match path {
                "/" => Ok(TestRoute::Home),
                "/about" => Ok(TestRoute::About),
                _ => Err(RouteError::Unknown {
                    path: path.to_owned(),
                }),
            }
        }

        fn format(&self) -> String {
            match self {
                TestRoute::Home => "/".to_owned(),
                TestRoute::About => "/about".to_owned(),
            }
        }
    }

    fn with_executor_provided<T>(scope: &ComponentScope, render: impl FnOnce() -> T) -> T {
        let executor = Rc::new(LocalExecutor::new()) as Rc<dyn Executor>;
        scope.render(move || {
            provide_context(executor);
            render()
        })
    }

    fn empty_page() -> Element {
        Element::Fragment(Vec::new())
    }

    #[test]
    fn use_route_reads_the_initial_route() {
        let (scope, _dirty) = ComponentScope::new();
        let route = with_executor_provided(&scope, || {
            provide_router(TestRoute::Home, Vec::new(), empty_page);
            use_route::<TestRoute>()
        });
        assert_eq!(route, TestRoute::Home);
    }

    #[test]
    fn the_router_persists_navigation_across_renders() {
        let (scope, _dirty) = ComponentScope::new();
        let router = with_executor_provided(&scope, || {
            provide_router(TestRoute::Home, Vec::new(), empty_page);
            use_router::<TestRoute>()
        });
        router.push(TestRoute::About);

        // provide_context is cleared at the start of every render, so
        // provide_router re-provides a "new" Router value each time -- but
        // its Signal/Ref fields are the same persisted cells from the
        // first render (same call-site slots), so the navigation above
        // must still be visible on this second render.
        let route_on_second_render = with_executor_provided(&scope, || {
            provide_router(TestRoute::Home, Vec::new(), empty_page);
            use_route::<TestRoute>()
        });
        assert_eq!(route_on_second_render, TestRoute::About);
    }

    #[test]
    #[should_panic(expected = "no provide_router")]
    fn use_router_without_a_provider_panics() {
        let (scope, _dirty) = ComponentScope::new();
        scope.render(|| {
            let _ = use_router::<TestRoute>();
        });
    }

    #[test]
    #[should_panic(expected = "Executor reachable via use_context")]
    fn provide_router_without_an_executor_panics() {
        let (scope, _dirty) = ComponentScope::new();
        scope.render(|| {
            provide_router(TestRoute::Home, Vec::new(), empty_page);
        });
    }
}

//! [`use_error_boundary`]: a composable boundary for explicit, recoverable
//! render/resource failures within a subtree.
//!
//! This crate never catches a Rust panic, an abort, or corrupted native
//! state — there is no `catch_unwind` here, and none is planned; those
//! are not recoverable application failures, they are bugs or fatal
//! conditions. An error boundary only ever receives a *typed* error a
//! descendant chose to report, explicitly, via [`ErrorReporter::report`]
//! — a failure never becomes a render error merely because a boundary
//! exists somewhere above it. A component that can fail (an event
//! handler, an async resource) is responsible for catching its own
//! failure and reporting it; nothing here does that automatically.

use crate::scope::use_child_scope_keyed;
use crate::signal::Signal;
use crate::{provide_context, use_signal};

/// Reports a typed error to the nearest enclosing
/// [`use_error_boundary::<E>`](use_error_boundary) — obtained via
/// [`use_context`], the same "nearest provider wins" rule context already
/// follows. Get one either from [`ErrorBoundary::reporter`] (if a
/// component holds the boundary itself) or `use_context::<ErrorReporter<E>>()`
/// (for a descendant that only needs to report, not display, the error).
#[derive(Clone)]
pub struct ErrorReporter<E> {
    error: Signal<Option<E>>,
}

impl<E: Clone + 'static> ErrorReporter<E> {
    /// Reports `error`, marking this boundary's owning scope dirty so it
    /// re-renders and picks up the failure.
    pub fn report(&self, error: E) {
        self.error.set(Some(error));
    }
}

/// The state [`use_error_boundary`] returns: whether this boundary
/// currently has a reported failure, and how to reset it.
pub struct ErrorBoundary<E> {
    error: Signal<Option<E>>,
    generation: Signal<u64>,
}

impl<E: Clone + 'static> ErrorBoundary<E> {
    /// The failure currently reported to this boundary, if any — `Some`
    /// means the caller should render its fallback instead of the
    /// protected subtree; `None` means render normally.
    pub fn error(&self) -> Option<E> {
        self.error.get()
    }

    /// Identifies this boundary's current "attempt" at rendering its
    /// protected subtree — wrap that subtree in
    /// `use_child_scope_keyed(boundary.generation(), || ...)` so
    /// [`Self::reset`] disposes its previous state (signals, effects,
    /// nested boundaries) instead of resuming whatever it was when it
    /// failed, the documented default per the project's error-boundary
    /// contract.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Clears the reported error and advances [`Self::generation`], so
    /// the next render of a subtree keyed on it mounts fresh.
    pub fn reset(&self) {
        self.error.set(None);
        self.generation.set(self.generation.get() + 1);
    }

    /// A reporter descendants can call to report a failure to this exact
    /// boundary — normally not needed directly; a descendant reporting
    /// its own failure should call `use_context::<ErrorReporter<E>>()`
    /// instead, so it always reaches whichever boundary is nearest to
    /// *it*, not necessarily this one.
    pub fn reporter(&self) -> ErrorReporter<E> {
        ErrorReporter {
            error: self.error.clone(),
        }
    }
}

/// Establishes an error boundary for failures of type `E` in whatever
/// this call's component renders. Provides an [`ErrorReporter<E>`] to
/// [`use_context`] for descendants — but only while this boundary has no
/// error of its own: a failure reported *while already rendering this
/// boundary's own fallback* has nothing useful to retry against here, so
/// it is left to reach the next enclosing boundary instead, per this
/// project's "propagate to the next boundary" contract for a failing
/// fallback.
///
/// # Panics
///
/// Panics outside a [`ComponentScope::render`](crate::ComponentScope::render) pass, or if
/// hooks ran in a different order or count than last render.
pub fn use_error_boundary<E: Clone + 'static>() -> ErrorBoundary<E> {
    let error = use_signal(|| None::<E>);
    let generation = use_signal(|| 0_u64);
    let boundary = ErrorBoundary {
        error: error.clone(),
        generation,
    };
    if error.get().is_none() {
        provide_context(boundary.reporter());
    }
    boundary
}

/// Convenience for the common shape: render `children` inside this
/// boundary's own [`ErrorBoundary::generation`]-keyed scope when there is
/// no error, or `fallback(error, boundary)` when there is — so a typical
/// boundary component's whole body is one call, without hand-writing the
/// `match`/`use_child_scope_keyed` every time. Equivalent to calling
/// [`use_error_boundary`] and doing that match by hand; use the longer
/// form directly for anything this shape doesn't fit.
pub fn error_boundary<E: Clone + 'static, T>(
    children: impl FnOnce() -> T,
    fallback: impl FnOnce(E, &ErrorBoundary<E>) -> T,
) -> T {
    let boundary = use_error_boundary::<E>();
    match boundary.error() {
        Some(error) => fallback(error, &boundary),
        None => use_child_scope_keyed(boundary.generation(), children),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ComponentScope, use_context};

    #[derive(Debug, Clone, PartialEq)]
    struct DemoError(&'static str);

    #[test]
    fn no_error_renders_children_via_a_keyed_scope() {
        let (scope, _dirty) = ComponentScope::new();
        let value =
            scope.render(|| error_boundary::<DemoError, _>(|| "children", |_, _| "fallback"));
        assert_eq!(value, "children");
    }

    #[test]
    fn a_reported_error_switches_to_the_fallback() {
        let (scope, _dirty) = ComponentScope::new();

        // First render: no error, but a descendant reports one.
        scope.render(|| {
            error_boundary::<DemoError, _>(
                || {
                    let reporter = use_context::<ErrorReporter<DemoError>>()
                        .expect("boundary above provided a reporter");
                    reporter.report(DemoError("boom"));
                    "children"
                },
                |_, _| "fallback",
            )
        });

        // Second render sees the reported error.
        let value = scope.render(|| {
            error_boundary::<DemoError, _>(
                || "children",
                |error, _| {
                    assert_eq!(error, DemoError("boom"));
                    "fallback"
                },
            )
        });
        assert_eq!(value, "fallback");
    }

    #[test]
    fn reset_clears_the_error_and_advances_generation() {
        let (scope, _dirty) = ComponentScope::new();

        let generation_before = scope.render(|| {
            let boundary = use_error_boundary::<DemoError>();
            boundary.reporter().report(DemoError("boom"));
            boundary.generation()
        });

        // This render sees the error reported by the previous one, and
        // resets it.
        let generation_at_reset = scope.render(|| {
            let boundary = use_error_boundary::<DemoError>();
            assert_eq!(boundary.error(), Some(DemoError("boom")));
            boundary.reset();
            boundary.generation()
        });
        assert_eq!(
            generation_at_reset,
            generation_before + 1,
            "reset must advance the generation"
        );

        let error_after_reset = scope.render(|| use_error_boundary::<DemoError>().error());
        assert_eq!(error_after_reset, None);
    }

    #[test]
    fn resetting_disposes_the_previous_generations_state() {
        let (scope, _dirty) = ComponentScope::new();
        let disposed = std::rc::Rc::new(std::cell::Cell::new(false));

        let mount_child = |disposed: &std::rc::Rc<std::cell::Cell<bool>>| {
            let disposed = std::rc::Rc::clone(disposed);
            move || {
                crate::use_effect((), move || {
                    Some(Box::new(move || disposed.set(true)) as crate::Cleanup)
                });
            }
        };

        // Render 1: mounts the child inside the boundary's generation-0
        // keyed scope.
        scope.render(|| error_boundary::<DemoError, _>(mount_child(&disposed), |_, _| ()));
        assert!(!disposed.get());

        // Render 2: a descendant reports a failure; the boundary itself
        // hasn't re-rendered past that report yet, so the child is still
        // mounted (and still keyed on generation 0) this same render.
        scope.render(|| {
            error_boundary::<DemoError, _>(
                || {
                    let reporter = use_context::<ErrorReporter<DemoError>>().unwrap();
                    reporter.report(DemoError("boom"));
                    mount_child(&disposed)()
                },
                |_, _| (),
            )
        });
        assert!(
            !disposed.get(),
            "the report alone must not dispose anything before the boundary re-renders past it"
        );

        // Render 3: the boundary now sees the error and renders its
        // fallback, which resets — this is the render where the
        // generation-0 child stops being touched at all and is pruned.
        scope.render(|| {
            error_boundary::<DemoError, _>(
                || unreachable!("must not render children while an error is pending"),
                |_, boundary| boundary.reset(),
            )
        });

        assert!(
            disposed.get(),
            "resetting must dispose the failed generation's state, not resume it"
        );
    }

    /// One tree body used across all three renders below, so the hook
    /// shape (which positional/keyed hooks run, and in what order) stays
    /// identical every render — only the reported errors driving which
    /// branches get taken change between calls.
    fn nested_boundaries(scope: &ComponentScope) -> DemoError {
        scope.render(|| {
            error_boundary::<DemoError, _>(
                || {
                    error_boundary::<DemoError, _>(
                        || DemoError("inner children: no error reported here"),
                        |_inner_error, _inner_boundary| {
                            // The inner boundary is showing its own
                            // fallback; reporting here must be found by
                            // the *outer* boundary, not loop back into
                            // this one, which has nothing useful left to
                            // retry.
                            let reporter = use_context::<ErrorReporter<DemoError>>()
                                .expect("an outer boundary is still active");
                            reporter.report(DemoError("fallback also failed"));
                            DemoError("inner fallback return value, unused")
                        },
                    )
                },
                |error, _| error,
            )
        })
    }

    #[test]
    fn a_failure_while_rendering_the_fallback_reaches_the_next_boundary_out() {
        let (scope, _dirty) = ComponentScope::new();

        // Render 1: both boundaries healthy — inner's children runs,
        // reporting nothing (the inner error_boundary's own fallback,
        // which does the reporting, isn't reached yet).
        nested_boundaries(&scope);

        // Render 2: still nothing reported to *outer* yet, so outer
        // still renders its own children — which is where inner now
        // finds nothing new either... this render's real job is just to
        // get inner from "no error" to "an error was reported to it" in
        // render 3's setup, so drive a report into the inner boundary
        // directly first.
        scope.render(|| {
            error_boundary::<DemoError, _>(
                || {
                    let inner = use_error_boundary::<DemoError>();
                    let reporter = inner.reporter();
                    reporter.report(DemoError("inner failed"));
                    DemoError("render 2 children return value, unused")
                },
                |_, _| DemoError("outer fallback unreached"),
            )
        });

        // Render 3: inner now sees its own reported error and renders
        // its fallback, which reports to whatever boundary is next out
        // — found via the same nested_boundaries shape as render 1, this
        // time with inner actually failed.
        nested_boundaries(&scope);

        // Render 4: outer now sees the error inner's fallback reported
        // to it in render 3.
        let outer_error = nested_boundaries(&scope);
        assert_eq!(outer_error, DemoError("fallback also failed"));
    }

    #[test]
    fn sibling_state_outside_the_boundary_is_unaffected_by_its_failure() {
        let (scope, _dirty) = ComponentScope::new();
        let sibling_signal = scope.render(|| {
            let sibling = use_signal(|| "sibling-untouched");
            error_boundary::<DemoError, _>(
                || {
                    let reporter = use_context::<ErrorReporter<DemoError>>().unwrap();
                    reporter.report(DemoError("boom"));
                },
                |_, _| (),
            );
            sibling
        });

        scope.render(|| {
            let sibling = use_signal(|| "sibling-untouched");
            error_boundary::<DemoError, _>(|| (), |_, _| ());
            assert_eq!(sibling.get(), "sibling-untouched");
        });

        assert_eq!(sibling_signal.get(), "sibling-untouched");
    }
}

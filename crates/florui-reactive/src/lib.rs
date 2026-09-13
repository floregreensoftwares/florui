//! Persistent local component state: [`use_signal`], [`use_memo`], and the
//! [`Scope`] that gives them somewhere stable to live across repeated
//! renders of the same tree.
//!
//! # Scope
//!
//! Not built yet: `use_effect`, `use_context`, `use_ref`, per-component
//! identity (needed for keyed/conditional mounting), disposal, and
//! automatic dependency tracking for `use_memo` (deps are compared by
//! equality, not inferred). One `Scope` has one flat, call-ordered slot
//! list, so every hook call in the tree it renders must run in the same
//! order and count every time. No event wiring in `view!` yet either —
//! [`Scope`] just exposes a dirty flag for a host to poll.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

thread_local! {
    static ACTIVE_SCOPES: RefCell<Vec<Rc<ScopeInner>>> = const { RefCell::new(Vec::new()) };
}

struct ScopeInner {
    slots: RefCell<Vec<Box<dyn Any>>>,
    cursor: Cell<usize>,
    dirty: Rc<Cell<bool>>,
}

/// Where a tree's hook state lives across repeated re-renders — replaying
/// `use_signal`/`use_memo` calls against the same slots, in the same
/// order, is what lets a plain Rust function call keep state instead of
/// starting fresh every time.
pub struct Scope {
    inner: Rc<ScopeInner>,
}

impl Scope {
    /// A fresh scope, plus the dirty flag a host polls to know when a
    /// [`Signal::set`] inside it means "render again."
    pub fn new() -> (Self, Rc<Cell<bool>>) {
        let dirty = Rc::new(Cell::new(false));
        let scope = Self {
            inner: Rc::new(ScopeInner {
                slots: RefCell::new(Vec::new()),
                cursor: Cell::new(0),
                dirty: dirty.clone(),
            }),
        };
        (scope, dirty)
    }

    /// Runs `render` with this scope active, so hook calls inside see the
    /// slots this scope left behind last time.
    pub fn render<T>(&self, render: impl FnOnce() -> T) -> T {
        self.inner.cursor.set(0);
        ACTIVE_SCOPES.with(|scopes| scopes.borrow_mut().push(self.inner.clone()));
        let result = render();
        let popped = ACTIVE_SCOPES.with(|scopes| scopes.borrow_mut().pop());
        debug_assert!(
            popped.is_some_and(|popped| Rc::ptr_eq(&popped, &self.inner)),
            "Scope::render must pop the exact scope it pushed"
        );
        result
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new().0
    }
}

/// Persistent local state for one call-order position. Cloning is cheap
/// and shares the same cell — every clone reads and writes the same value.
pub struct Signal<T> {
    value: Rc<RefCell<T>>,
    dirty: Rc<Cell<bool>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self {
            value: Rc::clone(&self.value),
            dirty: Rc::clone(&self.dirty),
        }
    }
}

impl<T: Clone> Signal<T> {
    /// Reads the current value.
    pub fn get(&self) -> T {
        self.value.borrow().clone()
    }
}

impl<T> Signal<T> {
    /// Writes a new value and marks the owning [`Scope`] dirty.
    pub fn set(&self, value: T) {
        *self.value.borrow_mut() = value;
        self.dirty.set(true);
    }
}

/// Reserves the next call-order slot in the currently active [`Scope`] —
/// shared by every hook. `hook_name` names the caller in the panic message.
///
/// # Panics
///
/// Panics if called outside a [`Scope::render`] pass.
fn active_slot(hook_name: &str) -> (Rc<ScopeInner>, usize) {
    ACTIVE_SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        let scope = scopes.last().unwrap_or_else(|| {
            panic!(
                "{hook_name} called outside of Scope::render — hooks must run \
                 during a component tree's render pass"
            )
        });
        let index = scope.cursor.get();
        scope.cursor.set(index + 1);
        (Rc::clone(scope), index)
    })
}

/// Persistent local state: `init` runs once, the first render this call
/// appears in; every later render returns that same [`Signal`] untouched.
///
/// # Panics
///
/// Panics outside a [`Scope::render`] pass, or if hooks ran in a different
/// order or count than last render (see the crate-level scope note).
pub fn use_signal<T: 'static>(init: impl FnOnce() -> T) -> Signal<T> {
    let (scope, index) = active_slot("use_signal");
    let mut slots = scope.slots.borrow_mut();
    if index == slots.len() {
        let signal = Signal {
            value: Rc::new(RefCell::new(init())),
            dirty: Rc::clone(&scope.dirty),
        };
        slots.push(Box::new(signal.clone()));
        signal
    } else {
        slots[index]
            .downcast_ref::<Signal<T>>()
            .unwrap_or_else(|| {
                panic!(
                    "hook order changed between renders at call position {index} — \
                     hooks must run unconditionally, in the same order, every render"
                )
            })
            .clone()
    }
}

/// A cached derived value: `compute(&deps)` only re-runs when `deps`
/// compares unequal to last render's. `deps` must capture everything
/// `compute` actually depends on — nothing here tracks that for you.
///
/// # Panics
///
/// Panics outside a [`Scope::render`] pass, or if hooks ran in a different
/// order or count than last render (see the crate-level scope note).
pub fn use_memo<D, T>(deps: D, compute: impl FnOnce(&D) -> T) -> T
where
    D: PartialEq + 'static,
    T: Clone + 'static,
{
    let (scope, index) = active_slot("use_memo");
    let mut slots = scope.slots.borrow_mut();
    if index == slots.len() {
        let value = compute(&deps);
        slots.push(Box::new((deps, value.clone())));
        value
    } else {
        let (stored_deps, stored_value) =
            slots[index].downcast_mut::<(D, T)>().unwrap_or_else(|| {
                panic!(
                    "hook order changed between renders at call position {index} — \
                     hooks must run unconditionally, in the same order, every render"
                )
            });
        if *stored_deps != deps {
            *stored_value = compute(&deps);
            *stored_deps = deps;
        }
        stored_value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_reads_back_the_value_it_was_initialized_with() {
        let (scope, _dirty) = Scope::new();
        let value = scope.render(|| use_signal(|| 42).get());
        assert_eq!(value, 42);
    }

    #[test]
    fn a_signal_persists_its_value_across_renders_of_the_same_scope() {
        let (scope, _dirty) = Scope::new();
        scope.render(|| use_signal(|| 0).set(7));
        let value = scope.render(|| use_signal(|| 0).get());
        assert_eq!(
            value, 7,
            "the second render's initializer (0) must not overwrite the first render's set(7)"
        );
    }

    #[test]
    fn independent_scopes_never_share_state() {
        let (a, _) = Scope::new();
        let (b, _) = Scope::new();
        a.render(|| use_signal(|| 0).set(1));
        let b_value = b.render(|| use_signal(|| 0).get());
        assert_eq!(b_value, 0, "scope b's signal is its own, not scope a's");
    }

    #[test]
    fn two_use_signal_calls_in_one_render_get_independent_slots() {
        let (scope, _dirty) = Scope::new();
        let (first, second) = scope.render(|| (use_signal(|| "a").get(), use_signal(|| "b").get()));
        assert_eq!(first, "a");
        assert_eq!(second, "b");
    }

    #[test]
    fn setting_a_signal_marks_its_scope_dirty() {
        let (scope, dirty) = Scope::new();
        assert!(!dirty.get());
        scope.render(|| use_signal(|| 0).set(1));
        assert!(dirty.get());
    }

    #[test]
    fn cloning_a_signal_still_reads_and_writes_the_same_cell() {
        let (scope, _dirty) = Scope::new();
        let clone = scope.render(|| {
            let signal = use_signal(|| 0);
            let clone = signal.clone();
            signal.set(9);
            clone
        });
        assert_eq!(clone.get(), 9);
    }

    #[test]
    fn nested_scope_renders_restore_the_outer_scope_afterward() {
        let (outer, _) = Scope::new();
        let (inner, _) = Scope::new();
        outer.render(|| {
            use_signal(|| "outer-before").get();
            inner.render(|| {
                use_signal(|| "inner").get();
            });
            // Must land on the outer scope's second slot, unaffected by inner's render.
            let value = use_signal(|| "outer-after").get();
            assert_eq!(value, "outer-after");
        });
    }

    #[test]
    #[should_panic(expected = "use_signal called outside of Scope::render")]
    fn use_signal_outside_a_render_panics() {
        use_signal(|| 0);
    }

    #[test]
    #[should_panic(expected = "hook order changed between renders")]
    fn a_different_type_at_the_same_call_position_panics() {
        let (scope, _dirty) = Scope::new();
        scope.render(|| {
            use_signal(|| 0_i32);
        });
        scope.render(|| {
            use_signal(|| "not an i32");
        });
    }

    #[test]
    fn a_memo_computes_from_its_deps() {
        let (scope, _dirty) = Scope::new();
        let value = scope.render(|| use_memo(3, |n| n * 2));
        assert_eq!(value, 6);
    }

    #[test]
    fn a_memo_does_not_recompute_when_deps_are_unchanged() {
        let (scope, _dirty) = Scope::new();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));

        let render = || {
            let calls = calls.clone();
            scope.render(move || {
                use_memo(5, move |n| {
                    calls.set(calls.get() + 1);
                    n * 2
                })
            })
        };

        assert_eq!(render(), 10);
        assert_eq!(render(), 10);
        assert_eq!(calls.get(), 1, "compute must run once for the same deps");
    }

    #[test]
    fn a_memo_recomputes_when_deps_change() {
        let (scope, _dirty) = Scope::new();
        assert_eq!(scope.render(|| use_memo(2, |n| n * 10)), 20);
        assert_eq!(scope.render(|| use_memo(3, |n| n * 10)), 30);
    }

    #[test]
    #[should_panic(expected = "hook order changed between renders")]
    fn a_memo_with_a_different_deps_type_at_the_same_position_panics() {
        let (scope, _dirty) = Scope::new();
        scope.render(|| {
            use_memo(1_i32, |n| n.to_string());
        });
        scope.render(|| {
            use_memo("not an i32", |s| s.to_string());
        });
    }

    #[test]
    #[should_panic(expected = "use_memo called outside of Scope::render")]
    fn use_memo_outside_a_render_panics() {
        use_memo(1, |n| *n);
    }
}

//! [`Signal`]: persistent local state via [`use_signal`].
//!
//! Read-after-write is always synchronous: [`Signal::get`] returns
//! whatever the most recent [`Signal::set`] stored, immediately — this
//! holds whether or not that `set` happened inside [`crate::batch`].
//! `batch` only defers *notifying a host that something changed*
//! ([`DirtyFlag`]'s wake); the write itself, and every read after it, are
//! never deferred or reordered.

use std::cell::RefCell;
use std::rc::Rc;

use crate::DirtyFlag;
use crate::scope::active_slot;

/// Persistent local state for one call-order position. Cloning is cheap
/// and shares the same cell — every clone reads and writes the same value.
pub struct Signal<T> {
    value: Rc<RefCell<T>>,
    dirty: DirtyFlag,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self {
            value: Rc::clone(&self.value),
            dirty: self.dirty.clone(),
        }
    }
}

impl<T: Clone> Signal<T> {
    /// Reads the current value — always the most recent [`Self::set`],
    /// synchronously; see the module docs on why a [`crate::batch`] in
    /// progress does not change this.
    pub fn get(&self) -> T {
        self.value.borrow().clone()
    }
}

impl<T> Signal<T> {
    /// Writes a new value immediately, then marks the owning
    /// [`Scope`](crate::Scope) dirty — deferred to wake its host only
    /// once, alongside every other write in the same [`crate::batch`], if
    /// one is in progress; otherwise immediately, same as always. Also
    /// records an [`crate::trace::UpdateTrace`] attributed to whichever
    /// component is currently active — see [`crate::trace::with_component`].
    pub fn set(&self, value: T) {
        *self.value.borrow_mut() = value;
        self.dirty.mark();
        crate::trace::record();
    }
}

/// Persistent local state: `init` runs once, the first render this call
/// appears in; every later render returns that same [`Signal`] untouched.
///
/// # Panics
///
/// Panics outside a [`Scope::render`](crate::Scope::render) pass, or if
/// hooks ran in a different order or count than last render.
pub fn use_signal<T: 'static>(init: impl FnOnce() -> T) -> Signal<T> {
    let (scope, index) = active_slot("use_signal");
    let mut slots = scope.slots.borrow_mut();
    if index == slots.len() {
        let signal = Signal {
            value: Rc::new(RefCell::new(init())),
            dirty: scope.dirty.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scope;

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
}

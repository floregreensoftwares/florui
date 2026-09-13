//! [`Binding`]: a typed, read/write value exchange for editable component
//! values, kept separate from ordinary read-only props — see
//! slots-and-bindings.md's "Explicit bindings." Receiving a plain `T`
//! never grants mutation rights; only a `Binding<T>` does, and even then
//! the owner decides whether to actually accept a requested update.
//!
//! Not addressed here (tracked gaps, not oversights): a real input
//! control's own editing buffer (distinct from its committed value),
//! commit timing, and IME composition all need an actual text-input
//! widget, which doesn't exist in this engine yet. This module is the
//! primitive that widget will read and write through, proven here with
//! plain values rather than a real control.

use std::rc::Rc;

/// A read/write exchange for one value: [`Self::get`] reads the owner's
/// current value, [`Self::request_update`] asks the owner to change it.
/// The owner remains the source of truth and may reject a request outright
/// (a value that fails validation, for example) — a rejection does not
/// change anything a `Binding` can read; the next binding derived from the
/// owner still reflects whatever it actually accepted, so a rejected edit
/// reconciles automatically on the next read instead of needing a special
/// case.
pub struct Binding<T> {
    value: T,
    on_request: Rc<dyn Fn(T)>,
}

impl<T: Clone> Binding<T> {
    /// Builds a binding from `value` (the owner's current value as of now)
    /// and `on_request` (what to do with a requested new value —
    /// typically validate, then write to the owner's own storage only if
    /// accepted). Applications needing validation or controlled acceptance
    /// build a `Binding` this way directly; [`crate::Signal::binding`] is
    /// the convenience adapter for the common "always accept" case.
    pub fn new(value: T, on_request: impl Fn(T) + 'static) -> Self {
        Self {
            value,
            on_request: Rc::new(on_request),
        }
    }

    /// The owner's value as of when this binding was created — not a live
    /// subscription; a component re-derives its binding(s) every render
    /// the same way it reads any other reactive value.
    pub fn get(&self) -> T {
        self.value.clone()
    }

    /// Asks the owner to change its value to `value`. Does not itself
    /// change anything this binding can read — see [`Self::get`]'s doc: a
    /// rejected request leaves the owner's value, and so every binding
    /// read afterward, unchanged.
    pub fn request_update(&self, value: T) {
        (self.on_request)(value);
    }
}

impl<T: Clone> Clone for Binding<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            on_request: Rc::clone(&self.on_request),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_reads_back_the_value_it_was_built_with() {
        let binding = Binding::new(7, |_| {});
        assert_eq!(binding.get(), 7);
    }

    #[test]
    fn request_update_calls_the_owners_callback_with_the_requested_value() {
        let requested = Rc::new(std::cell::RefCell::new(None));
        let requested_in_binding = Rc::clone(&requested);
        let binding = Binding::new(1, move |value| {
            *requested_in_binding.borrow_mut() = Some(value)
        });
        binding.request_update(2);
        assert_eq!(*requested.borrow(), Some(2));
    }

    #[test]
    fn request_update_does_not_change_what_this_binding_itself_reads() {
        let binding = Binding::new(1, |_| {});
        binding.request_update(2);
        assert_eq!(
            binding.get(),
            1,
            "a Binding is a snapshot — accepting a request is the owner's job, observed by \
             deriving a fresh Binding afterward, not by this one changing in place"
        );
    }

    #[test]
    fn a_rejecting_owner_leaves_the_next_derived_binding_unchanged() {
        let accepted = Rc::new(std::cell::Cell::new(10));
        let make_binding = || {
            let accepted = Rc::clone(&accepted);
            Binding::new(accepted.get(), move |value: i32| {
                // Only accept even numbers — an odd request is rejected.
                if value % 2 == 0 {
                    accepted.set(value);
                }
            })
        };

        make_binding().request_update(11); // rejected
        assert_eq!(
            make_binding().get(),
            10,
            "a rejected update must reconcile back to the last accepted value, not the rejected one"
        );

        make_binding().request_update(12); // accepted
        assert_eq!(make_binding().get(), 12);
    }
}

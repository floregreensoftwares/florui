//! A stored callback for a declarative event-handler attribute
//! (`onclick={move || ...}`) on an [`crate::ElementNode`].

use std::fmt;
use std::rc::Rc;

/// A callback captured from an `on*` attribute in `view!`, carried on the
/// `Element` tree so a host can look it up (by node and event name) after
/// a real input event and call it.
#[derive(Clone)]
pub struct Handler(Rc<dyn Fn()>);

impl Handler {
    pub fn new(f: impl Fn() + 'static) -> Self {
        Self(Rc::new(f))
    }

    /// Runs the callback.
    pub fn call(&self) {
        (self.0)();
    }
}

impl fmt::Debug for Handler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Handler(..)")
    }
}

impl PartialEq for Handler {
    /// Equal only if they share the same underlying callback — comparing
    /// behavior isn't possible, so this is identity, not content, equality.
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Handler {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn call_runs_the_stored_closure() {
        let called = Rc::new(Cell::new(false));
        let called_in_closure = called.clone();
        let handler = Handler::new(move || called_in_closure.set(true));
        handler.call();
        assert!(called.get());
    }

    #[test]
    fn clones_share_the_same_callback() {
        let calls = Rc::new(Cell::new(0));
        let calls_in_closure = calls.clone();
        let handler = Handler::new(move || calls_in_closure.set(calls_in_closure.get() + 1));
        let clone = handler.clone();
        handler.call();
        clone.call();
        assert_eq!(calls.get(), 2);
        assert_eq!(handler, clone);
    }

    #[test]
    fn independently_constructed_handlers_are_not_equal() {
        let a = Handler::new(|| ());
        let b = Handler::new(|| ());
        assert_ne!(a, b);
    }
}

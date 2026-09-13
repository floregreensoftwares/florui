//! [`Scope`]: where a tree's hook state lives across repeated re-renders.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

thread_local! {
    pub(crate) static ACTIVE_SCOPES: RefCell<Vec<Rc<ScopeInner>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct ScopeInner {
    pub(crate) slots: RefCell<Vec<Box<dyn Any>>>,
    pub(crate) cursor: Cell<usize>,
    pub(crate) dirty: Rc<Cell<bool>>,
    pub(crate) context: RefCell<HashMap<TypeId, Box<dyn Any>>>,
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
    /// [`Signal::set`](crate::Signal::set) inside it means "render again."
    pub fn new() -> (Self, Rc<Cell<bool>>) {
        let dirty = Rc::new(Cell::new(false));
        let scope = Self {
            inner: Rc::new(ScopeInner {
                slots: RefCell::new(Vec::new()),
                cursor: Cell::new(0),
                dirty: dirty.clone(),
                context: RefCell::new(HashMap::new()),
            }),
        };
        (scope, dirty)
    }

    /// Runs `render` with this scope active, so hook calls inside see the
    /// slots this scope left behind last time. Provided context is
    /// cleared first — a render that wants it visible must provide it
    /// again, the same way it re-runs every other line of the component.
    pub fn render<T>(&self, render: impl FnOnce() -> T) -> T {
        self.inner.cursor.set(0);
        self.inner.context.borrow_mut().clear();
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

/// Reserves the next call-order slot in the currently active [`Scope`] —
/// shared by every order-sensitive hook. `hook_name` names the caller in
/// the panic message.
///
/// # Panics
///
/// Panics if called outside a [`Scope::render`] pass.
pub(crate) fn active_slot(hook_name: &str) -> (Rc<ScopeInner>, usize) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_signal;

    #[test]
    fn independent_scopes_never_share_state() {
        let (a, _) = Scope::new();
        let (b, _) = Scope::new();
        a.render(|| use_signal(|| 0).set(1));
        let b_value = b.render(|| use_signal(|| 0).get());
        assert_eq!(b_value, 0, "scope b's signal is its own, not scope a's");
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
}

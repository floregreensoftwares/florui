//! [`Scope`]: where a tree's hook state lives across repeated re-renders.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::DirtyFlag;

thread_local! {
    pub(crate) static ACTIVE_SCOPES: RefCell<Vec<Rc<ScopeInner>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct ScopeInner {
    pub(crate) slots: RefCell<Vec<Box<dyn Any>>>,
    pub(crate) cursor: Cell<usize>,
    pub(crate) dirty: DirtyFlag,
    pub(crate) context: RefCell<HashMap<TypeId, Box<dyn Any>>>,
    pub(crate) pending_effects: RefCell<Vec<PendingEffect>>,
}

impl Drop for ScopeInner {
    /// Removing an identity disposes its hooks: every effect this scope
    /// (and, as the field drop cascades into any stored child `Scope`,
    /// every scope nested inside it) still owns runs its cleanup here.
    fn drop(&mut self) {
        crate::effect::dispose(self);
    }
}

/// An effect queued during a render, to run once that render commits.
pub(crate) struct PendingEffect {
    pub(crate) index: usize,
    pub(crate) run: Box<dyn FnOnce() -> Option<crate::effect::Cleanup>>,
}

/// Where a tree's hook state lives across repeated re-renders — replaying
/// `use_signal`/`use_memo` calls against the same slots, in the same
/// order, is what lets a plain Rust function call keep state instead of
/// starting fresh every time.
pub struct Scope {
    inner: Rc<ScopeInner>,
}

impl Scope {
    /// A fresh scope, plus the [`DirtyFlag`] a host uses to know when a
    /// [`Signal::set`](crate::Signal::set) inside it (or inside any
    /// [`use_child_scope`] nested within it) means "render again" —
    /// either by polling [`DirtyFlag::get`] or, better, registering a
    /// [`DirtyFlag::on_mark`] callback to hear about it immediately.
    pub fn new() -> (Self, DirtyFlag) {
        let dirty = DirtyFlag::new();
        (Self::with_dirty_flag(dirty.clone()), dirty)
    }

    /// A fresh scope sharing an existing dirty flag, so a write anywhere
    /// under it is still visible to whoever holds that flag.
    fn with_dirty_flag(dirty: DirtyFlag) -> Self {
        Self {
            inner: Rc::new(ScopeInner {
                slots: RefCell::new(Vec::new()),
                cursor: Cell::new(0),
                dirty,
                context: RefCell::new(HashMap::new()),
                pending_effects: RefCell::new(Vec::new()),
            }),
        }
    }

    /// Runs `render` with this scope active, so hook calls inside see the
    /// slots this scope left behind last time. Provided context is
    /// cleared first — a render that wants it visible must provide it
    /// again, the same way it re-runs every other line of the component.
    /// Once `render` returns, any [`use_effect`](crate::use_effect) queued
    /// during it runs — after commit, same as a real hook model requires.
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
        crate::effect::run_pending(&self.inner);
        result
    }
}

impl Clone for Scope {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new().0
    }
}

/// Renders `render` once, in a fresh, throwaway [`Scope`] — every
/// `#[component]` call needs one active somewhere up the call stack, and
/// this is the convenient choice for a one-shot render (a test, a static
/// capture) that never needs its hook state to persist afterward. A host
/// that renders repeatedly should keep its own [`Scope`] and call
/// [`Scope::render`] directly instead, so state actually survives between
/// renders.
pub fn render_once<T>(render: impl FnOnce() -> T) -> T {
    Scope::new().0.render(render)
}

/// Gives each call site of this function its own persistent [`Scope`],
/// nested inside whichever scope is currently active — this is the
/// mechanism `#[component]` needs so every component gets its own hook
/// state, not one shared list for the whole tree; it is not meant to be
/// called directly from ordinary component code. The child shares its
/// parent's dirty flag, so a write anywhere under it still reaches
/// whoever is watching the root.
///
/// # Panics
///
/// Panics if called outside a [`Scope::render`] pass, or if this call's
/// position held something other than a child scope last render.
pub fn use_child_scope<T>(render: impl FnOnce() -> T) -> T {
    let (parent, index) = active_slot("use_child_scope");
    let mut slots = parent.slots.borrow_mut();
    if index == slots.len() {
        slots.push(Box::new(Scope::with_dirty_flag(parent.dirty.clone())));
    }
    let child = slots[index]
        .downcast_ref::<Scope>()
        .unwrap_or_else(|| {
            panic!(
                "hook order changed between renders at call position {index} — \
                 hooks must run unconditionally, in the same order, every render"
            )
        })
        .clone();
    drop(slots);
    child.render(render)
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

    #[test]
    fn a_child_scope_persists_its_own_state_across_renders_of_the_parent() {
        let (root, _dirty) = Scope::new();
        root.render(|| {
            use_child_scope(|| use_signal(|| 0).set(5));
        });
        let value = root.render(|| use_child_scope(|| use_signal(|| 0).get()));
        assert_eq!(
            value, 5,
            "the child scope, and its signal, must be the same instance across parent renders"
        );
    }

    #[test]
    fn two_child_scopes_at_different_positions_are_independent() {
        let (root, _dirty) = Scope::new();
        let (a, b) = root.render(|| {
            let a = use_child_scope(|| use_signal(|| "a").get());
            let b = use_child_scope(|| use_signal(|| "b").get());
            (a, b)
        });
        assert_eq!(a, "a");
        assert_eq!(b, "b");
    }

    #[test]
    fn setting_a_signal_in_a_child_scope_marks_the_shared_dirty_flag() {
        let (root, dirty) = Scope::new();
        root.render(|| {
            use_child_scope(|| use_signal(|| 0).set(1));
        });
        assert!(
            dirty.get(),
            "a host watching only the root's flag must still see a write deep in a child scope"
        );
    }

    #[test]
    fn a_waker_registered_on_the_root_flag_fires_from_a_signal_set_in_a_child_scope() {
        let (root, dirty) = Scope::new();
        let woken = Rc::new(std::cell::Cell::new(false));
        let woken_in_waker = Rc::clone(&woken);
        dirty.on_mark(move || woken_in_waker.set(true));

        root.render(|| {
            use_child_scope(|| use_signal(|| 0).set(1));
        });

        assert!(
            woken.get(),
            "a host should learn about the write immediately, not just by polling later"
        );
    }

    #[test]
    fn a_grandchild_scope_also_shares_the_root_dirty_flag() {
        let (root, dirty) = Scope::new();
        root.render(|| {
            use_child_scope(|| {
                use_child_scope(|| use_signal(|| 0).set(1));
            });
        });
        assert!(dirty.get());
    }

    #[test]
    #[should_panic(expected = "use_child_scope called outside of Scope::render")]
    fn use_child_scope_outside_a_render_panics() {
        use_child_scope(|| ());
    }

    #[test]
    #[should_panic(expected = "hook order changed between renders")]
    fn a_non_child_scope_hook_at_the_same_position_panics() {
        let (root, _dirty) = Scope::new();
        root.render(|| {
            use_child_scope(|| ());
        });
        root.render(|| {
            use_signal(|| 0);
        });
    }
}

//! A minimal mount/rerender driver: enough to make hooks real and testable
//! without a reconciler.
//!
//! This mounts exactly one root component and lets a caller explicitly
//! trigger a rerender. It is **not** wired into `view!`: calling a
//! `#[component]` function nested inside another component's `view!` body
//! is still a plain, uninstrumented function call with no persistent
//! instance, so nested components get no hook state today. Making that
//! automatic needs a real reconciler matching a tree across renders by
//! position and key, which does not exist yet — this driver only proves
//! the hook mechanism itself (persistent storage, call-order validation,
//! effect scheduling and cleanup) with a single instance.

use std::cell::RefCell;

use super::instance::{Instance, PendingEffect, SharedInstance, Slot};
use crate::element::Element;

thread_local! {
    static CURRENT: RefCell<Option<SharedInstance>> = const { RefCell::new(None) };
}

/// Panics when called outside `render_once`, i.e. outside a mounted
/// component's own render — hooks have nothing to attach to otherwise.
pub(super) fn with_current_instance<R>(f: impl FnOnce(&SharedInstance) -> R) -> R {
    CURRENT.with(|current| {
        let borrowed = current.borrow();
        let instance = borrowed.as_ref().unwrap_or_else(|| {
            panic!(
                "a hook (use_signal/use_effect/use_ref) was called outside of a mounted \
                 component render; call it only from within the function passed to \
                 `florui::reactive::mount`"
            )
        });
        f(instance)
    })
}

/// A mounted component instance: its hook state and its last rendered
/// element.
pub struct Mounted {
    component: Box<dyn Fn() -> Element>,
    instance: SharedInstance,
    element: Element,
}

/// Mounts `component`, running it once and any effects it queued.
pub fn mount(component: impl Fn() -> Element + 'static) -> Mounted {
    let instance = Instance::new();
    let component: Box<dyn Fn() -> Element> = Box::new(component);
    let element = render_once(&instance, &component);
    Mounted {
        component,
        instance,
        element,
    }
}

impl Mounted {
    pub fn element(&self) -> &Element {
        &self.element
    }

    /// Re-runs the component against its existing hook state and runs any
    /// effects whose dependencies changed.
    pub fn rerender(&mut self) -> &Element {
        self.element = render_once(&self.instance, &self.component);
        &self.element
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        // "Removing an identity disposes its hooks": run every remaining
        // effect cleanup.
        self.instance.borrow_mut().dispose();
    }
}

fn render_once(instance: &SharedInstance, component: &dyn Fn() -> Element) -> Element {
    instance.borrow_mut().begin_render();
    CURRENT.with(|current| *current.borrow_mut() = Some(instance.clone()));

    let element = component();

    CURRENT.with(|current| *current.borrow_mut() = None);
    instance.borrow_mut().end_render();

    run_pending_effects(instance);
    element
}

fn run_pending_effects(instance: &SharedInstance) {
    let pending: Vec<PendingEffect> = std::mem::take(&mut instance.borrow_mut().pending_effects);

    for pending in pending {
        let old_cleanup = {
            let mut instance = instance.borrow_mut();
            let Slot::Effect(effect) = instance.slot_mut(pending.index) else {
                unreachable!("effect slots are only ever populated as Slot::Effect");
            };
            effect.cleanup.take()
        };
        if let Some(cleanup) = old_cleanup {
            cleanup();
        }

        let new_cleanup = (pending.run)();

        let mut instance = instance.borrow_mut();
        let Slot::Effect(effect) = instance.slot_mut(pending.index) else {
            unreachable!("effect slots are only ever populated as Slot::Effect");
        };
        effect.deps = Some(pending.new_deps);
        effect.cleanup = new_cleanup;
    }
}

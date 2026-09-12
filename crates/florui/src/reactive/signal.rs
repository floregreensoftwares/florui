//! `use_signal`: persistent local state that survives rerenders of the same
//! mounted instance.

use std::any::Any;
use std::marker::PhantomData;

use super::instance::{SharedInstance, Slot, SlotKind};
use super::mount::with_current_instance;

/// A handle to state owned by the mounted instance, not by this value.
/// Cloning a `Signal` gives another handle to the same slot, not a copy of
/// the value — call [`Signal::get`] for that.
pub struct Signal<T: 'static> {
    instance: SharedInstance,
    index: usize,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal {
            instance: self.instance.clone(),
            index: self.index,
            _marker: PhantomData,
        }
    }
}

impl<T: Clone + 'static> Signal<T> {
    pub fn get(&self) -> T {
        let instance = self.instance.borrow();
        let Slot::Signal(value) = instance.slot(self.index) else {
            unreachable!("a Signal always points at a Slot::Signal");
        };
        value
            .downcast_ref::<T>()
            .expect("Signal<T> slot type matches T by construction")
            .clone()
    }
}

impl<T: 'static> Signal<T> {
    /// Replaces the stored value. There is no scheduler yet, so this does
    /// not itself trigger a rerender — call `Mounted::rerender` to observe
    /// the new value. A future scheduler can mark the instance dirty here
    /// without changing this method's signature.
    pub fn set(&self, value: T) {
        let mut instance = self.instance.borrow_mut();
        *instance.slot_mut(self.index) = Slot::Signal(Box::new(value));
    }

    pub fn update(&self, f: impl FnOnce(&mut T)) {
        let mut instance = self.instance.borrow_mut();
        let Slot::Signal(current) = instance.slot_mut(self.index) else {
            unreachable!("a Signal always points at a Slot::Signal");
        };
        let current = current
            .downcast_mut::<T>()
            .expect("Signal<T> slot type matches T by construction");
        f(current);
    }
}

/// Persistent local reactive state. `init` runs once, the first time this
/// call site is reached for a given mounted instance; later renders reuse
/// the stored value instead of recomputing it.
pub fn use_signal<T: 'static>(init: impl FnOnce() -> T) -> Signal<T> {
    with_current_instance(|instance_rc| {
        let mut init = Some(init);
        let index = instance_rc.borrow_mut().next_index(SlotKind::Signal, || {
            let value: Box<dyn Any> =
                Box::new((init.take().expect("init is called at most once"))());
            Slot::Signal(value)
        });
        Signal {
            instance: instance_rc.clone(),
            index,
            _marker: PhantomData,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::mount;

    #[test]
    fn initializer_runs_once_across_rerenders() {
        use std::cell::Cell;
        use std::rc::Rc;

        let init_calls = Rc::new(Cell::new(0));
        let init_calls_in_component = init_calls.clone();

        let mut mounted = mount(move || {
            let init_calls = init_calls_in_component.clone();
            let _count = use_signal(move || {
                init_calls.set(init_calls.get() + 1);
                0
            });
            crate::Element::text("ignored")
        });

        mounted.rerender();
        mounted.rerender();

        assert_eq!(init_calls.get(), 1);
    }

    #[test]
    fn set_value_is_observed_on_the_next_render() {
        let signal_handle: std::rc::Rc<std::cell::RefCell<Option<Signal<i32>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let handle_in_component = signal_handle.clone();

        let mut mounted = mount(move || {
            let count = use_signal(|| 0);
            *handle_in_component.borrow_mut() = Some(count.clone());
            crate::Element::text(count.get().to_string())
        });
        assert_eq!(mounted.element(), &crate::Element::text("0"));

        signal_handle.borrow().as_ref().unwrap().set(5);
        let element = mounted.rerender();
        assert_eq!(element, &crate::Element::text("5"));
    }
}

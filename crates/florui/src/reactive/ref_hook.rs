//! `use_ref`: stable, non-reactive storage. Unlike a [`super::Signal`],
//! mutating a `RefHandle` never marks anything for rerender — it is for
//! data a component needs to keep between renders without that data's
//! changes being observable output.

use std::any::Any;
use std::marker::PhantomData;

use super::instance::{SharedInstance, Slot, SlotKind};
use super::mount::with_current_instance;

pub struct RefHandle<T: 'static> {
    instance: SharedInstance,
    index: usize,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> RefHandle<T> {
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let instance = self.instance.borrow();
        let Slot::Ref(value) = instance.slot(self.index) else {
            unreachable!("a RefHandle always points at a Slot::Ref");
        };
        f(value
            .downcast_ref::<T>()
            .expect("RefHandle<T> slot type matches T by construction"))
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut instance = self.instance.borrow_mut();
        let Slot::Ref(value) = instance.slot_mut(self.index) else {
            unreachable!("a RefHandle always points at a Slot::Ref");
        };
        f(value
            .downcast_mut::<T>()
            .expect("RefHandle<T> slot type matches T by construction"))
    }
}

/// Stable storage that survives rerenders without ever scheduling one.
/// `init` runs once, like [`super::use_signal`]'s.
pub fn use_ref<T: 'static>(init: impl FnOnce() -> T) -> RefHandle<T> {
    with_current_instance(|instance_rc| {
        let mut init = Some(init);
        let index = instance_rc.borrow_mut().next_index(SlotKind::Ref, || {
            let value: Box<dyn Any> =
                Box::new((init.take().expect("init is called at most once"))());
            Slot::Ref(value)
        });
        RefHandle {
            instance: instance_rc.clone(),
            index,
            _marker: PhantomData,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Element;
    use crate::reactive::mount;

    #[test]
    fn value_persists_across_rerenders_without_reinitializing() {
        let mut mounted = mount(|| {
            let calls = use_ref(|| 0);
            calls.with_mut(|c| *c += 1);
            Element::text(calls.with(|c| c.to_string()))
        });

        assert_eq!(mounted.element(), &Element::text("1"));
        assert_eq!(mounted.rerender(), &Element::text("2"));
        assert_eq!(mounted.rerender(), &Element::text("3"));
    }
}

//! `use_effect`: synchronizing with something outside the element tree
//! after a render commits, re-running only when its dependencies change.

use super::instance::{Cleanup, EffectSlot, PendingEffect, Slot, SlotKind};
use super::mount::with_current_instance;

/// Runs `effect` after this render commits, but only the first time this
/// call site is reached or when `deps` differs from the previous render's.
/// `effect`'s return value, if any, is run as cleanup before the next time
/// it re-runs, and when the mounted instance is dropped.
pub fn use_effect<D>(deps: D, effect: impl FnOnce() -> Option<Cleanup> + 'static)
where
    D: PartialEq + 'static,
{
    with_current_instance(|instance_rc| {
        let index = instance_rc
            .borrow_mut()
            .next_index(SlotKind::Effect, || Slot::Effect(EffectSlot::default()));

        let changed = {
            let instance = instance_rc.borrow();
            let Slot::Effect(slot) = instance.slot(index) else {
                unreachable!("an effect call site always points at a Slot::Effect");
            };
            match &slot.deps {
                None => true,
                Some(previous) => previous.downcast_ref::<D>() != Some(&deps),
            }
        };

        if changed {
            instance_rc
                .borrow_mut()
                .pending_effects
                .push(PendingEffect {
                    index,
                    new_deps: Box::new(deps),
                    run: Box::new(effect),
                });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::Element;
    use crate::reactive::mount;

    #[test]
    fn runs_on_mount_and_skips_unchanged_dependencies() {
        let runs = Rc::new(RefCell::new(0));
        let runs_in_component = runs.clone();

        let mut mounted = mount(move || {
            let runs = runs_in_component.clone();
            use_effect(1, move || {
                *runs.borrow_mut() += 1;
                None
            });
            Element::text("x")
        });

        mounted.rerender();
        mounted.rerender();

        assert_eq!(*runs.borrow(), 1);
    }

    #[test]
    fn reruns_when_dependencies_change() {
        let dep = Rc::new(RefCell::new(1));
        let runs = Rc::new(RefCell::new(0));
        let dep_in_component = dep.clone();
        let runs_in_component = runs.clone();

        let mut mounted = mount(move || {
            let runs = runs_in_component.clone();
            use_effect(*dep_in_component.borrow(), move || {
                *runs.borrow_mut() += 1;
                None
            });
            Element::text("x")
        });
        assert_eq!(*runs.borrow(), 1);

        *dep.borrow_mut() = 2;
        mounted.rerender();
        assert_eq!(*runs.borrow(), 2);
    }

    #[test]
    fn cleanup_runs_before_the_next_effect_and_on_unmount() {
        let events = Rc::new(RefCell::new(Vec::<&'static str>::new()));
        let dep = Rc::new(RefCell::new(1));
        let events_in_component = events.clone();
        let dep_in_component = dep.clone();

        let mounted = mount(move || {
            let events_run = events_in_component.clone();
            let events_cleanup = events_in_component.clone();
            use_effect(*dep_in_component.borrow(), move || {
                events_run.borrow_mut().push("run");
                Some(Box::new(move || {
                    events_cleanup.borrow_mut().push("cleanup")
                }))
            });
            Element::text("x")
        });
        assert_eq!(*events.borrow(), vec!["run"]);

        drop(mounted);
        assert_eq!(*events.borrow(), vec!["run", "cleanup"]);
    }

    #[test]
    fn cleanup_runs_before_the_effect_reruns_on_changed_dependencies() {
        let events = Rc::new(RefCell::new(Vec::<&'static str>::new()));
        let dep = Rc::new(RefCell::new(1));
        let events_in_component = events.clone();
        let dep_in_component = dep.clone();

        let mut mounted = mount(move || {
            let events_run = events_in_component.clone();
            let events_cleanup = events_in_component.clone();
            use_effect(*dep_in_component.borrow(), move || {
                events_run.borrow_mut().push("run");
                Some(Box::new(move || {
                    events_cleanup.borrow_mut().push("cleanup")
                }))
            });
            Element::text("x")
        });
        assert_eq!(*events.borrow(), vec!["run"]);

        *dep.borrow_mut() = 2;
        mounted.rerender();
        assert_eq!(*events.borrow(), vec!["run", "cleanup", "run"]);
    }
}

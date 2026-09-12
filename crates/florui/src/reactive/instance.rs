//! Per-mounted-component hook storage, and the ordering/count validation
//! that keeps hook call order honest across renders.

use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

pub type SharedInstance = Rc<RefCell<Instance>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Signal,
    Effect,
    Ref,
}

impl fmt::Display for SlotKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            SlotKind::Signal => "use_signal",
            SlotKind::Effect => "use_effect",
            SlotKind::Ref => "use_ref",
        };
        write!(f, "{name}")
    }
}

pub enum Slot {
    Signal(Box<dyn Any>),
    Effect(EffectSlot),
    Ref(Box<dyn Any>),
}

impl Slot {
    fn kind(&self) -> SlotKind {
        match self {
            Slot::Signal(_) => SlotKind::Signal,
            Slot::Effect(_) => SlotKind::Effect,
            Slot::Ref(_) => SlotKind::Ref,
        }
    }
}

/// An effect's cleanup: run before it re-runs, and on unmount.
pub type Cleanup = Box<dyn FnOnce()>;

#[derive(Default)]
pub struct EffectSlot {
    pub deps: Option<Box<dyn Any>>,
    pub cleanup: Option<Cleanup>,
}

pub struct PendingEffect {
    pub index: usize,
    pub new_deps: Box<dyn Any>,
    pub run: Box<dyn FnOnce() -> Option<Cleanup>>,
}

/// Storage for one mounted component: its hook slots, in call order, plus
/// the effects queued during the render just finished but not yet run.
#[derive(Default)]
pub struct Instance {
    slots: Vec<Slot>,
    cursor: usize,
    mounted: bool,
    pub(super) pending_effects: Vec<PendingEffect>,
}

impl Instance {
    pub fn new() -> SharedInstance {
        Rc::new(RefCell::new(Instance::default()))
    }

    pub fn begin_render(&mut self) {
        self.cursor = 0;
    }

    /// Panics if this render called a different number of hooks than the
    /// previous one, since that means hooks were called conditionally
    /// instead of unconditionally in the same order every render. The very
    /// first render has nothing to compare against, so it only records the
    /// baseline.
    pub fn end_render(&mut self) {
        if self.mounted && self.cursor != self.slots.len() {
            panic!(
                "hook count changed between renders: this render called {} hooks, the previous \
                 one called {} — hooks must run unconditionally and in the same order every \
                 render, never inside loops, branches, or early returns",
                self.cursor,
                self.slots.len()
            );
        }
        self.mounted = true;
    }

    /// Returns the slot index for the next hook call, creating the slot on
    /// first render or validating it still has the expected kind on later
    /// renders.
    pub fn next_index(&mut self, kind: SlotKind, init: impl FnOnce() -> Slot) -> usize {
        let index = self.cursor;
        self.cursor += 1;

        if index == self.slots.len() {
            self.slots.push(init());
        } else {
            let existing = self.slots[index].kind();
            if existing != kind {
                panic!(
                    "hook at position {index} was `{existing}` on a previous render but `{kind}` \
                     now — hooks must be called in the same order every render, unconditionally"
                );
            }
        }

        index
    }

    pub fn slot(&self, index: usize) -> &Slot {
        &self.slots[index]
    }

    pub fn slot_mut(&mut self, index: usize) -> &mut Slot {
        &mut self.slots[index]
    }

    /// Runs every effect's cleanup, in slot order. Called when a mounted
    /// instance is dropped: "removing an identity disposes its hooks."
    pub fn dispose(&mut self) {
        for slot in &mut self.slots {
            if let Slot::Effect(effect) = slot
                && let Some(cleanup) = effect.cleanup.take()
            {
                cleanup();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_render_records_a_baseline_without_checking_it() {
        let mut instance = Instance::default();
        instance.begin_render();
        instance.next_index(SlotKind::Signal, || Slot::Signal(Box::new(0)));
        instance.end_render();
    }

    #[test]
    #[should_panic(expected = "hook count changed between renders")]
    fn fewer_hooks_on_a_later_render_panics() {
        let mut instance = Instance::default();
        instance.begin_render();
        instance.next_index(SlotKind::Signal, || Slot::Signal(Box::new(0)));
        instance.end_render();

        instance.begin_render();
        instance.end_render();
    }

    #[test]
    #[should_panic(expected = "was `use_signal` on a previous render but `use_effect` now")]
    fn hook_kind_mismatch_panics() {
        let mut instance = Instance::default();
        instance.begin_render();
        instance.next_index(SlotKind::Signal, || Slot::Signal(Box::new(0)));
        instance.end_render();

        instance.begin_render();
        instance.next_index(SlotKind::Effect, || Slot::Effect(EffectSlot::default()));
        instance.end_render();
    }
}

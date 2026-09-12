//! Which nodes `:hover`/`:focus`/`:active` currently match.
//!
//! This is deliberately not wired to real pointer/keyboard events: no
//! event system exists yet. A caller (today, a test; eventually a real
//! runtime) decides which [`NodeId`]s are in which state and builds this
//! directly — [`crate::cascade::compute`] only consumes it.

use std::collections::HashSet;

use crate::selector::PseudoClass;
use crate::tree::NodeId;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InteractionState {
    hovered: HashSet<NodeId>,
    focused: HashSet<NodeId>,
    active: HashSet<NodeId>,
}

impl InteractionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_hovered(mut self, id: NodeId) -> Self {
        self.hovered.insert(id);
        self
    }

    pub fn with_focused(mut self, id: NodeId) -> Self {
        self.focused.insert(id);
        self
    }

    pub fn with_active(mut self, id: NodeId) -> Self {
        self.active.insert(id);
        self
    }

    pub fn matches(&self, pseudo: PseudoClass, id: NodeId) -> bool {
        match pseudo {
            PseudoClass::Hover => self.hovered.contains(&id),
            PseudoClass::Focus => self.focused.contains(&id),
            PseudoClass::Active => self.active.contains(&id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_each_state_independently() {
        let state = InteractionState::new()
            .with_hovered(1)
            .with_focused(2)
            .with_active(3);
        assert!(state.matches(PseudoClass::Hover, 1));
        assert!(!state.matches(PseudoClass::Hover, 2));
        assert!(state.matches(PseudoClass::Focus, 2));
        assert!(state.matches(PseudoClass::Active, 3));
        assert!(!state.matches(PseudoClass::Active, 1));
    }
}

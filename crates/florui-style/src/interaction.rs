//! Which nodes `:hover`/`:focus`/`:active` currently match.
//!
//! This is deliberately not wired to real pointer/keyboard events: no
//! event system exists yet. A caller (today, a test; eventually a real
//! runtime) decides which [`NodeId`]s are in which state and builds this
//! directly — [`crate::cascade::compute`] only consumes it.

use std::collections::HashSet;

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

    pub fn is_hovered(&self, id: NodeId) -> bool {
        self.hovered.contains(&id)
    }

    pub fn is_focused(&self, id: NodeId) -> bool {
        self.focused.contains(&id)
    }

    pub fn is_active(&self, id: NodeId) -> bool {
        self.active.contains(&id)
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
        assert!(state.is_hovered(1));
        assert!(!state.is_hovered(2));
        assert!(state.is_focused(2));
        assert!(state.is_active(3));
        assert!(!state.is_active(1));
    }
}

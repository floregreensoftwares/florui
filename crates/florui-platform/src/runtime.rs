//! [`UiRuntime`]: the window-independent half of a running Florui tree.
//!
//! Renders on demand against whatever viewport a host supplies, caches
//! the resulting geometry, and answers hit tests and click dispatch
//! against that cache — a host never has to render again just to know
//! what is under a point. Nothing here knows about `winit`, a window, or
//! any other platform type; a game or mobile host could drive this the
//! same way [`crate::run`] does.

use std::collections::HashMap;

use florui::Element;
use florui_layout::BoxLayout;
use florui_reactive::{DirtyFlag, Scope};
use florui_style::{Arena, ComputedStyle, InteractionState, NodeId, Rule, StyleError};
use taffy::prelude::*;

pub struct UiRuntime {
    scope: Scope,
    dirty: DirtyFlag,
    rules: Vec<Rule>,
    root: Box<dyn Fn() -> Element>,
    interaction: InteractionState,
    hovered: Option<NodeId>,
    arena: Arena,
    styles: HashMap<NodeId, ComputedStyle>,
    layouts: HashMap<NodeId, BoxLayout>,
}

impl UiRuntime {
    /// Parses `css` and renders `root` once against `viewport`, so a
    /// freshly constructed runtime always has real geometry ready.
    pub fn new(
        css: &str,
        root: impl Fn() -> Element + 'static,
        viewport: Size<AvailableSpace>,
    ) -> Result<Self, StyleError> {
        let rules = florui_style::parse_stylesheet(css)?;
        Ok(Self::with_rules(rules, root, viewport))
    }

    /// Same as [`Self::new`], but for a host that already parsed its
    /// stylesheet (typically to fail fast before opening a window) and
    /// doesn't want to parse it again just to build the runtime.
    pub(crate) fn with_rules(
        rules: Vec<Rule>,
        root: impl Fn() -> Element + 'static,
        viewport: Size<AvailableSpace>,
    ) -> Self {
        let (scope, dirty) = Scope::new();
        let mut runtime = Self {
            scope,
            dirty,
            rules,
            root: Box::new(root),
            interaction: InteractionState::new(),
            hovered: None,
            arena: Arena::build(&Element::Fragment(Vec::new())),
            styles: HashMap::new(),
            layouts: HashMap::new(),
        };
        runtime.update(viewport);
        runtime
    }

    /// The flag that marks itself whenever a
    /// [`florui_reactive::Signal::set`] happens anywhere under the root —
    /// register a waker with [`DirtyFlag::on_mark`] to learn about it the
    /// instant it happens rather than polling [`Self::is_dirty`] after
    /// specific events a host already knew to check.
    pub fn dirty_flag(&self) -> DirtyFlag {
        self.dirty.clone()
    }

    /// Re-renders the tree against `viewport` and caches the resulting
    /// geometry for [`Self::geometry`]/[`Self::hit_test`] to answer
    /// without rendering again.
    pub fn update(&mut self, viewport: Size<AvailableSpace>) {
        let tree = self.scope.render(|| (self.root)());
        self.arena = Arena::build(&tree);
        self.styles = florui_style::compute(&self.arena, &self.rules, &self.interaction);
        self.layouts = florui_layout::compute_layout(&self.arena, &self.styles, viewport)
            .expect("this tree's explicit sizes never produce a layout failure");
    }

    /// The geometry computed by the most recent [`Self::update`].
    pub fn geometry(
        &self,
    ) -> (
        &Arena,
        &HashMap<NodeId, ComputedStyle>,
        &HashMap<NodeId, BoxLayout>,
    ) {
        (&self.arena, &self.styles, &self.layouts)
    }

    /// The topmost node under `(x, y)`, against the last computed
    /// geometry — does not render again.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        florui_layout::hit_test(&self.arena, &self.layouts, x, y)
    }

    /// Updates which node is `:hover`ed. Returns whether that actually
    /// changed anything — `:hover` can affect computed style, so a caller
    /// should follow a `true` result with a fresh [`Self::update`].
    pub fn set_hovered(&mut self, node: Option<NodeId>) -> bool {
        if node == self.hovered {
            return false;
        }
        self.hovered = node;
        self.interaction = match node {
            Some(id) => InteractionState::new().with_hovered(id),
            None => InteractionState::new(),
        };
        true
    }

    /// Calls `node`'s `click` handler, if it declared one, against the
    /// last computed geometry.
    pub fn dispatch_click(&self, node: NodeId) {
        if let Some(handler) = self.arena.handler(node, "click") {
            handler.call();
        }
    }

    /// Whether a [`florui_reactive::Signal::set`] happened since the last
    /// [`Self::clear_dirty`].
    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    pub fn clear_dirty(&self) {
        self.dirty.clear();
    }
}

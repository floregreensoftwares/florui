//! [`Portal`]: renders its children in place -- inheriting whatever
//! `use_context` values and hook-scope nesting are active at its own
//! call site, exactly like any other nested `view!` content, since
//! `view!` already evaluates a component's children *before* the
//! component's own body runs (see `crates/florui-macros/src/view/
//! codegen.rs`'s `component_call`) -- but doesn't return that content at
//! its own call site. Instead it pushes the rendered result into this
//! render's [`PortalRegistry`], which [`crate::UiRuntime::update`]
//! collects and splices in as extra, independently laid-out overlay
//! roots (see [`florui_style::Arena::build_with_overlays`] and
//! `florui_layout`'s own overlay layout pass) -- unclipped by any
//! ancestor, painted after (on top of) the document, and winning
//! hit-testing over it, all for free from how a later `Arena` root
//! already behaves.

use std::cell::RefCell;

use florui::{Children, Element, IntoNodes, component};
use florui_reactive::use_context;

/// Collects every [`Portal`]'s own rendered content during one
/// [`crate::UiRuntime::update`] render pass. Cleared and rebuilt fresh
/// every render (see [`Self::take`]) -- a `Portal` not called this
/// render simply never pushes into it, so nothing stale survives; no
/// unmount lifecycle is needed the way [`crate::SizeObserverRegistry`]'s
/// persistent, id-keyed observers need one.
#[derive(Default)]
pub struct PortalRegistry {
    portals: RefCell<Vec<Element>>,
}

impl PortalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, rendered: Element) {
        self.portals.borrow_mut().push(rendered);
    }

    /// Takes every portal collected so far, clearing the registry.
    pub(crate) fn take(&self) -> Vec<Element> {
        self.portals.take()
    }
}

/// Renders `children` as an overlay root instead of at this call site --
/// see the module doc. Requires a [`PortalRegistry`] in context, which
/// [`crate::UiRuntime`] provides every render.
///
/// # Panics
///
/// Panics if no [`PortalRegistry`] is in context — only a
/// `UiRuntime`-hosted render provides one.
#[component]
pub fn Portal(children: Children) -> Element {
    let registry = use_context::<std::rc::Rc<PortalRegistry>>().expect(
        "Portal needs a PortalRegistry in context — only a UiRuntime-hosted render provides one",
    );
    registry.push(Element::Fragment(children.into_nodes()));
    Element::Fragment(Vec::new())
}

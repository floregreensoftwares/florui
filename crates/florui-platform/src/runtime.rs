//! [`UiRuntime`]: the window-independent half of a running Florui tree.
//!
//! Renders on demand against whatever viewport a host supplies, caches
//! the resulting geometry, and answers hit tests and click dispatch
//! against that cache — a host never has to render again just to know
//! what is under a point. Nothing here knows about `winit`, a window, or
//! any other platform type; a game or mobile host could drive this the
//! same way [`crate::run`] does.

use std::collections::HashMap;
use std::rc::Rc;

use florui::Element;
use florui_layout::BoxLayout;
use florui_reactive::executor::{Executor, LocalExecutor};
use florui_reactive::{DirtyFlag, Scope, provide_context};
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
    /// Reachable by [`florui_reactive::use_resource`] via context, provided
    /// fresh every render the same way any other context value is. Advanced
    /// once per [`Self::update`] so a fetch that's already resolvable (or
    /// was woken by prior progress) commits without a host needing to know
    /// that async work is involved at all. A background completion that
    /// isn't woken by anything already driving another [`Self::update`]
    /// (a real timer or I/O reactor, on a host whose event loop otherwise
    /// only wakes for input) needs its own bridge from the executor's
    /// waker to that event loop — not yet wired here.
    executor: Rc<LocalExecutor>,
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
            executor: Rc::new(LocalExecutor::new()),
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
        let executor = Rc::clone(&self.executor);
        let tree = self.scope.render(|| {
            provide_context(Rc::clone(&executor) as Rc<dyn Executor>);
            (self.root)()
        });
        // Lets any resource the render just started (or a prior task's
        // waker already requeued) make progress before this frame commits.
        self.executor.run_until_stalled();
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
    /// last computed geometry — inside [`florui_reactive::batch`], so a
    /// handler that writes more than one `Signal` (or writes the same one
    /// more than once) wakes this runtime's host exactly once for the
    /// whole click, not once per write.
    pub fn dispatch_click(&self, node: NodeId) {
        if let Some(handler) = self.arena.handler(node, "click") {
            florui_reactive::batch(|| handler.call());
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

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use florui::prelude::*;
    use florui_reactive::testing::manual_future;
    use florui_reactive::{Resource, use_resource};

    use super::*;

    fn viewport() -> Size<AvailableSpace> {
        Size {
            width: AvailableSpace::Definite(100.0),
            height: AvailableSpace::Definite(100.0),
        }
    }

    fn status_text(id: NodeId, runtime: &UiRuntime) -> String {
        let (arena, ..) = runtime.geometry();
        arena.text_content(id).to_string()
    }

    fn find_status(runtime: &UiRuntime) -> NodeId {
        let (arena, ..) = runtime.geometry();
        arena
            .find(|arena, id| arena.id_attr(id) == Some("status"))
            .expect("root always renders a #status node")
    }

    /// Proves `use_resource` works through a real [`UiRuntime`], not just
    /// the raw `florui-reactive` hook in isolation: the `Executor` it
    /// needs comes from context [`Self::update`] provides, and its
    /// eventual `Ready` state reaches a real rendered tree.
    #[test]
    fn a_resource_resolves_through_a_real_update_cycle() {
        let (future, resolver) = manual_future::<Result<i32, &'static str>>();
        let future = Rc::new(RefCell::new(Some(future)));

        let root = move || {
            let future = Rc::clone(&future);
            let resource = use_resource("key", move |_| {
                future
                    .borrow_mut()
                    .take()
                    .expect("the fetch only runs once for an unchanged key")
            });
            let text = match resource.get() {
                Resource::Idle => "idle".to_string(),
                Resource::Pending { .. } => "pending".to_string(),
                Resource::Ready(value) => format!("ready:{value}"),
                Resource::Failed { error, .. } => format!("failed:{error}"),
            };
            view! { <div id="status">{text}</div> }
        };

        let mut runtime = UiRuntime::with_rules(Vec::new(), root, viewport());
        // The effect that starts the fetch runs after the first render
        // commits, the same as any other mount effect — its `Signal::set`
        // to `Pending` only shows up once something re-renders afterward.
        assert!(runtime.is_dirty());
        runtime.clear_dirty();
        runtime.update(viewport());
        let status = find_status(&runtime);
        assert_eq!(status_text(status, &runtime), "pending");

        resolver.resolve(Ok(42));
        // This render's own snapshot still reads the pre-completion state:
        // `update` advances the executor (committing `Ready`) only after
        // building this render's tree, the same ordering that makes the
        // initial `Pending` above take one extra render to show up too.
        runtime.update(viewport());
        assert!(runtime.is_dirty());
        runtime.clear_dirty();
        runtime.update(viewport());
        let status = find_status(&runtime);
        assert_eq!(status_text(status, &runtime), "ready:42");
    }

    #[test]
    fn dispatch_click_calls_the_nodes_own_click_handler() {
        let clicked = Rc::new(Cell::new(false));
        let clicked_in_handler = Rc::clone(&clicked);
        let runtime = UiRuntime::with_rules(
            Vec::new(),
            move || {
                let clicked = Rc::clone(&clicked_in_handler);
                view! { <button onclick={move || clicked.set(true)} /> }
            },
            Size::MAX_CONTENT,
        );
        let button = runtime.geometry().0.roots()[0];

        runtime.dispatch_click(button);

        assert!(clicked.get());
    }

    #[test]
    fn dispatch_click_on_a_node_with_no_handler_does_nothing() {
        let runtime = UiRuntime::with_rules(Vec::new(), || view! { <div /> }, Size::MAX_CONTENT);
        let node = runtime.geometry().0.roots()[0];

        // Must not panic — the whole point of the test.
        runtime.dispatch_click(node);
    }

    #[test]
    fn a_handler_writing_two_signals_wakes_the_host_exactly_once() {
        let wakes = Rc::new(Cell::new(0));
        let wakes_in_waker = Rc::clone(&wakes);

        let runtime = UiRuntime::with_rules(
            Vec::new(),
            || {
                let a = use_signal(|| 0);
                let b = use_signal(|| 0);
                view! {
                    <button onclick={move || {
                        a.set(1);
                        b.set(2);
                    }} />
                }
            },
            Size::MAX_CONTENT,
        );
        runtime
            .dirty_flag()
            .on_mark(move || wakes_in_waker.set(wakes_in_waker.get() + 1));
        let button = runtime.geometry().0.roots()[0];

        runtime.dispatch_click(button);

        assert_eq!(
            wakes.get(),
            1,
            "one click writing two signals must wake the host once, not twice"
        );
    }
}

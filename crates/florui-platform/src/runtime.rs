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
use florui_reactive::{ComponentScope, DirtyFlag, provide_context};
use florui_style::{
    AnimationTimeline, Arena, ComputedStyle, FocusPath, InteractionState, NodeId, Rule, StyleError,
};
use taffy::prelude::*;

use crate::focus;
use crate::scroll::ScrollRegistry;
use crate::size_observer::SizeObserverRegistry;

pub struct UiRuntime {
    scope: ComponentScope,
    dirty: DirtyFlag,
    rules: Vec<Rule>,
    root: Box<dyn Fn() -> Element>,
    interaction: InteractionState,
    hovered: Option<NodeId>,
    /// Persistent across renders — see [`FocusPath`]'s own doc for why a
    /// plain [`NodeId`] can't fill this role. `None` means nothing is
    /// focused.
    focused_path: Option<FocusPath>,
    /// [`Self::focused_path`] resolved against the current [`Self::arena`]
    /// — valid only within the arena generation it was resolved in, the
    /// same contract [`Self::hovered`] already has.
    focused_node: Option<NodeId>,
    /// Whether the current focus is keyboard-driven (`:focus-visible`
    /// should match) rather than a mouse click.
    focus_visible: bool,
    arena: Arena,
    styles: HashMap<NodeId, ComputedStyle>,
    layouts: HashMap<NodeId, BoxLayout>,
    /// Carries real `transition`/`@keyframes` state across [`Self::update`]
    /// calls, sampled against a real wall clock captured once at
    /// [`Self::with_rules_and_context`] — see
    /// [`florui_style::AnimationTimeline`]'s own doc.
    animation_timeline: AnimationTimeline,
    animation_epoch: std::time::Instant,
    /// The one long-lived font this runtime's own [`Self::update`] lays out
    /// with — loading one builds a whole Parley `FontContext` (and, with
    /// fontique's default `system_fonts: true`, enumerates the system's
    /// installed fonts), too expensive to redo every render. A host that
    /// also paints borrows it back out via [`Self::geometry_and_font_mut`]
    /// so painting shapes and rasterizes the identical glyphs this runtime
    /// already measured, rather than a second, separately-loaded instance.
    font: florui_text::Font,
    /// Reachable by [`florui_reactive::use_resource`] via context, provided
    /// fresh every render the same way any other context value is. Advanced
    /// once per [`Self::update`] so a fetch that's already resolvable (or
    /// was woken by prior progress) commits without a host needing to know
    /// that async work is involved at all. A background completion woken
    /// only through this executor's own waker (real I/O, a timer) reaches
    /// an event-driven host via [`Self::on_needs_update`], the same
    /// callback a [`florui_reactive::Signal::set`] anywhere under the root
    /// already uses.
    executor: Rc<LocalExecutor>,
    /// Reachable by [`crate::use_committed_size`] via context, the same
    /// way `executor` is. Notified after each [`Self::update`]'s own
    /// layout, once real geometry for that render exists.
    size_observers: Rc<SizeObserverRegistry>,
    /// Reachable by [`crate::use_scroll_offset`] via context, the same way
    /// `size_observers` is — synced right after it, once this render's own
    /// real layout and content extents exist.
    scroll_registry: Rc<ScrollRegistry>,
    /// Extra `provide_context` calls a host supplied at construction — run
    /// every [`Self::update`] (including the very first one, inside
    /// [`Self::with_rules`] itself) alongside `executor`/`size_observers`,
    /// without this window-independent runtime having to know what any of
    /// them actually are. [`crate::desktop::DesktopHost`] uses this to
    /// make its own window-specific capabilities (`WindowControls`)
    /// reachable from components from the very first render onward, the
    /// same way `size_observers` already is for a capability this crate
    /// owns directly.
    extra_context_providers: Vec<Box<dyn Fn()>>,
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
    /// doesn't want to parse it again just to build the runtime. No real
    /// OS accessibility signal reaches this path (devtools, benches, and
    /// most tests use it) — `respect_reduced_motion` stays at its
    /// documented default (`true`) and the initial OS-preference read is
    /// `false` (no preference), matching the deterministic behavior every
    /// non-desktop caller already relies on.
    pub(crate) fn with_rules(
        rules: Vec<Rule>,
        root: impl Fn() -> Element + 'static,
        viewport: Size<AvailableSpace>,
    ) -> Self {
        Self::with_rules_and_context(rules, root, viewport, Vec::new(), true, false, false)
    }

    /// Same as [`Self::with_rules`], but for a host (only
    /// [`crate::desktop::DesktopHost`] today) with its own window-specific
    /// capabilities to make reachable from every render's own
    /// `use_context` calls, starting with this constructor's own first
    /// render — see `extra_context_providers`'s own doc for why that
    /// matters and [`crate::use_committed_size`] for the established
    /// pattern a capability provided this way follows.
    ///
    /// `respect_reduced_motion`/`initial_os_prefers_reduced_motion` must be
    /// constructor arguments, not set afterward via
    /// [`Self::set_os_prefers_reduced_motion`] — this constructor already
    /// runs its own first [`Self::update`] below, before a caller gets the
    /// constructed runtime back, exactly the same trap
    /// `extra_context_providers` already had to avoid (see its own doc).
    /// Seeded here, a `@keyframes` animation already running at mount is
    /// correctly suppressed (or not) on frame one; seeded only afterward,
    /// it would render unsuppressed for exactly one frame regardless of
    /// the real OS preference. `initial_prefers_dark_color_scheme` follows
    /// the identical requirement, for the identical reason — a
    /// `@media (prefers-color-scheme: dark)` rule must already resolve
    /// correctly on this constructor's own first render.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_rules_and_context(
        rules: Vec<Rule>,
        root: impl Fn() -> Element + 'static,
        viewport: Size<AvailableSpace>,
        extra_context_providers: Vec<Box<dyn Fn()>>,
        respect_reduced_motion: bool,
        initial_os_prefers_reduced_motion: bool,
        initial_prefers_dark_color_scheme: bool,
    ) -> Self {
        let (scope, dirty) = ComponentScope::new();
        let mut animation_timeline = AnimationTimeline::new();
        animation_timeline.set_auto_suppress_motion(respect_reduced_motion);
        animation_timeline.set_os_prefers_reduced_motion(initial_os_prefers_reduced_motion);
        animation_timeline.set_prefers_dark_color_scheme(initial_prefers_dark_color_scheme);
        let mut runtime = Self {
            scope,
            dirty,
            rules,
            root: Box::new(root),
            interaction: InteractionState::new(),
            hovered: None,
            focused_path: None,
            focused_node: None,
            focus_visible: false,
            arena: Arena::build(&Element::Fragment(Vec::new())),
            styles: HashMap::new(),
            layouts: HashMap::new(),
            animation_timeline,
            animation_epoch: std::time::Instant::now(),
            font: florui_text::Font::load_embedded(),
            executor: Rc::new(LocalExecutor::new()),
            size_observers: Rc::new(SizeObserverRegistry::new()),
            scroll_registry: Rc::new(ScrollRegistry::new()),
            extra_context_providers,
        };
        runtime.update(viewport);
        runtime
    }

    /// Pushes a freshly-read real OS reduced-motion preference in — a real
    /// desktop host calls this fresh before every [`Self::update`] after
    /// construction (the constructor itself already seeds the initial
    /// value; see [`Self::with_rules_and_context`]'s own doc for why that
    /// distinction matters). Does not itself trigger a render.
    pub(crate) fn set_os_prefers_reduced_motion(&mut self, value: bool) {
        self.animation_timeline.set_os_prefers_reduced_motion(value);
    }

    /// Pushes a freshly-resolved effective color scheme in — a real
    /// desktop host calls this on window construction (via
    /// [`Self::with_rules_and_context`]'s own constructor argument) and
    /// again on every live `WindowEvent::ThemeChanged` while no explicit
    /// `WindowOptions.theme` override is active. Does not itself trigger a
    /// render.
    pub(crate) fn set_prefers_dark_color_scheme(&mut self, value: bool) {
        self.animation_timeline.set_prefers_dark_color_scheme(value);
    }

    /// The flag that marks itself whenever a
    /// [`florui_reactive::Signal::set`] happens anywhere under the root —
    /// register a waker with [`DirtyFlag::on_mark`] to learn about it the
    /// instant it happens rather than polling [`Self::is_dirty`] after
    /// specific events a host already knew to check. Prefer
    /// [`Self::on_needs_update`] for a host loop, which also covers async
    /// resource completions this flag alone does not.
    pub fn dirty_flag(&self) -> DirtyFlag {
        self.dirty.clone()
    }

    /// This runtime's own [`ScrollRegistry`] — the same instance every
    /// render's [`crate::use_scroll_offset`] reaches via context, reachable
    /// here too for a real host's own input handling (e.g. a wheel event)
    /// to drive directly, without going through a component at all.
    pub(crate) fn scroll_registry(&self) -> Rc<ScrollRegistry> {
        Rc::clone(&self.scroll_registry)
    }

    /// Replaces the stylesheet driving every subsequent [`Self::update`].
    /// Only `rules` changes — `scope` (every `Signal`, `use_memo`,
    /// `use_effect`, and the rest of a component's persistent state) is
    /// left completely alone, so a CSS-only reload never resets state the
    /// way a fresh render from scratch would. Does not itself re-render;
    /// call [`Self::update`] afterward to see the new rules take effect.
    pub fn set_rules(&mut self, rules: Vec<Rule>) {
        self.rules = rules;
    }

    /// Registers an additional font (e.g. for a script neither embedded
    /// default covers) with this runtime's own long-lived [`Self::font`]
    /// instance, returning its resolved family name — see
    /// [`florui_text::Font::register`]. Reaching that instance is the
    /// part that didn't exist before this runtime owned it across
    /// renders: registering now genuinely changes what every later
    /// [`Self::update`] shapes and measures with, not just a
    /// freshly-loaded instance nothing else could reach. Same contract as
    /// [`Self::set_rules`]: does not itself re-render — call
    /// [`Self::update`] afterward to see the new font take effect.
    pub fn register_font(&mut self, font_bytes: &[u8]) -> Result<String, florui_text::TextError> {
        self.font.register(font_bytes)
    }

    /// Registers `listener` to run whenever this runtime has something an
    /// event-driven host should react to by calling [`Self::update`]
    /// again: a [`florui_reactive::Signal::set`] anywhere under the root,
    /// or a [`florui_reactive::use_resource`] fetch running on this
    /// runtime's own executor becoming newly pollable. Real progress (a
    /// background thread finishing, an I/O reactor firing) still only
    /// happens on its own — this is only the notification that it did, so
    /// a host that otherwise only wakes for input still learns about it.
    ///
    /// May run on a different thread than whichever owns this runtime, the
    /// same way a real I/O completion can — `listener` itself must not
    /// touch this runtime; only signal that an update is due, the way a
    /// host's own event-loop proxy does. Replaces any previously
    /// registered listener.
    pub fn on_needs_update(&self, listener: impl Fn() + Send + Sync + 'static) {
        let listener = std::sync::Arc::new(listener);
        let for_dirty = std::sync::Arc::clone(&listener);
        self.dirty.on_mark(move || for_dirty());
        self.executor.on_woken(move || listener());
    }

    /// Re-renders the tree against `viewport` and caches the resulting
    /// geometry for [`Self::geometry`]/[`Self::hit_test`] to answer
    /// without rendering again.
    pub fn update(&mut self, viewport: Size<AvailableSpace>) {
        let executor = Rc::clone(&self.executor);
        let size_observers = Rc::clone(&self.size_observers);
        let scroll_registry = Rc::clone(&self.scroll_registry);
        let tree = self.scope.render(|| {
            provide_context(Rc::clone(&executor) as Rc<dyn Executor>);
            provide_context(Rc::clone(&size_observers));
            provide_context(Rc::clone(&scroll_registry));
            for provider in &self.extra_context_providers {
                provider();
            }
            (self.root)()
        });
        // Lets any resource the render just started (or a prior task's
        // waker already requeued) make progress before this frame commits.
        self.executor.run_until_stalled();
        self.arena = Arena::build(&tree);
        self.resolve_focus();
        self.animation_timeline
            .advance_to(self.animation_epoch.elapsed().as_secs_f64());
        let florui_layout::LayoutResult {
            styles,
            layouts,
            content_extents,
        } = florui_layout::compute_with_style(
            &mut self.font,
            &self.arena,
            &self.rules,
            &self.interaction,
            media_viewport(viewport),
            &mut self.animation_timeline,
            viewport,
        )
        .expect("this tree's explicit sizes never produce a layout failure");
        self.styles = styles;
        self.layouts = layouts;
        // After layout, not before: a committed-size observer must see
        // this render's own real geometry, not the previous one's.
        self.size_observers.notify(&self.arena, &self.layouts);
        self.scroll_registry
            .sync(&self.arena, &self.layouts, &content_extents);
    }

    /// Whether the most recent [`Self::update`] left any `transition`/
    /// `@keyframes` animation still in progress — a host's cue to keep
    /// scheduling redraws (and calling `update` again) on its own timer
    /// rather than waiting for the next real input/state event.
    pub fn is_animating(&self) -> bool {
        self.animation_timeline.is_animating()
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

    /// Same geometry as [`Self::geometry`], plus this runtime's own
    /// long-lived font — for a host that paints the geometry it just read,
    /// via [`florui_paint::paint_to_buffer`], which needs a `&mut Font` of
    /// its own. Passing this one back in (rather than a separately loaded
    /// instance) keeps painting and layout shaping the identical glyphs.
    pub fn geometry_and_font_mut(
        &mut self,
    ) -> (
        &Arena,
        &HashMap<NodeId, ComputedStyle>,
        &HashMap<NodeId, BoxLayout>,
        &mut florui_text::Font,
    ) {
        (&self.arena, &self.styles, &self.layouts, &mut self.font)
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
        self.rebuild_interaction();
        true
    }

    /// The currently focused node, against the last computed geometry —
    /// `None` if nothing is focused.
    pub fn focused(&self) -> Option<NodeId> {
        self.focused_node
    }

    /// Sets (or, for `None`, clears) keyboard focus. Returns whether that
    /// actually changed anything — `:focus`/`:focus-visible` can affect
    /// computed style, so a caller should follow a `true` result with a
    /// fresh [`Self::update`]. `via_keyboard` decides whether
    /// `:focus-visible` matches alongside `:focus` — real Tab traversal
    /// passes `true`; a mouse click setting focus (matching real HTML
    /// `:focus` behavior, not `:focus-visible`) passes `false`.
    pub fn set_focused(&mut self, node: Option<NodeId>, via_keyboard: bool) -> bool {
        let focus_visible = via_keyboard && node.is_some();
        if node == self.focused_node && focus_visible == self.focus_visible {
            return false;
        }
        self.focused_node = node;
        self.focused_path = node.map(|id| FocusPath::of(&self.arena, id));
        self.focus_visible = focus_visible;
        self.rebuild_interaction();
        true
    }

    /// Moves keyboard focus to the next focusable element in document
    /// order, wrapping to the first after the last — always keyboard-
    /// origin, so `:focus-visible` matches. Returns whether focus
    /// actually changed (`false` when there is nothing focusable at all).
    pub fn focus_next(&mut self) -> bool {
        self.step_focus(1)
    }

    /// Same as [`Self::focus_next`], stepping backward and wrapping to
    /// the last element after the first.
    pub fn focus_previous(&mut self) -> bool {
        self.step_focus(-1)
    }

    fn step_focus(&mut self, direction: isize) -> bool {
        let order = focus::focus_order(&self.arena);
        if order.is_empty() {
            return self.set_focused(None, true);
        }
        let next_index = match self
            .focused_node
            .and_then(|id| order.iter().position(|&candidate| candidate == id))
        {
            Some(index) => {
                let len = order.len() as isize;
                (index as isize + direction).rem_euclid(len) as usize
            }
            None => {
                if direction >= 0 {
                    0
                } else {
                    order.len() - 1
                }
            }
        };
        self.set_focused(Some(order[next_index]), true)
    }

    /// Re-resolves [`Self::focused_path`] against this render's freshly
    /// rebuilt [`Self::arena`] — a [`NodeId`] from the previous arena
    /// generation isn't safe to reuse directly (see [`FocusPath`]'s own
    /// doc). Clears focus outright if the focused element is no longer
    /// present; deliberately no "restore to trigger" fallback, which is
    /// an overlay-specific concern this slice doesn't attempt.
    fn resolve_focus(&mut self) {
        self.focused_node = match &self.focused_path {
            Some(path) => {
                let candidates = focus::focus_order(&self.arena);
                let resolved = path.resolve(&self.arena, &candidates);
                if resolved.is_none() {
                    self.focused_path = None;
                    self.focus_visible = false;
                }
                resolved
            }
            None => None,
        };
        self.rebuild_interaction();
    }

    /// Rebuilds `self.interaction` from whatever's currently live
    /// (`hovered`, `focused_node`, `focus_visible`) — the single place
    /// that assembles it, so setting one doesn't silently clobber the
    /// others the way replacing it wholesale would.
    fn rebuild_interaction(&mut self) {
        let mut state = InteractionState::new();
        if let Some(id) = self.hovered {
            state = state.with_hovered(id);
        }
        if let Some(id) = self.focused_node {
            state = state.with_focused(id);
            if self.focus_visible {
                state = state.with_focus_visible(id);
            }
        }
        self.interaction = state;
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

/// The viewport `@media`'s own size features resolve against, from
/// whatever the host actually gave [`UiRuntime::update`] — a definite
/// axis is the real one; `MinContent`/`MaxContent` (a host measuring its
/// own intrinsic size, not rendering into a fixed viewport) falls back to
/// [`florui_style::Viewport::default`]'s own placeholder on that axis,
/// since there is no real viewport size to report.
fn media_viewport(viewport: Size<AvailableSpace>) -> florui_style::Viewport {
    let default = florui_style::Viewport::default();
    florui_style::Viewport {
        width: match viewport.width {
            AvailableSpace::Definite(width) => width,
            AvailableSpace::MinContent | AvailableSpace::MaxContent => default.width,
        },
        height: match viewport.height {
            AvailableSpace::Definite(height) => height,
            AvailableSpace::MinContent | AvailableSpace::MaxContent => default.height,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use florui_style::Rgba;

    use florui::prelude::*;
    use florui_reactive::testing::manual_future;
    use florui_reactive::{Resource, use_context, use_resource};

    use super::*;
    use crate::use_committed_size;

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

    /// The concrete, automatable half of "CSS reload preserves state": a
    /// real winit file watcher and event loop are only exercisable
    /// manually, but the actual mechanism — swapping `rules` without
    /// touching `scope` — needs no window at all to prove.
    #[test]
    fn set_rules_changes_style_without_resetting_component_state() {
        let root = || {
            let count = use_signal(|| 0);
            let clicked = count.clone();
            view! {
                <button id="status" onclick={move || clicked.set(clicked.get() + 1)}>
                    {count.get().to_string()}
                </button>
            }
        };
        let mut runtime = UiRuntime::with_rules(Vec::new(), root, viewport());
        let status = find_status(&runtime);
        runtime.dispatch_click(status);
        runtime.update(viewport());
        assert_eq!(status_text(find_status(&runtime), &runtime), "1");

        let new_rules = florui_style::parse_stylesheet("#status { color: #ff0000; }")
            .expect("a trivial rule always parses");
        runtime.set_rules(new_rules);
        runtime.update(viewport());

        assert_eq!(
            status_text(find_status(&runtime), &runtime),
            "1",
            "swapping in a real stylesheet must not reset the click count set before it"
        );
        let (_, styles, _) = runtime.geometry();
        let status = find_status(&runtime);
        assert_eq!(
            styles[&status].color,
            Rgba::opaque(0xff, 0x00, 0x00),
            "the new rule must actually take effect, not just fail to reset state"
        );
    }

    /// Proves `register_font` reaches the runtime's own long-lived `Font`
    /// instance — the one every real `Self::update` shapes and measures
    /// with — not a separate, freshly-loaded one nothing else could ever
    /// see. Before the font became a persistent field on `UiRuntime`
    /// itself, there was no instance for a caller to register an extra
    /// font onto in the first place: each `compute_layout` call built and
    /// discarded its own.
    #[test]
    fn register_font_reaches_the_same_persistent_font_instance_across_updates() {
        let mut runtime = UiRuntime::with_rules(Vec::new(), || view! { <div /> }, viewport());

        let family_name = runtime
            .register_font(florui_text::EMBEDDED_MONOSPACE_FONT)
            .expect("a real embedded font file must register successfully");
        assert!(!family_name.is_empty());

        // Does not itself re-render (matches set_rules's own contract) --
        // an explicit update afterward must still work normally, proving
        // registering didn't leave the runtime's own font in a broken or
        // replaced state.
        runtime.update(viewport());

        let (.., font) = runtime.geometry_and_font_mut();
        let metrics = font.measure(florui_text::FontFamily::SansSerif, "x", 16.0, 400.0);
        assert!(
            metrics.width > 0.0,
            "the same font instance register_font touched must still measure real text \
             correctly afterward"
        );
    }

    /// `DesktopHost` needs a window-specific capability (`WindowControls`)
    /// reachable from `use_context` starting with this constructor's own
    /// first render, not only from the second render onward — a component
    /// that unconditionally calls `use_window_controls()` at mount would
    /// otherwise silently see `None` on its very first frame. Registering
    /// a provider only *after* construction (a plain setter, rather than
    /// a constructor argument) would miss exactly that first render, since
    /// the constructor already ran its own first `update` before a setter
    /// call could ever run.
    #[test]
    fn extra_context_providers_apply_starting_from_the_very_first_render() {
        let seen = Rc::new(RefCell::new(None));
        let seen_in_root = Rc::clone(&seen);
        let root = move || {
            *seen_in_root.borrow_mut() = use_context::<i32>();
            view! { <div /> }
        };
        let providers: Vec<Box<dyn Fn()>> =
            vec![Box::new(|| florui_reactive::provide_context(42_i32))];

        let _runtime = UiRuntime::with_rules_and_context(
            Vec::new(),
            root,
            viewport(),
            providers,
            true,
            false,
            false,
        );

        assert_eq!(
            *seen.borrow(),
            Some(42),
            "a provider passed to the constructor must run during the constructor's own first render"
        );
    }

    /// Same shape as the `extra_context_providers` test above, for the
    /// identical reason: `respect_reduced_motion`/
    /// `initial_os_prefers_reduced_motion` must be constructor arguments,
    /// not set afterward, because the constructor already runs its own
    /// first `update` before a caller could ever call
    /// `set_os_prefers_reduced_motion`. Uses a `@keyframes` animation, not
    /// a transition: a transition needs a *previous* render to change away
    /// from, which the constructor's own first render never has — a
    /// `@keyframes` animation is active from the very first render an
    /// element with `animation-name` appears in, exactly the case this
    /// guards.
    #[test]
    fn reduced_motion_suppression_applies_starting_from_the_very_first_render() {
        let css = "
            .box {
                opacity: 1;
                animation-name: dim;
                animation-duration: 10s;
                animation-fill-mode: forwards;
            }
            @keyframes dim {
                from { opacity: 0.3; }
                to { opacity: 0.3; }
            }
        ";
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let root = || view! { <div class="box" /> };

        let runtime = UiRuntime::with_rules_and_context(
            rules,
            root,
            viewport(),
            Vec::new(),
            true,
            true,
            false,
        );

        let (arena, styles, _) = runtime.geometry();
        let node = arena.roots()[0];
        assert_eq!(
            styles[&node].opacity, 1.0,
            "suppression seeded at construction must already apply to the constructor's own \
             first render (plain opacity: 1), not splice in the animation's 0.3 value"
        );
    }

    /// Same shape again, for `initial_prefers_dark_color_scheme`: a
    /// `@media (prefers-color-scheme: dark)` rule must already resolve
    /// correctly on the constructor's own first render, not just on
    /// updates after it.
    #[test]
    fn color_scheme_applies_starting_from_the_very_first_render() {
        let css = "
            .box { background-color: #ffffff; }
            @media (prefers-color-scheme: dark) {
                .box { background-color: #000000; }
            }
        ";
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let root = || view! { <div class="box" /> };

        let runtime = UiRuntime::with_rules_and_context(
            rules,
            root,
            viewport(),
            Vec::new(),
            true,
            false,
            true,
        );

        let (arena, styles, _) = runtime.geometry();
        let node = arena.roots()[0];
        assert_eq!(
            styles[&node].background_color,
            florui_style::Rgba::opaque(0, 0, 0),
            "the dark-scheme value seeded at construction must already apply to the \
             constructor's own first render"
        );
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

    /// Proves the bridge an event-driven host needs actually exists: a
    /// fetch resolved by a real OS thread (not this test calling anything
    /// on the resource or the runtime) still reaches the rendered tree,
    /// with [`UiRuntime::on_needs_update`] as the only thing telling this
    /// test when to call [`UiRuntime::update`] again — no click, no
    /// resize, no polling loop. `on_needs_update` is registered right
    /// after construction, before the mount-effect catch-up update below —
    /// the same order [`crate::run`]'s real desktop host uses — so a fetch
    /// that resolves unusually fast still has a listener in place; the
    /// artificial delay is extra margin, not what makes this correct.
    #[test]
    fn a_background_completion_notifies_on_needs_update_without_a_polling_loop() {
        let root = || {
            let resource = use_resource("key", |_| {
                florui_reactive::blocking::spawn_blocking(|| {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    Ok::<i32, &'static str>(42)
                })
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
        let (needs_update_tx, needs_update_rx) = std::sync::mpsc::channel();
        runtime.on_needs_update(move || {
            let _ = needs_update_tx.send(());
        });
        if runtime.is_dirty() {
            runtime.clear_dirty();
            runtime.update(viewport());
        }
        assert_eq!(status_text(find_status(&runtime), &runtime), "pending");

        needs_update_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the background thread's completion must reach on_needs_update on its own");
        // As above: this render's own snapshot still reads the
        // pre-completion state, since `update` only commits `Ready` (via
        // run_until_stalled) after building it.
        runtime.update(viewport());
        assert!(runtime.is_dirty());
        runtime.clear_dirty();
        runtime.update(viewport());
        assert_eq!(status_text(find_status(&runtime), &runtime), "ready:42");
    }

    /// loading-boundaries.md's own acceptance requirement: "an externally
    /// completed future reveals content without a manual update, click,
    /// or resize." Same shape as the resource-only version above, with a
    /// `loading_boundary` deciding between a fallback and the real
    /// content instead of a component branching on `Resource` itself.
    #[test]
    fn a_loading_boundary_reveals_content_on_a_real_background_completion() {
        let root = || {
            let resource = use_resource("key", |_| {
                florui_reactive::blocking::spawn_blocking(|| {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    Ok::<i32, &'static str>(42)
                })
            });
            florui_reactive::loading_boundary(
                &[&resource],
                |_refreshing| view! { <div id="status">{"content"}</div> },
                || view! { <div id="status">{"fallback"}</div> },
            )
        };

        let mut runtime = UiRuntime::with_rules(Vec::new(), root, viewport());
        let (needs_update_tx, needs_update_rx) = std::sync::mpsc::channel();
        runtime.on_needs_update(move || {
            let _ = needs_update_tx.send(());
        });
        if runtime.is_dirty() {
            runtime.clear_dirty();
            runtime.update(viewport());
        }
        assert_eq!(status_text(find_status(&runtime), &runtime), "fallback");

        needs_update_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the background thread's completion must reach on_needs_update on its own");
        // As above: this render's own snapshot still reads the
        // pre-completion state.
        runtime.update(viewport());
        assert!(runtime.is_dirty());
        runtime.clear_dirty();
        runtime.update(viewport());
        assert_eq!(status_text(find_status(&runtime), &runtime), "content");
    }

    #[test]
    fn use_committed_size_notifies_after_layout_and_coalesces_unchanged_sizes() {
        let sizes = Rc::new(RefCell::new(Vec::<(f32, f32)>::new()));
        let sizes_for_root = Rc::clone(&sizes);

        let root = move || {
            let sizes = Rc::clone(&sizes_for_root);
            let long = use_signal(|| false);
            let toggle = long.clone();
            let text = if long.get() {
                "a much longer run of text than before"
            } else {
                "short"
            };
            use_committed_size("box", move |w, h| sizes.borrow_mut().push((w, h)));
            view! {
                <div>
                    <div id="box">{text}</div>
                    <button onclick={move || toggle.set(true)} />
                </div>
            }
        };

        let mut runtime = UiRuntime::with_rules(Vec::new(), root, Size::MAX_CONTENT);
        assert_eq!(
            sizes.borrow().len(),
            1,
            "the very first layout already has a committed size to report"
        );
        let (short_width, _) = sizes.borrow()[0];

        let button = runtime
            .geometry()
            .0
            .find(|a, id| a.tag(id) == "button")
            .unwrap();
        runtime.dispatch_click(button);
        runtime.update(Size::MAX_CONTENT);

        assert_eq!(
            sizes.borrow().len(),
            2,
            "the text grew wider, so the observer must fire again"
        );
        let (long_width, _) = sizes.borrow()[1];
        assert!(
            long_width > short_width,
            "the longer text must measure wider than the short one"
        );

        // Nothing changed this time — must not renotify.
        runtime.update(Size::MAX_CONTENT);
        assert_eq!(
            sizes.borrow().len(),
            2,
            "an unchanged committed size must not renotify"
        );
    }

    #[test]
    fn use_committed_size_stops_firing_once_its_scope_unmounts() {
        let sizes = Rc::new(RefCell::new(0));
        let sizes_for_root = Rc::clone(&sizes);
        let show = Rc::new(Cell::new(true));
        let show_for_root = Rc::clone(&show);

        let root = move || {
            let sizes = Rc::clone(&sizes_for_root);
            if show_for_root.get() {
                use_child_scope_keyed("observed", move || {
                    use_committed_size("box", move |_, _| *sizes.borrow_mut() += 1);
                    view! { <div id="box">{"content"}</div> }
                })
            } else {
                view! { <div /> }
            }
        };

        let mut runtime = UiRuntime::with_rules(Vec::new(), root, viewport());
        assert_eq!(*sizes.borrow(), 1);

        show.set(false);
        runtime.update(viewport());
        assert_eq!(
            *sizes.borrow(),
            1,
            "unmounting the observing scope must dispose its attachment, not fire it again"
        );
    }

    fn three_buttons_runtime() -> UiRuntime {
        UiRuntime::with_rules(
            Vec::new(),
            || {
                view! {
                    <div>
                        <button id="a">{"A"}</button>
                        <button id="b">{"B"}</button>
                        <button id="c">{"C"}</button>
                    </div>
                }
            },
            viewport(),
        )
    }

    fn node_id(runtime: &UiRuntime, id_attr: &str) -> NodeId {
        let (arena, ..) = runtime.geometry();
        arena
            .find(|arena, node| arena.id_attr(node) == Some(id_attr))
            .unwrap()
    }

    #[test]
    fn focus_next_moves_forward_through_document_order_and_wraps() {
        let mut runtime = three_buttons_runtime();
        let (a, b, c) = (
            node_id(&runtime, "a"),
            node_id(&runtime, "b"),
            node_id(&runtime, "c"),
        );

        assert!(runtime.focus_next());
        assert_eq!(runtime.focused(), Some(a));
        assert!(runtime.focus_next());
        assert_eq!(runtime.focused(), Some(b));
        assert!(runtime.focus_next());
        assert_eq!(runtime.focused(), Some(c));
        assert!(
            runtime.focus_next(),
            "tabbing past the last element must wrap back to the first"
        );
        assert_eq!(runtime.focused(), Some(a));
    }

    #[test]
    fn focus_previous_moves_backward_and_wraps() {
        let mut runtime = three_buttons_runtime();
        let (a, c) = (node_id(&runtime, "a"), node_id(&runtime, "c"));

        assert!(
            runtime.focus_previous(),
            "shift-tabbing with nothing focused must land on the last element"
        );
        assert_eq!(runtime.focused(), Some(c));
        assert!(runtime.focus_previous());
        assert_eq!(runtime.focused(), Some(node_id(&runtime, "b")));
        assert!(runtime.focus_previous());
        assert_eq!(runtime.focused(), Some(a));
        assert!(
            runtime.focus_previous(),
            "shift-tabbing past the first element must wrap to the last"
        );
        assert_eq!(runtime.focused(), Some(c));
    }

    #[test]
    fn focus_applies_only_to_the_focused_button() {
        let mut runtime = three_buttons_runtime();
        runtime.set_rules(
            florui_style::parse_stylesheet(
                "button { background-color: #111111; } button:focus { background-color: #222222; }",
            )
            .unwrap(),
        );
        let a = node_id(&runtime, "a");
        let b = node_id(&runtime, "b");
        runtime.set_focused(Some(a), true);
        runtime.update(viewport());

        let (_, styles, _) = runtime.geometry();
        assert_eq!(styles[&a].background_color, Rgba::opaque(0x22, 0x22, 0x22));
        assert_eq!(styles[&b].background_color, Rgba::opaque(0x11, 0x11, 0x11));
    }

    #[test]
    fn focus_visible_applies_after_keyboard_focus_but_not_a_pointer_click() {
        let mut runtime = three_buttons_runtime();
        runtime.set_rules(
            florui_style::parse_stylesheet(
                "button { background-color: #111111; } button:focus-visible { background-color: #333333; }",
            )
            .unwrap(),
        );
        let a = node_id(&runtime, "a");

        runtime.set_focused(Some(a), false);
        runtime.update(viewport());
        let (_, styles, _) = runtime.geometry();
        assert_eq!(
            styles[&a].background_color,
            Rgba::opaque(0x11, 0x11, 0x11),
            ":focus-visible must not match a pointer-origin focus"
        );

        runtime.set_focused(Some(a), true);
        runtime.update(viewport());
        let (_, styles, _) = runtime.geometry();
        assert_eq!(styles[&a].background_color, Rgba::opaque(0x33, 0x33, 0x33));
    }

    #[test]
    fn removing_the_focused_element_clears_focus_without_panicking() {
        let show = Rc::new(Cell::new(true));
        let show_for_root = Rc::clone(&show);
        let mut runtime = UiRuntime::with_rules(
            Vec::new(),
            move || {
                if show_for_root.get() {
                    view! { <button id="target">{"Go"}</button> }
                } else {
                    view! { <div /> }
                }
            },
            viewport(),
        );
        let target = node_id(&runtime, "target");
        runtime.set_focused(Some(target), true);
        assert_eq!(runtime.focused(), Some(target));

        show.set(false);
        runtime.update(viewport());

        assert_eq!(
            runtime.focused(),
            None,
            "focus must clear outright once its element is gone"
        );
    }
}

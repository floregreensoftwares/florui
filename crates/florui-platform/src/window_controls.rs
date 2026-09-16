//! A real window's own drag/minimize/maximize/close, reachable from
//! ordinary components the same way [`crate::use_committed_size`] reaches
//! `SizeObserverRegistry` — [`crate::desktop::DesktopHost`] provides one
//! [`WindowControls`] instance through context every render, starting
//! with the very first one, not tied to
//! [`crate::appearance::DecorationMode::Custom`] specifically: a plain
//! system-decorated window can still usefully offer its own draggable
//! toolbar region or extra chrome alongside the OS's own title bar, the
//! same way many real desktop apps do.
//!
//! This is the capability [`crate::appearance::WindowCapability::CustomDecorations`]
//! is *for* — an application-drawn title bar built from ordinary Florui
//! components needs some way to actually move, minimize, and maximize the
//! real window from inside that component tree, since none of that comes
//! for free once the OS stops drawing (and hit-testing) its own chrome.
//!
//! # Marking a draggable region
//!
//! [`WINDOW_DRAG_REGION_ID`] is a well-known `id` value: give exactly one
//! leaf element that `id` (a flex spacer inside a custom title bar row,
//! say — never an element with its own interactive children, since a
//! press only starts a drag when it hits *this exact* element, not an
//! ancestor of whatever else was actually pressed) and
//! [`crate::desktop::DesktopHost`] calls [`WindowControls::drag`] itself
//! the moment that element is pressed, instead of treating the press as
//! a normal click candidate. No new attribute or markup concept on
//! `florui`/`florui-style`'s own side — this reuses the `id` attribute
//! every element already has, kept entirely inside `florui-platform`
//! itself.

use std::rc::Rc;
use std::sync::Arc;

use florui_reactive::use_context;
use winit::window::Window;

/// See this module's own doc, "Marking a draggable region."
pub const WINDOW_DRAG_REGION_ID: &str = "florui-window-drag-region";

/// A real, live handle to the window a [`crate::desktop::DesktopHost`] is
/// running — every method here acts on the actual window immediately, not
/// a request the host might defer or ignore.
pub struct WindowControls {
    window: Arc<Window>,
    request_close: Box<dyn Fn()>,
}

impl WindowControls {
    pub(crate) fn new(window: Arc<Window>, request_close: impl Fn() + 'static) -> Self {
        Self {
            window,
            request_close: Box::new(request_close),
        }
    }

    /// Starts an OS-native move-drag of the real window from wherever the
    /// mouse currently is — the same interaction the system title bar
    /// gives for free, for a component (a custom title bar's own
    /// draggable region, say) that wants it back. Call from a real press
    /// handler; a `winit`-level failure (no button currently held, an
    /// unsupported platform) is silently ignored — there is nothing a
    /// caller could usefully do differently either way, and dragging
    /// simply not starting is a safe, visible-enough failure mode on its
    /// own.
    pub fn drag(&self) {
        let _ = self.window.drag_window();
    }

    /// Whether the real window is currently maximized — a live query, not
    /// a cached guess, so a custom maximize/restore button can show the
    /// right icon even when something other than this button changed the
    /// state (double-clicking a draggable region, a window-snap gesture).
    pub fn is_maximized(&self) -> bool {
        self.window.is_maximized()
    }

    pub fn minimize(&self) {
        self.window.set_minimized(true);
    }

    /// Maximizes if not already, restores otherwise — the single toggle a
    /// custom title bar's own maximize/restore button needs, rather than
    /// making every caller re-read [`Self::is_maximized`] first.
    pub fn toggle_maximize(&self) {
        let maximized = self.window.is_maximized();
        self.window.set_maximized(!maximized);
    }

    /// Requests the same shutdown a real click on the OS's own close
    /// button would — routed back through
    /// [`crate::desktop::DesktopHost`]'s own event loop, not a direct
    /// `winit` call, so it goes through the identical `CloseRequested`
    /// path either kind of close button ends up taking.
    pub fn close(&self) {
        (self.request_close)();
    }
}

/// Reads the [`WindowControls`] [`crate::desktop::DesktopHost`] provides
/// every render. `None` outside a real desktop host — a component using
/// this should degrade (hide the custom title bar row it would have
/// driven, say) rather than assume it's always present, unlike
/// [`crate::use_committed_size`]'s own `SizeObserverRegistry`, which every
/// [`crate::UiRuntime`] provides unconditionally.
pub fn use_window_controls() -> Option<Rc<WindowControls>> {
    use_context::<Rc<WindowControls>>()
}

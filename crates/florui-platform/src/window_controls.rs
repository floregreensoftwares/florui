//! A real window's own drag/minimize/maximize/close, reachable from
//! components the same way [`crate::use_committed_size`] reaches
//! `SizeObserverRegistry` — [`crate::desktop::DesktopHost`] provides one
//! [`WindowControls`] every render, starting with the first. Not tied to
//! [`crate::appearance::DecorationMode::Custom`]: a system-decorated
//! window can still want a draggable toolbar or extra chrome.
//!
//! # Marking a draggable region
//!
//! [`WINDOW_DRAG_REGION_ID`] is a well-known `id`: give one leaf element
//! that id (never one with interactive children — a press only drags
//! when it hits this exact element) and [`crate::desktop::DesktopHost`]
//! calls [`WindowControls::drag`] on press instead of treating it as a
//! click. Reuses the existing `id` attribute; no new markup concept.
//!
//! # Marking an interactive region under [`InputMode::Selective`]
//!
//! [`WINDOW_INPUT_REGION_CLASS`] is the same idea for
//! [`WindowControls::set_input_mode`]'s selective mode, but for
//! potentially many elements at once, so it's a class, not an `id`:
//! [`crate::desktop::DesktopHost`] re-collects every element carrying it
//! after each render and keeps the real window's own input targeting in
//! sync — an app never computes or tracks a rectangle itself.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use florui_reactive::use_context;
use winit::window::Window;

/// See this module's own doc, "Marking a draggable region."
pub const WINDOW_DRAG_REGION_ID: &str = "florui-window-drag-region";

/// See this module's own doc, "Marking an interactive region under
/// `InputMode::Selective`."
pub const WINDOW_INPUT_REGION_CLASS: &str = "florui-window-input-region";

/// What the real window does with mouse input — see
/// [`WindowControls::set_input_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Ordinary input targeting — the window receives every click
    /// normally, exactly like before this existed.
    #[default]
    Normal,
    /// The whole window lets every click pass through to whatever is
    /// behind it, on any other application — see
    /// [`crate::overlay`]'s own doc. No-op outside Windows.
    Passthrough,
    /// Only elements carrying [`WINDOW_INPUT_REGION_CLASS`] receive
    /// clicks; everywhere else passes through, on any other
    /// application, the same as [`InputMode::Passthrough`] — see
    /// `crate::os::windows::input_regions`'s own doc for why this needs
    /// a live cursor-tracking hook rather than a plain per-window hit
    /// test. No-op outside Windows.
    Selective,
}

/// A real screen rectangle (physical pixels, not logical/DPI-scaled) —
/// what [`InputMode::Selective`]'s own hit-testing compares the cursor
/// against. Computed by [`crate::desktop::DesktopHost`] from the real
/// committed layout; never constructed by application code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Split out of [`WindowControls`] so it's testable without a real
/// `Window`. `None` (no guard registered) always confirms.
#[derive(Default)]
struct CloseGuard(RefCell<Option<Box<dyn Fn() -> bool>>>);

impl CloseGuard {
    fn set(&self, guard: impl Fn() -> bool + 'static) {
        *self.0.borrow_mut() = Some(Box::new(guard));
    }

    fn clear(&self) {
        *self.0.borrow_mut() = None;
    }

    fn confirm(&self) -> bool {
        match self.0.borrow().as_ref() {
            Some(guard) => guard(),
            None => true,
        }
    }
}

/// A real, live handle to the window a [`crate::desktop::DesktopHost`] is
/// running — every method here acts on the actual window immediately, not
/// a request the host might defer or ignore.
pub struct WindowControls {
    window: Arc<Window>,
    request_close: Box<dyn Fn()>,
    close_guard: CloseGuard,
    input_mode: Cell<InputMode>,
}

impl WindowControls {
    pub(crate) fn new(window: Arc<Window>, request_close: impl Fn() + 'static) -> Self {
        Self {
            window,
            request_close: Box::new(request_close),
            close_guard: CloseGuard::default(),
            input_mode: Cell::new(InputMode::Normal),
        }
    }

    /// Starts an OS-native move-drag from the current mouse position. A
    /// `winit`-level failure is silently ignored — nothing useful to do
    /// differently either way.
    pub fn drag(&self) {
        let _ = self.window.drag_window();
    }

    /// Live query, not a cached guess — picks up snap/double-click too.
    pub fn is_maximized(&self) -> bool {
        self.window.is_maximized()
    }

    pub fn minimize(&self) {
        self.window.set_minimized(true);
    }

    pub fn toggle_maximize(&self) {
        let maximized = self.window.is_maximized();
        self.window.set_maximized(!maximized);
    }

    pub fn set_always_on_top(&self, enabled: bool) {
        self.window.set_window_level(if enabled {
            winit::window::WindowLevel::AlwaysOnTop
        } else {
            winit::window::WindowLevel::Normal
        });
    }

    /// Borderless fullscreen on the window's current monitor.
    pub fn set_fullscreen(&self, enabled: bool) {
        self.window
            .set_fullscreen(enabled.then_some(winit::window::Fullscreen::Borderless(None)));
    }

    pub fn is_fullscreen(&self) -> bool {
        self.window.fullscreen().is_some()
    }

    /// Switches how the real window targets mouse input — see
    /// [`InputMode`]'s own doc for what each mode does. `false` (no-op,
    /// though this mode is still recorded and reported by
    /// [`Self::input_mode`]) outside Windows.
    pub fn set_input_mode(&self, mode: InputMode) -> bool {
        self.input_mode.set(mode);
        crate::overlay::set_input_mode(&self.window, mode)
    }

    /// The mode last requested via [`Self::set_input_mode`] —
    /// [`InputMode::Normal`] if never called.
    pub fn input_mode(&self) -> InputMode {
        self.input_mode.get()
    }

    /// Feeds [`InputMode::Selective`]'s live hit-testing the current
    /// screen rectangles of every [`WINDOW_INPUT_REGION_CLASS`] element —
    /// called by [`crate::desktop::DesktopHost`] after every render; a
    /// no-op whenever [`Self::input_mode`] isn't
    /// [`InputMode::Selective`].
    pub(crate) fn sync_input_regions(&self, regions: &[ScreenRect]) {
        if self.input_mode.get() == InputMode::Selective {
            crate::overlay::sync_input_regions(&self.window, regions);
        }
    }

    /// Same shutdown path as the OS's own close button, including
    /// whatever [`Self::set_close_guard`] currently allows or vetoes.
    pub fn close(&self) {
        (self.request_close)();
    }

    /// `guard` decides every future close request — the real OS close
    /// button as much as [`Self::close`]. Returning `false` leaves the
    /// window open; showing a "you have unsaved changes" dialog is the
    /// caller's own job. Replaces any previous guard; pair with
    /// [`florui_reactive::use_effect`] and [`Self::clear_close_guard`] as
    /// its cleanup so it tracks live reactive state, not a stale snapshot.
    pub fn set_close_guard(&self, guard: impl Fn() -> bool + 'static) {
        self.close_guard.set(guard);
    }

    /// Removes the guard; future closes are allowed again.
    pub fn clear_close_guard(&self) {
        self.close_guard.clear();
    }

    pub(crate) fn confirm_close(&self) -> bool {
        self.close_guard.confirm()
    }
}

impl Drop for WindowControls {
    /// [`InputMode::Selective`]'s hook and [`InputMode::Passthrough`]'s
    /// `WS_EX_TRANSPARENT` both outlive this struct otherwise — nothing
    /// else ever un-sets them once the window itself is gone.
    fn drop(&mut self) {
        if self.input_mode.get() != InputMode::Normal {
            crate::overlay::set_input_mode(&self.window, InputMode::Normal);
        }
    }
}

/// Reads the [`WindowControls`] [`crate::desktop::DesktopHost`] provides
/// every render. `None` outside a real desktop host.
pub fn use_window_controls() -> Option<Rc<WindowControls>> {
    use_context::<Rc<WindowControls>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirms_by_default_with_no_guard_registered() {
        let guard = CloseGuard::default();
        assert!(guard.confirm(), "no guard set must never block a close");
    }

    #[test]
    fn a_registered_guard_can_veto_a_close() {
        let guard = CloseGuard::default();
        guard.set(|| false);
        assert!(!guard.confirm());
    }

    #[test]
    fn a_registered_guard_can_allow_a_close() {
        let guard = CloseGuard::default();
        guard.set(|| true);
        assert!(guard.confirm());
    }

    #[test]
    fn confirm_reads_the_guards_current_state_not_a_snapshot() {
        let allowed = Rc::new(RefCell::new(false));
        let guard = CloseGuard::default();
        let allowed_in_guard = Rc::clone(&allowed);
        guard.set(move || *allowed_in_guard.borrow());

        assert!(
            !guard.confirm(),
            "the guard's own real state starts out vetoing the close"
        );
        *allowed.borrow_mut() = true;
        assert!(
            guard.confirm(),
            "confirm() must re-read the guard's current state, not cache the first result"
        );
    }

    #[test]
    fn clearing_a_guard_restores_the_default_allow_behavior() {
        let guard = CloseGuard::default();
        guard.set(|| false);
        assert!(!guard.confirm());

        guard.clear();
        assert!(
            guard.confirm(),
            "clearing the guard must go back to the no-guard default, not stay vetoed"
        );
    }

    #[test]
    fn setting_a_new_guard_replaces_the_previous_one() {
        let guard = CloseGuard::default();
        guard.set(|| false);
        guard.set(|| true);
        assert!(
            guard.confirm(),
            "the second set() must fully replace the first, not combine with it"
        );
    }
}

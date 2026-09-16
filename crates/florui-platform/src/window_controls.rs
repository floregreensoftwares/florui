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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use florui_reactive::use_context;
use winit::window::Window;

/// See this module's own doc, "Marking a draggable region."
pub const WINDOW_DRAG_REGION_ID: &str = "florui-window-drag-region";

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
}

impl WindowControls {
    pub(crate) fn new(window: Arc<Window>, request_close: impl Fn() + 'static) -> Self {
        Self {
            window,
            request_close: Box::new(request_close),
            close_guard: CloseGuard::default(),
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

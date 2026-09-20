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
use winit::window::{BadIcon, Icon, Window};

use crate::file_dialog::{
    OpenFileDialogOptions, OpenFileDialogOutcome, SaveFileDialogOptions, SaveFileDialogOutcome,
};

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

/// Converts an already-decoded [`florui_icon::RawIcon`] into a real
/// `winit` icon -- shared by [`WindowControls::set_icon`] and
/// [`crate::desktop`]'s own creation-time icon application, so the
/// straight-RGBA-to-`Icon` conversion exists in exactly one place. Errors
/// straight through, unwrapped, matching [`crate::desktop::RunError`]'s
/// own existing convention of exposing raw `winit`/`softbuffer`/`notify`
/// errors rather than wrapping them in a florui-local type.
pub(crate) fn to_winit_icon(icon: &florui_icon::RawIcon) -> Result<Icon, BadIcon> {
    Icon::from_rgba(icon.rgba.clone(), icon.width, icon.height)
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

/// The app's own file-dialog callback, deliberately not `Send` (it may
/// close over `Signal`s) -- never leaves the UI thread; only the plain
/// outcome data crosses over from the background thread.
type PendingDialogCallback<Outcome> = RefCell<Option<Box<dyn FnOnce(Outcome)>>>;

/// A real, live handle to the window a [`crate::desktop::DesktopHost`] is
/// running — every method here acts on the actual window immediately, not
/// a request the host might defer or ignore.
pub struct WindowControls {
    window: Arc<Window>,
    request_close: Box<dyn Fn()>,
    close_guard: CloseGuard,
    input_mode: Cell<InputMode>,
    /// `Arc`, not `Box` like `request_close` -- a *clone* of this needs to
    /// move into the background thread [`Self::open_file_dialog`] spawns,
    /// while the original stays here for the next call.
    notify_open_dialog_result: Arc<dyn Fn(OpenFileDialogOutcome) + Send + Sync>,
    notify_save_dialog_result: Arc<dyn Fn(SaveFileDialogOutcome) + Send + Sync>,
    /// `Some` between [`Self::open_file_dialog`]/`save_file_dialog` being
    /// called and its result arriving.
    pending_open_dialog: PendingDialogCallback<OpenFileDialogOutcome>,
    pending_save_dialog: PendingDialogCallback<SaveFileDialogOutcome>,
}

impl WindowControls {
    pub(crate) fn new(
        window: Arc<Window>,
        request_close: impl Fn() + 'static,
        notify_open_dialog_result: impl Fn(OpenFileDialogOutcome) + Send + Sync + 'static,
        notify_save_dialog_result: impl Fn(SaveFileDialogOutcome) + Send + Sync + 'static,
    ) -> Self {
        Self {
            window,
            request_close: Box::new(request_close),
            close_guard: CloseGuard::default(),
            input_mode: Cell::new(InputMode::Normal),
            notify_open_dialog_result: Arc::new(notify_open_dialog_result),
            notify_save_dialog_result: Arc::new(notify_save_dialog_result),
            pending_open_dialog: RefCell::new(None),
            pending_save_dialog: RefCell::new(None),
        }
    }

    /// Shows a real native "Open File" dialog, asynchronously: this
    /// method returns immediately, and `on_result` runs later, on this
    /// same UI thread, once the dialog closes -- safe to touch `Signal`s
    /// or any other component state directly from `on_result`. If a
    /// dialog is already pending for this window, `on_result` is called
    /// immediately with [`OpenFileDialogOutcome::Failed`] rather than
    /// silently replacing or queuing behind the first one.
    pub fn open_file_dialog(
        &self,
        options: OpenFileDialogOptions,
        on_result: impl FnOnce(OpenFileDialogOutcome) + 'static,
    ) {
        if self.pending_open_dialog.borrow().is_some() {
            on_result(OpenFileDialogOutcome::Failed(
                "another open-file dialog is already showing for this window".to_owned(),
            ));
            return;
        }
        *self.pending_open_dialog.borrow_mut() = Some(Box::new(on_result));
        let notify = Arc::clone(&self.notify_open_dialog_result);
        crate::file_dialog::spawn_open_dialog(&self.window, options, move |outcome| {
            notify(outcome);
        });
    }

    /// See [`Self::open_file_dialog`]'s own doc -- same contract, for a
    /// native "Save File" dialog instead.
    pub fn save_file_dialog(
        &self,
        options: SaveFileDialogOptions,
        on_result: impl FnOnce(SaveFileDialogOutcome) + 'static,
    ) {
        if self.pending_save_dialog.borrow().is_some() {
            on_result(SaveFileDialogOutcome::Failed(
                "another save-file dialog is already showing for this window".to_owned(),
            ));
            return;
        }
        *self.pending_save_dialog.borrow_mut() = Some(Box::new(on_result));
        let notify = Arc::clone(&self.notify_save_dialog_result);
        crate::file_dialog::spawn_save_dialog(&self.window, options, move |outcome| {
            notify(outcome);
        });
    }

    /// Called by [`crate::desktop::DesktopHost`] once `outcome` arrives
    /// via its own `UserEvent` -- takes and invokes whichever callback
    /// [`Self::open_file_dialog`] stored, on this same UI thread.
    pub(crate) fn deliver_open_dialog_result(&self, outcome: OpenFileDialogOutcome) {
        if let Some(callback) = self.pending_open_dialog.borrow_mut().take() {
            callback(outcome);
        }
    }

    /// See [`Self::deliver_open_dialog_result`]'s own doc -- same
    /// contract, for [`Self::save_file_dialog`] instead.
    pub(crate) fn deliver_save_dialog_result(&self, outcome: SaveFileDialogOutcome) {
        if let Some(callback) = self.pending_save_dialog.borrow_mut().take() {
            callback(outcome);
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

    /// Sets this already-open window's icon at runtime -- the "update"
    /// half of florui-config's own "window creation/update APIs" for
    /// per-window icon overrides (creation-time icon application lives in
    /// [`crate::desktop`], via the same [`to_winit_icon`]). No-op on
    /// macOS ("macOS doesn't have window icons", per `winit`'s own
    /// `Window::set_window_icon` doc) -- not gated here, since a silent
    /// platform no-op is exactly what the underlying `winit` call itself
    /// already does.
    pub fn set_icon(&self, icon: &florui_icon::RawIcon) -> Result<(), BadIcon> {
        let winit_icon = to_winit_icon(icon)?;
        self.window.set_window_icon(Some(winit_icon));
        Ok(())
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

    fn raw_icon(rgba: Vec<u8>, width: u32, height: u32) -> florui_icon::RawIcon {
        florui_icon::RawIcon {
            rgba,
            width,
            height,
        }
    }

    #[test]
    fn to_winit_icon_converts_a_well_formed_raw_icon() {
        let icon = raw_icon(vec![0; (2 * 2 * 4) as usize], 2, 2);
        assert!(to_winit_icon(&icon).is_ok());
    }

    #[test]
    fn to_winit_icon_reports_a_byte_count_not_divisible_by_4() {
        let icon = raw_icon(vec![0; 5], 1, 1);
        let err = to_winit_icon(&icon).unwrap_err();
        assert!(matches!(err, BadIcon::ByteCountNotDivisibleBy4 { .. }));
    }

    #[test]
    fn to_winit_icon_reports_a_dimension_pixel_count_mismatch() {
        // 4 pixels' worth of bytes, but dimensions claim only 1 pixel.
        let icon = raw_icon(vec![0; 4 * 4], 1, 1);
        let err = to_winit_icon(&icon).unwrap_err();
        assert!(matches!(err, BadIcon::DimensionsVsPixelCount { .. }));
    }

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

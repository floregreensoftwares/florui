//! The mechanism behind `InputMode::Selective` — see `overlay`'s own doc
//! for why a plain custom `WM_NCHITTEST` handler can't reach a different
//! process's window on its own. Real per-pixel cross-process
//! click-through only exists as `overlay::set_transparent`'s
//! all-or-nothing toggle; this module gets the *selective* behavior by
//! flipping that same toggle live, polling the real cursor position
//! against whichever regions are currently registered — the same
//! technique real click-through overlays (game overlays, on-screen
//! widgets) use, since Win32 has no native per-region click-through
//! primitive that also keeps painting the rest of the window normally.
//!
//! Polling (a hidden window's own `WM_TIMER`, always on this thread's
//! normal message loop) rather than a `WH_MOUSE_LL` hook: a window that
//! is currently transparent receives no mouse messages of its own by
//! design (that's the whole point of the toggle), so nothing would ever
//! observe the cursor moving back into an interactive region without an
//! independent, always-running check -- and a low-level hook is exactly
//! the kind of thing endpoint security software silently no-ops, which
//! would strand the window transparent with no way back.

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, HWND_MESSAGE, KillTimer,
    RegisterClassExW, SetTimer, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPED,
};

use super::overlay::set_transparent;
use crate::window_controls::ScreenRect;

const POLL_INTERVAL_MS: u32 = 16;
const POLL_TIMER_ID: usize = 1;
const CLASS_NAME: &str = "FloruiInputRegionPollWindow\0";

struct PollState {
    hwnd: HWND,
    poll_hwnd: HWND,
    regions: Vec<ScreenRect>,
    /// The window's own last-applied `WS_EX_TRANSPARENT` state, tracked
    /// so a tick only calls `SetWindowLongPtrW` when it actually needs
    /// to change, not on every poll.
    transparent: bool,
}

thread_local! {
    /// One entry -- `DesktopHost` only ever drives a single real window,
    /// so a thread-local singleton is simpler than threading a handle
    /// through the timer callback, which has no user-data slot of its
    /// own.
    static POLL: RefCell<Option<PollState>> = const { RefCell::new(None) };
}

/// Starts polling for `hwnd` if it isn't already running. Starts
/// pass-through (no regions registered yet, so nothing is interactive
/// until the next [`set_regions`] call) — never silently blocks input to
/// whatever's behind an unsynced window.
pub(crate) fn start(hwnd: HWND) {
    POLL.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(existing) = state.as_mut() {
            existing.hwnd = hwnd;
            return;
        }
        let Some(poll_hwnd) = create_poll_window() else {
            return;
        };
        // Safety: poll_hwnd was just created on this thread.
        unsafe { SetTimer(poll_hwnd, POLL_TIMER_ID, POLL_INTERVAL_MS, None) };
        set_transparent(hwnd, true);
        *state = Some(PollState {
            hwnd,
            poll_hwnd,
            regions: Vec::new(),
            transparent: true,
        });
    });
}

/// Stops polling for `hwnd` and restores normal (non-transparent) input
/// targeting — a no-op if `hwnd` isn't the one currently running.
pub(crate) fn stop(hwnd: HWND) {
    POLL.with(|state| {
        let mut state = state.borrow_mut();
        if state.as_ref().is_none_or(|s| s.hwnd != hwnd) {
            return;
        }
        if let Some(stopped) = state.take() {
            // Safety: stopped.poll_hwnd came from a successful start(),
            // never torn down since.
            unsafe {
                KillTimer(stopped.poll_hwnd, POLL_TIMER_ID);
                DestroyWindow(stopped.poll_hwnd);
            }
        }
        set_transparent(hwnd, false);
    });
}

/// Replaces the registered regions and immediately re-evaluates against
/// the real current cursor position — a static cursor already sitting
/// over a newly-registered region becomes interactive right away,
/// without waiting for the next poll tick.
pub(crate) fn set_regions(hwnd: HWND, regions: &[ScreenRect]) {
    POLL.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else { return };
        if state.hwnd != hwnd {
            return;
        }
        state.regions = regions.to_vec();
        apply_for_current_cursor(state);
    });
}

/// Reads the real cursor position and toggles `hwnd`'s transparency if
/// it's no longer where the last-applied state assumed. Cheap enough
/// (one `GetCursorPos` plus a handful of rectangle checks) to run every
/// tick; only calls `set_transparent` — a real `SetWindowLongPtrW`/
/// `SetLayeredWindowAttributes` pair that can block on the desktop
/// compositor — when the answer actually changed.
fn apply_for_current_cursor(state: &mut PollState) {
    let mut cursor = POINT { x: 0, y: 0 };
    // Safety: cursor is a valid out-pointer.
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return;
    }
    let inside = state
        .regions
        .iter()
        .any(|region| region.contains(cursor.x, cursor.y));
    let desired_transparent = !inside;
    if state.transparent != desired_transparent {
        state.transparent = desired_transparent;
        set_transparent(state.hwnd, desired_transparent);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_poll_window() -> Option<HWND> {
    // Safety: GetModuleHandleW(null) returns this process's own module
    // handle, a documented always-valid call.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class_name = wide(CLASS_NAME);

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(poll_wndproc),
        hInstance: instance as _,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    // Safety: class is fully initialized; a second registration of the
    // same name is a documented no-op failure, tolerated the same way
    // tray.rs's own window class registration is.
    unsafe { RegisterClassExW(&class) };

    // Safety: HWND_MESSAGE makes this a message-only window -- never
    // shown, no taskbar entry, exactly what a timer target needs.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance as _,
            std::ptr::null(),
        )
    };
    (!hwnd.is_null()).then_some(hwnd)
}

unsafe extern "system" fn poll_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TIMER {
        POLL.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                apply_for_current_cursor(state);
            }
        });
        return 0;
    }
    // Safety: standard default handling for anything this proc doesn't
    // itself own.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

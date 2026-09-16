//! `InputMode::Passthrough` (whole window) via `WS_EX_TRANSPARENT`, and
//! the same raw toggle `input_regions` reuses for
//! `InputMode::Selective`'s per-region version. A plain custom
//! `WM_NCHITTEST` handler can't do selective click-through on its own:
//! its own `HTTRANSPARENT` return value only forwards the hit test to
//! *other windows on the same thread* (see its own Win32 doc) — it can
//! never hand a click to a different process's window, which is the
//! whole point here. `input_regions`'s hook-driven live toggle is what
//! actually reaches across processes, the same way
//! `InputMode::Passthrough` already does.
//!
//! `WS_EX_TRANSPARENT` alone only reorders painting under DWM
//! composition; it does not skip hit-testing unless the window is also
//! `WS_EX_LAYERED`. `SetLayeredWindowAttributes` with full alpha keeps
//! the window fully visible and still GDI-painted (no
//! `UpdateLayeredWindow` involved), so this doesn't change how the
//! window renders -- only that DWM now treats it as pass-through.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW,
    WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::window_controls::{InputMode, ScreenRect};

pub(crate) fn hwnd_of(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(win32.hwnd.get() as HWND)
}

/// The raw toggle both [`set_input_mode`] and `input_regions`'s hook use
/// — no window recreation, takes effect immediately.
pub(crate) fn set_transparent(hwnd: HWND, enabled: bool) {
    // Safety: hwnd is live.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let flags = (WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize;
        let new_style = if enabled {
            style | flags
        } else {
            style & !flags
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
        if enabled {
            SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        }
    }
}

pub(crate) fn set_input_mode(window: &Window, mode: InputMode) -> bool {
    let Some(hwnd) = hwnd_of(window) else {
        return false;
    };
    match mode {
        InputMode::Normal => {
            super::input_regions::stop(hwnd);
            set_transparent(hwnd, false);
        }
        InputMode::Passthrough => {
            super::input_regions::stop(hwnd);
            set_transparent(hwnd, true);
        }
        InputMode::Selective => super::input_regions::start(hwnd),
    }
    true
}

pub(crate) fn sync_input_regions(window: &Window, regions: &[ScreenRect]) {
    let Some(hwnd) = hwnd_of(window) else {
        return;
    };
    super::input_regions::set_regions(hwnd, regions);
}

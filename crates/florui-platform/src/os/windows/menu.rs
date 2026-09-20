//! Native right-click context menus via `CreatePopupMenu`/
//! `TrackPopupMenuEx` — see `crate::menu`'s own doc for the public
//! contract. Built fresh from the caller's snapshot on every call, so
//! there is no persistent `HMENU` to mutate between calls and no need for
//! `EnableMenuItem`/`CheckMenuItem`/`ModifyMenuW`.
//!
//! `TPM_RETURNCMD` (see [`show`]) makes `TrackPopupMenuEx` return the
//! chosen command id directly, with no `WM_COMMAND` sent at all — this
//! deliberately avoids subclassing the real `winit`-owned window's own
//! `WndProc`, which no other module in this crate does either (`tray.rs`
//! and `input_regions.rs` both spin up their own message-only helper
//! window instead of touching the real one). Synchronous and blocking on
//! the calling (UI) thread, like `WindowControls::drag`'s own
//! `drag_window()` call -- confirmed against a real, human-driven menu
//! (show, select, dismiss, all while the window kept pumping its own
//! redraw requests normally) in a disposable scratch experiment first.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_TOPALIGN, TrackPopupMenuEx, WM_NULL,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::menu::{ContextMenuOutcome, MenuCommandId, MenuEntry, menu_label};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The real platform window handle backing `window`, if any -- `None`
/// (never a panic) when `winit` reports anything other than a real Win32
/// handle, matching `caption.rs`'s own extraction pattern.
fn raw_hwnd(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(win32.hwnd.get() as *mut c_void as HWND)
}

/// Owns the wide-string label buffers each `AppendMenuW` call points into,
/// so they stay valid through `TrackPopupMenuEx`, which can redraw the
/// popup at any time while it's open, not only during the append calls
/// themselves.
struct MenuLabels {
    _labels: Vec<Vec<u16>>,
}

/// Builds a real popup `HMENU` from `items`. `None` if `CreatePopupMenu`
/// itself fails. Returns the backing label buffers alongside the handle so
/// the caller can keep them alive for exactly as long as the menu is shown.
fn build_menu(items: &[MenuEntry]) -> Option<(HMENU, MenuLabels)> {
    // Safety: no arguments; CreatePopupMenu is always safe to call.
    let hmenu = unsafe { CreatePopupMenu() };
    if hmenu.is_null() {
        return None;
    }

    let mut labels = Vec::with_capacity(items.len());
    for item in items {
        match item {
            MenuEntry::Separator => {
                // Safety: hmenu was just created above; a separator entry
                // carries no string pointer.
                unsafe { AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null()) };
            }
            MenuEntry::Action {
                id,
                label,
                enabled,
                checked,
                shortcut,
            } => {
                let mut flags = MF_STRING;
                if !enabled {
                    flags |= MF_GRAYED;
                }
                if *checked {
                    flags |= MF_CHECKED;
                }
                let text = wide(&menu_label(label, shortcut.as_deref()));
                // Safety: hmenu was just created above; text outlives this
                // call (pushed into `labels`, which the caller keeps alive
                // for as long as the menu is shown).
                unsafe { AppendMenuW(hmenu, flags, id.0 as usize, text.as_ptr()) };
                labels.push(text);
            }
        }
    }

    Some((hmenu, MenuLabels { _labels: labels }))
}

pub(crate) fn show_context_menu(window: &Window, items: &[MenuEntry]) -> ContextMenuOutcome {
    let Some(hwnd) = raw_hwnd(window) else {
        return ContextMenuOutcome::Unavailable;
    };
    let Some((hmenu, _labels)) = build_menu(items) else {
        return ContextMenuOutcome::Unavailable;
    };

    let mut cursor = POINT { x: 0, y: 0 };
    // Safety: cursor is a valid out-pointer.
    unsafe { GetCursorPos(&mut cursor) };

    // Safety: hwnd is a real, live window handle. SetForegroundWindow
    // before TrackPopupMenuEx is an MSDN-documented requirement -- without
    // it, clicking outside the menu can fail to dismiss it correctly.
    unsafe { SetForegroundWindow(hwnd) };

    // Safety: hmenu and hwnd are both real and live; a null lptpm is
    // documented-valid and uses the default alignment/exclusion rect.
    let result = unsafe {
        TrackPopupMenuEx(
            hmenu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            cursor.x,
            cursor.y,
            hwnd,
            std::ptr::null(),
        )
    };
    // Safety: hmenu came from a successful build_menu above; a popup menu
    // is never destroyed automatically (unlike one attached via SetMenu),
    // so this call owns that cleanup.
    unsafe { DestroyMenu(hmenu) };
    // Safety: hwnd is real and live. The MSDN-documented follow-up nudge
    // after TrackPopupMenuEx, pairing with SetForegroundWindow above.
    unsafe { PostMessageW(hwnd, WM_NULL, 0, 0) };

    match result as u32 {
        0 => ContextMenuOutcome::Dismissed,
        id => ContextMenuOutcome::Selected(MenuCommandId(id)),
    }
}

//! Recoloring the *real, OS-drawn* title bar instead of replacing it — a
//! lighter, separate capability from
//! [`crate::window_controls`]/[`crate::appearance::DecorationMode::Custom`]:
//! the system still draws and owns the caption (buttons, drag, snap, the
//! system menu, all still real system chrome), only its background and
//! text/button-glyph color change. On Windows this is
//! `DwmSetWindowAttribute` with `DWMWA_CAPTION_COLOR`/`DWMWA_TEXT_COLOR` —
//! real evidence gathered via the `system_caption_override_probe` example
//! (see its own doc): both DWM calls returned `S_OK` and the real title
//! bar visibly changed color. `winit` has no caption-color API at all, so
//! this module goes straight to the raw HWND, the same way that probe
//! does.
//!
//! `DWMWA_CAPTION_COLOR`/`DWMWA_TEXT_COLOR` only exist from Windows 11
//! onward (build 22000+); [`override_caption_colors`] reports whether both
//! calls actually succeeded rather than assuming so just because it
//! compiled and didn't panic — the same honesty
//! [`crate::appearance::CapabilityStatus`] exists to enforce elsewhere in
//! this crate. On any other platform this function is a real, harmless
//! no-op that always reports `false` — there is no equivalent OS API to
//! call, not a Windows-specific limitation of this crate's own
//! implementation.

use florui_style::Rgba;
use winit::window::Window;

/// Recolors `window`'s real, OS-drawn title bar background (`caption`) and
/// text/button-glyph color (`text`) — both fully opaque colors; `Rgba`'s
/// own alpha channel is ignored, the same as `DWMWA_CAPTION_COLOR`'s own
/// `COLORREF` has no alpha slot. Returns whether the OS actually confirmed
/// both changes (`S_OK` from both `DwmSetWindowAttribute` calls on
/// Windows; always `false` elsewhere) — not a promise the caller can
/// safely ignore, since a pre-Windows-11-22000 host silently leaves the
/// title bar unchanged.
#[cfg(target_os = "windows")]
pub fn override_caption_colors(window: &Window, caption: Rgba, text: Rgba) -> bool {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    fn colorref(color: Rgba) -> u32 {
        u32::from_le_bytes([color.r, color.g, color.b, 0])
    }

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return false;
    };
    let hwnd = win32.hwnd.get() as HWND;
    let caption_color = colorref(caption);
    let text_color = colorref(text);

    // Safety: `hwnd` came from `window`'s own live `HasWindowHandle`, and
    // both pointers passed point at locals that outlive the call.
    let caption_result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            (&raw const caption_color).cast(),
            size_of::<u32>() as u32,
        )
    };
    let text_result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR as u32,
            (&raw const text_color).cast(),
            size_of::<u32>() as u32,
        )
    };
    caption_result == 0 && text_result == 0
}

#[cfg(not(target_os = "windows"))]
pub fn override_caption_colors(_window: &Window, _caption: Rgba, _text: Rgba) -> bool {
    false
}

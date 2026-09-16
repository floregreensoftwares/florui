//! Recolors the real, OS-drawn title bar via `DwmSetWindowAttribute`
//! (`DWMWA_CAPTION_COLOR`/`DWMWA_TEXT_COLOR`) — `winit` has no
//! caption-color API, so this goes straight to the raw HWND. Both
//! attributes only exist from Windows 11 (build 22000+); see
//! [`override_caption_colors`]'s own doc for what a `false` result means.

use florui_style::Rgba;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// `Rgba`'s alpha is ignored — `COLORREF` has no alpha slot. Returns
/// whether both `DwmSetWindowAttribute` calls returned `S_OK`; a
/// pre-Windows-11-22000 host returns `false` and leaves the title bar
/// unchanged.
pub fn override_caption_colors(window: &Window, caption: Rgba, text: Rgba) -> bool {
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

    // Safety: hwnd is live (from window's own HasWindowHandle); both
    // pointers point at locals outliving the call.
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

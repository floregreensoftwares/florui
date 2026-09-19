//! Reads the real "Show animations in Windows" accessibility setting
//! (Settings > Accessibility > Visual effects, or the classic Ease of
//! Access Center) via `SystemParametersInfoW`/`SPI_GETCLIENTAREAANIMATION`
//! — the same signal Chromium/Firefox read on Windows to resolve
//! `prefers-reduced-motion`. No window handle needed: this is a global
//! system setting, not a per-window one.

use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
};
use windows_sys::core::BOOL;

/// `false` on failure (mirroring this crate's other OS-capability
/// wrappers, which report an inert default rather than an error) and
/// whenever `SPI_GETCLIENTAREAANIMATION` itself reports animations
/// enabled — i.e. no reduced-motion preference.
pub fn prefers_reduced_motion() -> bool {
    let mut client_area_animation_enabled: BOOL = 1;
    // Safety: `client_area_animation_enabled` is a local outliving the
    // call, and `SPI_GETCLIENTAREAANIMATION` writes exactly one `BOOL`
    // through the pointer it's given.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            &mut client_area_animation_enabled as *mut BOOL as *mut core::ffi::c_void,
            0,
        )
    };
    ok != 0 && client_area_animation_enabled == 0
}

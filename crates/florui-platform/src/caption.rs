//! Recolor the real system title bar — see [`crate::os::windows::caption`]
//! for the real implementation. A no-op returning `false` on any other
//! platform: no equivalent OS API exists yet.

#[cfg(target_os = "windows")]
pub use crate::os::windows::caption::override_caption_colors;

#[cfg(not(target_os = "windows"))]
pub fn override_caption_colors(
    _window: &winit::window::Window,
    _caption: florui_style::Rgba,
    _text: florui_style::Rgba,
) -> bool {
    false
}

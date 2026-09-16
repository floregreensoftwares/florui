//! Whole-window and selective click-through — see
//! [`crate::os::windows::overlay`] for the real `InputMode::Passthrough`
//! implementation and `crate::os::windows::input_regions` for
//! `InputMode::Selective`'s hook-driven version. Both are no-ops
//! elsewhere.

use crate::window_controls::{InputMode, ScreenRect};

#[cfg(target_os = "windows")]
pub(crate) fn set_input_mode(window: &winit::window::Window, mode: InputMode) -> bool {
    crate::os::windows::overlay::set_input_mode(window, mode)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_input_mode(_window: &winit::window::Window, _mode: InputMode) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn sync_input_regions(window: &winit::window::Window, regions: &[ScreenRect]) {
    crate::os::windows::overlay::sync_input_regions(window, regions);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn sync_input_regions(_window: &winit::window::Window, _regions: &[ScreenRect]) {}

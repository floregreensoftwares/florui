//! Native drag-and-drop for files/folders dropped onto a window from the
//! OS shell — see [`crate::os::windows::drag_drop`] for the real
//! implementation. `register` is a no-op returning `None` on any other
//! platform: no equivalent mechanism exists yet.
//!
//! `winit`'s own built-in `WindowEvent::DroppedFile`/`HoveredFile` is
//! deliberately not used here: reading `winit`'s own Windows backend
//! confirms it captures the drop's cursor position internally and never
//! surfaces it, and it hardcodes always-accept behavior with no way for
//! an app to reject a hovering payload. Apps never call [`register`]
//! directly -- see [`crate::WindowControls::set_drag_accept_policy`]/
//! [`crate::WindowControls::on_drag_event`] for the public entry point.

use std::path::PathBuf;

/// Where a drag event is happening, in the same logical (DPI-independent)
/// units layout runs against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragPosition {
    pub x: f32,
    pub y: f32,
}

/// What's being dragged. `Files` (which also covers folders -- Windows
/// delivers both through the same `CF_HDROP` mechanism) is the only kind
/// implemented today -- room to grow, matching
/// [`crate::ActivationEvent`]'s own shape, without pre-building payload
/// kinds nothing yet needs.
#[derive(Debug, Clone, PartialEq)]
pub enum DragPayload {
    Files(Vec<PathBuf>),
}

/// One step in a drag gesture's lifecycle, delivered to whatever
/// [`crate::WindowControls::on_drag_event`] registered. `Enter`/`Over`
/// fire for every recognized [`DragPayload`], whether or not
/// [`crate::WindowControls::set_drag_accept_policy`] accepted it --
/// `accepted` says which, so an app can render its own reject feedback
/// (the OS's own drop cursor already reflects it too, independently).
/// An unsupported payload (not yet a [`DragPayload`] kind, e.g. dragged
/// text with no file backing it) is never delivered at all. `Drop` only
/// ever fires for a payload that was accepted -- an unaccepted hover
/// cannot be completed, so there is nothing to drop.
#[derive(Debug, Clone, PartialEq)]
pub enum DragEvent {
    Enter {
        payload: DragPayload,
        position: DragPosition,
        accepted: bool,
    },
    Over {
        position: DragPosition,
        accepted: bool,
    },
    Leave,
    Drop {
        payload: DragPayload,
        position: DragPosition,
    },
}

/// A window can only ever have one registered drop target -- called on
/// every window's own [`winit::window::WindowAttributes`] before creation
/// so `winit`'s own built-in handling never registers first and blocks
/// [`register`]'s own `RegisterDragDrop` call. A no-op on any platform
/// where `winit` has no such built-in registration to disable.
pub(crate) fn disable_builtin_drag_and_drop(
    attrs: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    #[cfg(target_os = "windows")]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs.with_drag_and_drop(false)
    };
    attrs
}

#[cfg(target_os = "windows")]
pub(crate) use crate::os::windows::drag_drop::{DragDropRegistration, register};

#[cfg(not(target_os = "windows"))]
pub(crate) use stub::{DragDropRegistration, register};

#[cfg(not(target_os = "windows"))]
mod stub {
    use std::rc::Rc;
    use std::sync::Arc;

    use winit::window::Window;

    use crate::WindowControls;

    pub(crate) struct DragDropRegistration;

    pub(crate) fn register(
        _window: &Arc<Window>,
        _controls: Rc<WindowControls>,
    ) -> Option<DragDropRegistration> {
        None
    }
}

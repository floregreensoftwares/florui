//! A real `IDropTarget`, registered directly on the window's own `HWND` —
//! see `crate::drag_drop`'s own doc for why `winit`'s built-in
//! `WindowEvent::DroppedFile`/`HoveredFile` isn't used instead: reading
//! `winit`'s own Windows backend confirms it captures the drop's cursor
//! position internally and never surfaces it, and hardcodes
//! `DROPEFFECT_COPY` unconditionally. Uses the `windows` crate (not this
//! crate's usual `windows-sys`) via `#[implement(IDropTarget)]`, the same
//! call already made in `file_dialog.rs` for COM interface support
//! `windows-sys` doesn't have. Every callback here runs on this window's
//! own UI thread — OLE drives `IDropTarget` synchronously from its own
//! message pump during a drag gesture, not from a separate thread the
//! way `single_instance.rs`'s named-pipe listener is, so results reach
//! `WindowControls` directly with no `EventLoopProxy` hand-off needed.
//!
//! Confirmed against a real, human-driven drag from Explorer in a
//! disposable scratch experiment first: real screen coordinates,
//! `ScreenToClient` conversion, a multi-file payload delivered once (not
//! once per intermediate event), and clean `RevokeDragDrop` on close.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    OleInitialize, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows::core::implement;
use winit::window::Window;

use crate::WindowControls;
use crate::drag_drop::{DragEvent, DragPayload, DragPosition};
use crate::os::windows::raw_hwnd;

/// Pure conversion from a client-area (physical) point to the logical
/// units layout runs against — the same `(x / scale_factor)` formula
/// `WindowState::to_logical_cursor` uses for ordinary mouse input,
/// duplicated here since that method is private to `desktop.rs` and this
/// module has no other way to reach it.
pub(crate) fn screen_to_logical(client_point: (i32, i32), scale_factor: f64) -> DragPosition {
    DragPosition {
        x: (client_point.0 as f64 / scale_factor) as f32,
        y: (client_point.1 as f64 / scale_factor) as f32,
    }
}

/// Extracts every filename from a `CF_HDROP`-bearing `IDataObject`, or an
/// empty list if this payload isn't a file drop (e.g. dragged text with
/// no file backing it) — not yet a supported [`DragPayload`] kind, so
/// callers treat an empty result as "reject, don't dispatch."
fn hdrop_paths(data_object: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // Safety: format describes CF_HDROP/TYMED_HGLOBAL, the standard shape
    // OLE uses for dropped-file lists; data_object is the live object OLE
    // handed to this callback for the duration of the call.
    let Ok(medium) = (unsafe { data_object.GetData(&format) }) else {
        return Vec::new();
    };
    // Safety: a successful GetData for TYMED_HGLOBAL guarantees hGlobal
    // is a valid HDROP handle for the lifetime of `medium`.
    let hdrop = HDROP(unsafe { medium.u.hGlobal.0 });
    // Safety: hdrop was just obtained above from a live STGMEDIUM.
    let count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // Safety: hdrop is still valid; querying the length first with no
        // buffer is `DragQueryFileW`'s own documented contract.
        let len = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        let mut buf = vec![0u16; len + 1];
        // Safety: buf is sized len+1 for the null terminator, matching
        // the length just queried for this same index.
        unsafe { DragQueryFileW(hdrop, index, Some(&mut buf)) };
        paths.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
    }
    // Safety: hdrop came from the successful GetData call above;
    // DragFinish releases it exactly once, after every DragQueryFileW
    // call for it is done.
    unsafe { DragFinish(hdrop) };
    paths
}

fn to_drag_position(hwnd: HWND, window: &Window, pt: &POINTL) -> DragPosition {
    let mut point = POINT { x: pt.x, y: pt.y };
    // Safety: hwnd is the live window this target is registered on; point
    // is a valid, writable POINT for the duration of this call.
    let client = if unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
        (point.x, point.y)
    } else {
        (pt.x, pt.y)
    };
    screen_to_logical(client, window.scale_factor())
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
    window: Arc<Window>,
    controls: Rc<WindowControls>,
    /// Parsed once on `DragEnter`, reused by every following `DragOver`
    /// so a hover doesn't re-walk `IDataObject` on every mouse move —
    /// cleared on `DragLeave`/`Drop`. The `bool` is whether the accept
    /// policy accepted this payload; `Drop` only ever fires for one that
    /// was, so caching it here (rather than re-evaluating a possibly
    /// stateful policy again at `Drop` time) is also what makes that
    /// guarantee actually hold.
    cached_payload: std::cell::RefCell<Option<(DragPayload, bool)>>,
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let position = to_drag_position(self.hwnd, &self.window, pt);
        let payload = pdataobj.as_ref().and_then(|data| {
            let paths = hdrop_paths(data);
            (!paths.is_empty()).then_some(DragPayload::Files(paths))
        });
        let accepted = payload
            .as_ref()
            .is_some_and(|payload| self.controls.evaluate_drag_accept(payload));
        // Safety: pdweffect is a valid out-pointer for the duration of
        // this OLE callback, per IDropTarget's own documented contract.
        unsafe {
            *pdweffect = if accepted {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
        if let Some(payload) = payload {
            self.controls.dispatch_drag_event(DragEvent::Enter {
                payload: payload.clone(),
                position,
                accepted,
            });
            *self.cached_payload.borrow_mut() = Some((payload, accepted));
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let cached = self.cached_payload.borrow();
        let Some((_, accepted)) = cached.as_ref() else {
            // Safety: matches DragEnter's own contract.
            unsafe {
                *pdweffect = DROPEFFECT_NONE;
            }
            return Ok(());
        };
        let accepted = *accepted;
        drop(cached);
        // Safety: matches DragEnter's own contract.
        unsafe {
            *pdweffect = if accepted {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
        let position = to_drag_position(self.hwnd, &self.window, pt);
        self.controls
            .dispatch_drag_event(DragEvent::Over { position, accepted });
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.cached_payload.borrow_mut().take().is_some() {
            self.controls.dispatch_drag_event(DragEvent::Leave);
        }
        Ok(())
    }

    fn Drop(
        &self,
        _pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        _pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if let Some((payload, accepted)) = self.cached_payload.borrow_mut().take()
            && accepted
        {
            let position = to_drag_position(self.hwnd, &self.window, pt);
            self.controls
                .dispatch_drag_event(DragEvent::Drop { payload, position });
        }
        Ok(())
    }
}

/// Revokes the drop target on drop -- holds only the raw `HWND` value
/// (not a live `Window` reference), so it stays safe to run this even
/// after the `winit::window::Window` it was registered against has
/// otherwise started tearing down, as long as the OS handle itself is
/// still valid.
pub(crate) struct DragDropRegistration(HWND);

impl Drop for DragDropRegistration {
    fn drop(&mut self) {
        // Safety: self.0 was registered by `register` below and never
        // revoked before now.
        let _ = unsafe { RevokeDragDrop(self.0) };
    }
}

/// Registers a real `IDropTarget` for `window`, replacing whatever
/// `winit` would otherwise have registered itself (see
/// `crate::desktop`'s own `with_drag_and_drop(false)` call — a window can
/// only ever have one registered drop target). `None` on any failure (no
/// real Win32 handle, `OleInitialize` failure, or `RegisterDragDrop`
/// failure) — never a panic, matching `file_dialog.rs`'s own fallback
/// idiom.
pub(crate) fn register(
    window: &Arc<Window>,
    controls: Rc<WindowControls>,
) -> Option<DragDropRegistration> {
    let hwnd = raw_hwnd(window)?;

    static OLE_INIT_OK: OnceLock<bool> = OnceLock::new();
    let ole_ok = *OLE_INIT_OK.get_or_init(|| {
        // Safety: `register` only ever runs on this process's single UI
        // thread (called from `DesktopHost::resumed`), and `get_or_init`
        // runs this closure at most once -- before any RegisterDragDrop
        // call below.
        unsafe { OleInitialize(None) }.is_ok()
    });
    if !ole_ok {
        eprintln!("florui-platform: OleInitialize failed, drag-and-drop is unavailable");
        return None;
    }

    let target: IDropTarget = DropTarget {
        hwnd,
        window: Arc::clone(window),
        controls,
        cached_payload: std::cell::RefCell::new(None),
    }
    .into();
    // Safety: hwnd is a real, live window just created by the caller;
    // target is a valid IDropTarget this call keeps alive for as long as
    // OLE holds a reference to it (until RevokeDragDrop below).
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => Some(DragDropRegistration(hwnd)),
        Err(error) => {
            eprintln!("florui-platform: RegisterDragDrop failed: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_to_logical_at_1x_is_the_identity() {
        let position = screen_to_logical((100, 50), 1.0);
        assert_eq!(position, DragPosition { x: 100.0, y: 50.0 });
    }

    #[test]
    fn screen_to_logical_at_2x_halves_the_client_point() {
        let position = screen_to_logical((100, 50), 2.0);
        assert_eq!(position, DragPosition { x: 50.0, y: 25.0 });
    }

    #[test]
    fn screen_to_logical_preserves_negative_coordinates() {
        let position = screen_to_logical((-10, -4), 1.0);
        assert_eq!(position, DragPosition { x: -10.0, y: -4.0 });
    }
}

//! A real system tray icon via `Shell_NotifyIconW`, with real click
//! events: owns a hidden message-only window (not a `winit` one — it
//! needs no graphics, just a `WNDPROC`) that Windows posts the tray
//! callback message to. That window's own thread message loop is
//! `winit`'s event loop itself, since both run on the same thread —
//! Win32 dispatches to whichever window a message targets regardless of
//! which library created it, so no extra pumping is needed.

use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HICON,
    HWND_MESSAGE, IDI_APPLICATION, LoadIconW, RegisterClassExW, SetWindowLongPtrW, WM_APP,
    WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSEXW, WS_OVERLAPPED,
};

const TRAY_CALLBACK: u32 = WM_APP + 1;
const CLASS_NAME: &str = "FloruiTrayIconWindow\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    LeftClick,
    RightClick,
}

/// A live tray icon; removed and its hidden window destroyed on drop.
pub struct TrayIcon {
    hwnd: HWND,
    id: u32,
}

// `HWND` is a raw pointer, so this is already `!Send`/`!Sync` for free —
// same as `winit::window::Window`. Only ever touch it from the thread
// that created it.
impl TrayIcon {
    /// `tooltip` is capped at 127 UTF-16 units (`NOTIFYICONDATAW`'s own
    /// `szTip` size); longer text is truncated. `on_event` fires on a
    /// real left/right click, called from `winit`'s own event loop
    /// thread. `None` if window-class registration or `Shell_NotifyIconW`
    /// itself fails.
    pub fn new(tooltip: &str, on_event: impl Fn(TrayEvent) + 'static) -> Option<Self> {
        static NEXT_ID: AtomicU32 = AtomicU32::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        let hwnd = create_message_window()?;
        let callback: Box<Box<dyn Fn(TrayEvent)>> = Box::new(Box::new(on_event));
        // Safety: hwnd was just created on this thread and isn't shared
        // yet, so no other thread can race this write.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(callback) as isize);
        }

        let mut data = notify_icon_data(hwnd, id);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY_CALLBACK;
        // Safety: a null hInstance requests the system's own default
        // application icon, a documented `LoadIconW` behavior.
        data.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) as HICON };
        set_tip(&mut data, tooltip);

        // Safety: data is a fully initialized NOTIFYICONDATAW.
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
        if added == 0 {
            // Safety: hwnd was created here and owned by no one else yet.
            unsafe {
                drop_userdata(hwnd);
                let _ = DestroyWindow(hwnd);
            }
            return None;
        }

        Some(Self { hwnd, id })
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let data = notify_icon_data(self.hwnd, self.id);
        // Safety: data identifies this instance's own icon.
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
            drop_userdata(self.hwnd);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn notify_icon_data(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    // Safety: NOTIFYICONDATAW is a plain-old-data struct; zeroing is a
    // valid initial value for every field this function doesn't set.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = id;
    data
}

fn set_tip(data: &mut NOTIFYICONDATAW, tooltip: &str) {
    let utf16: Vec<u16> = tooltip.encode_utf16().collect();
    let len = utf16.len().min(data.szTip.len() - 1);
    data.szTip[..len].copy_from_slice(&utf16[..len]);
    data.szTip[len] = 0;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_message_window() -> Option<HWND> {
    // Safety: GetModuleHandleW(null) returns this process's own module
    // handle, a documented always-valid call.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class_name = wide(CLASS_NAME);

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: instance as _,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    // Safety: class is a fully initialized WNDCLASSEXW; registering the
    // same class name twice (a second TrayIcon in this process) is a
    // documented no-op failure we tolerate by ignoring the result --
    // CreateWindowExW below still works against the first registration.
    unsafe { RegisterClassExW(&class) };

    // Safety: HWND_MESSAGE makes this a message-only window -- never
    // shown, no taskbar entry, exactly what a tray icon's callback
    // target needs.
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

/// Safety: only called on this window's own creating thread, on a hwnd
/// this module created and still owns.
unsafe fn drop_userdata(hwnd: HWND) {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Box<dyn Fn(TrayEvent)>;
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == TRAY_CALLBACK {
        // Safety: userdata was set in `TrayIcon::new` on this same
        // thread before this window could receive any message.
        let ptr =
            unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Box<dyn Fn(TrayEvent)>;
        if !ptr.is_null() {
            let event = match lparam as u32 {
                WM_LBUTTONUP => Some(TrayEvent::LeftClick),
                WM_RBUTTONUP => Some(TrayEvent::RightClick),
                _ => None,
            };
            if let Some(event) = event {
                // Safety: ptr is valid until Drop runs (which happens on
                // this same thread, never concurrently with this call).
                unsafe { (*ptr)(event) };
            }
        }
        return 0;
    }
    if msg == WM_DESTROY {
        return 0;
    }
    // Safety: standard default handling for anything this proc doesn't
    // itself own.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

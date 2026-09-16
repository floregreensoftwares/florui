//! A system tray icon — see [`crate::os::windows::tray`] for the real
//! implementation. `TrayIcon::new` returns `None` on any other platform.

#[cfg(target_os = "windows")]
pub use crate::os::windows::tray::{TrayEvent, TrayIcon};

#[cfg(not(target_os = "windows"))]
pub use stub::{TrayEvent, TrayIcon};

#[cfg(not(target_os = "windows"))]
mod stub {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TrayEvent {
        LeftClick,
        RightClick,
    }

    pub struct TrayIcon(());

    impl TrayIcon {
        pub fn new(_tooltip: &str, _on_event: impl Fn(TrayEvent) + 'static) -> Option<Self> {
            None
        }
    }
}

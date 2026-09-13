//! [`UiRuntime`]: the window-independent half of a running Florui tree —
//! rendering, hit testing, and click dispatch, with no dependency on any
//! particular window system. The `desktop` feature (on by default) adds
//! [`run`], a real `winit`/`softbuffer` desktop host built on top of it,
//! so a caller doesn't have to write its own desktop event loop just to
//! see a component tree running; a mobile or game host that wants
//! [`UiRuntime`] alone, without pulling in `winit`/`softbuffer` at all,
//! can disable it (`default-features = false`).

mod runtime;

pub use runtime::UiRuntime;

#[cfg(feature = "desktop")]
mod desktop;

#[cfg(feature = "desktop")]
pub use desktop::{RunError, run};

#[cfg(feature = "desktop")]
pub mod dpi;

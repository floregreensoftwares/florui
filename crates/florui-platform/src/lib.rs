//! [`UiRuntime`]: the window-independent half of a running Florui tree —
//! rendering, hit testing, and click dispatch, with no dependency on any
//! particular window system. The `desktop` feature (on by default) adds
//! [`run`], a real `winit`/`softbuffer` desktop host built on top of it,
//! so a caller doesn't have to write its own desktop event loop just to
//! see a component tree running, and [`run_with_css_reload`], the same
//! host watching its stylesheet on disk so an edit reaches the window
//! without resetting any component state; a mobile or game host that
//! wants [`UiRuntime`] alone, without pulling in `winit`/`softbuffer`/
//! `notify` at all, can disable it (`default-features = false`).

mod runtime;
mod size_observer;

pub use runtime::UiRuntime;
pub use size_observer::{SizeObserverRegistry, use_committed_size};

#[cfg(feature = "desktop")]
mod desktop;

#[cfg(feature = "desktop")]
pub use desktop::{RunError, run, run_with_css_reload};

#[cfg(feature = "desktop")]
pub mod dpi;

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
pub use desktop::{
    RunError, RunOutcome, WindowOptions, WindowSpec, run, run_single_instance, run_windows,
    run_with_css_reload, run_with_css_reload_and_options, run_with_options,
};

#[cfg(feature = "desktop")]
mod activation;

#[cfg(feature = "desktop")]
pub use activation::{
    ActivationEvent, ActivationEvents, SingleInstance, classify_launch,
    probe_single_instance_capability, use_activation_events,
};

#[cfg(feature = "desktop")]
mod single_instance;

#[cfg(feature = "desktop")]
mod file_dialog;

#[cfg(feature = "desktop")]
pub use file_dialog::{
    FileDialogFilter, OpenFileDialogOptions, OpenFileDialogOutcome, SaveFileDialogOptions,
    SaveFileDialogOutcome,
};

#[cfg(feature = "desktop")]
pub mod accessibility;

#[cfg(feature = "desktop")]
pub mod appearance;

#[cfg(feature = "desktop")]
pub mod caption;

#[cfg(feature = "desktop")]
pub mod dpi;

#[cfg(feature = "desktop")]
pub mod gpu;

#[cfg(feature = "desktop")]
mod os;

#[cfg(feature = "desktop")]
pub mod overlay;

#[cfg(feature = "desktop")]
pub mod theme;

#[cfg(feature = "desktop")]
pub mod tray;

#[cfg(feature = "desktop")]
mod window_controls;

#[cfg(feature = "desktop")]
mod window_state;

#[cfg(feature = "desktop")]
pub use window_state::{WindowPersistence, probe_persistence_capability, reset_window_state};

#[cfg(feature = "desktop")]
pub use window_controls::{
    InputMode, WINDOW_DRAG_REGION_ID, WINDOW_INPUT_REGION_CLASS, WindowControls,
    use_window_controls,
};

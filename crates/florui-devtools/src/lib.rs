//! Development-only tooling for Florui: a minimal native preview host, CSS
//! hot reload, structured diagnostics, offscreen capture, and — via
//! [`live`] — a real `florui-style`/`florui-layout`-backed preview paired
//! with the [`inspector`].
//!
//! This crate is a *host*, not the core: it owns the window and event loop
//! the way a desktop shell would. [`element_scene`]/[`scene`] are an older
//! stand-in reading a literal inline `style=` attribute with no real
//! layout; [`preview`] watches a raw CSS-only fixture with no real
//! component tree; [`live`] is the real-tree counterpart of both.

pub mod capture;
pub mod color;
pub mod diagnostics;
pub mod element_preview;
pub mod element_scene;
pub mod fixture;
pub mod inspector;
pub mod live;
pub mod preview;
pub mod scene;

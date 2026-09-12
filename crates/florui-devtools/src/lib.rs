//! Development-only tooling for Florui: a minimal native preview host, CSS
//! hot reload, structured diagnostics, and offscreen capture. None of this
//! is the real style/layout/paint engine; it exists to validate the
//! edit-and-see loop before that engine exists.
//!
//! This crate is a *host*, not the core: it owns the window and event loop
//! the way a desktop shell would. When the core engine crate exists, it must
//! stay usable without this crate — window ownership, event-loop control,
//! and swapchain presentation belong here or in whatever embeds the engine
//! instead, never baked into mounting or hook scheduling.

pub mod capture;
pub mod color;
pub mod diagnostics;
pub mod fixture;
pub mod inspector;
pub mod preview;
pub mod scene;

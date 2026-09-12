//! Development-only tooling for Florui: a minimal native preview host, CSS
//! hot reload, structured diagnostics, and offscreen capture. None of this
//! is the real style/layout/paint engine; it exists to validate the
//! edit-and-see loop before that engine exists.

pub mod capture;
pub mod color;
pub mod diagnostics;
pub mod fixture;
pub mod inspector;
pub mod preview;
pub mod scene;

//! Development-only tooling for Florui: a minimal native preview host, CSS
//! hot reload, structured diagnostics, and offscreen capture.
//!
//! This crate is a *host*, not the core: it owns the window and event loop
//! the way a desktop shell would. Real style ([`florui_style`]) and layout
//! ([`florui_layout`]) now exist as separate crates this one bridges into
//! pixels via [`layout_capture`]; [`element_scene`] and [`scene`] remain
//! the older fixed-margin stand-in from before either existed, kept for
//! what still only reads a literal inline `style=` attribute with no
//! layout at all.

pub mod capture;
pub mod color;
pub mod diagnostics;
pub mod element_preview;
pub mod element_scene;
pub mod fixture;
pub mod inspector;
pub mod layout_capture;
pub mod preview;
pub mod scene;

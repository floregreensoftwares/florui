//! Development-only tooling for Florui: a minimal native preview host, CSS
//! hot reload, structured diagnostics, and offscreen capture.
//!
//! This crate is a *host*, not the core: it owns the window and event loop
//! the way a desktop shell would. Real style ([`florui_style`]), layout
//! ([`florui_layout`]), and painting ([`florui_paint`]) now exist as
//! separate crates this one bridges into a window; [`element_scene`] and
//! [`scene`] remain the older fixed-margin stand-in from before any of
//! them existed, kept for what still only reads a literal inline `style=`
//! attribute with no real layout at all.

pub mod capture;
pub mod color;
pub mod diagnostics;
pub mod element_preview;
pub mod element_scene;
pub mod fixture;
pub mod inspector;
pub mod preview;
pub mod scene;

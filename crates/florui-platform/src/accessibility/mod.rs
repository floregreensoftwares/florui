//! Everything accessibility: real OS preference reads (this file) and the
//! real AccessKit tree bridge ([`tree`], desktop-only).

#[cfg(target_os = "windows")]
pub use crate::os::windows::accessibility::prefers_reduced_motion;

#[cfg(not(target_os = "windows"))]
pub fn prefers_reduced_motion() -> bool {
    false
}

#[cfg(feature = "desktop")]
pub(crate) mod tree;

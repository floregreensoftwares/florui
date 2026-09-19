//! Reads real OS accessibility preferences — see
//! [`crate::os::windows::accessibility`] for the real implementation. A
//! no-op returning `false` (no preference) on any other platform: no
//! equivalent OS API exists yet.

#[cfg(target_os = "windows")]
pub use crate::os::windows::accessibility::prefers_reduced_motion;

#[cfg(not(target_os = "windows"))]
pub fn prefers_reduced_motion() -> bool {
    false
}

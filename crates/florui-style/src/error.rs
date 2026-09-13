//! [`StyleError`] exists for its `Result<_, StyleError>` signature's sake.
//! Real CSS parsing (now Stylo's, not a hand-rolled subset) recovers from
//! a malformed rule or declaration by skipping it, the same way a browser
//! does — it does not reject a whole stylesheet the way this crate's
//! previous closed-subset parser did. There is currently no way to
//! actually construct one.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleError {}

impl fmt::Display for StyleError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl std::error::Error for StyleError {}

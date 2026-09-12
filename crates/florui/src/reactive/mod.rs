//! Local component state: `use_signal`, `use_effect`, `use_ref`, and the
//! minimal mount/rerender driver that makes them real.
//!
//! **Scope**: this proves the hook mechanism itself — persistent
//! per-instance storage, call-order/count validation across renders, and
//! effect scheduling with cleanup — against a single manually mounted root
//! component (see [`mount`]). It is not wired into `view!`: a
//! `#[component]` called as a nested tag (`<Card>`) is still a plain
//! function call with no persistent instance, so nested components get no
//! hook state yet. Automatic per-instance mounting across a tree needs a
//! real reconciler that matches components across renders by position and
//! key — that does not exist. `use_memo` and `use_context` are not
//! implemented either: memoization needs a general dependency-equality
//! scheme and context needs a provider tree, neither of which exists
//! without that reconciler.

mod effect;
mod instance;
mod mount;
mod ref_hook;
mod signal;

pub use effect::use_effect;
pub use mount::{Mounted, mount};
pub use ref_hook::{RefHandle, use_ref};
pub use signal::{Signal, use_signal};

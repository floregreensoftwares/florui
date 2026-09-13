//! Persistent local component state: [`use_signal`], [`use_memo`],
//! [`use_context`], [`use_ref`], [`use_effect`], [`use_child_scope`], and
//! the [`Scope`] that gives them somewhere stable to live across repeated
//! renders of the same tree.
//!
//! # Scope
//!
//! Not built yet: keyed identity (so a list can reorder without losing
//! each item's own state), and automatic dependency tracking for
//! `use_memo`/`use_effect` (deps are compared by equality, not inferred).
//! [`use_child_scope`] gives per-call-site nesting, but nothing outside
//! this crate creates one automatically yet — `#[component]` still shares
//! its caller's scope rather than getting its own. One `Scope` has one
//! flat, call-ordered slot list, so every order-sensitive hook call in the
//! tree it renders must run in the same order and count every time —
//! `use_context` is the exception, since a provided value is looked up by
//! type, not call position. No event wiring in `view!` yet either —
//! [`Scope`] just exposes a dirty flag for a host to poll.

mod context;
mod effect;
mod memo;
mod refs;
mod scope;
mod signal;

pub use context::{provide_context, use_context};
pub use effect::{Cleanup, use_effect};
pub use memo::use_memo;
pub use refs::{Ref, use_ref};
pub use scope::{Scope, render_once, use_child_scope};
pub use signal::{Signal, use_signal};

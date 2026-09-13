//! Persistent local component state: [`use_signal`], [`use_memo`],
//! [`use_context`], and the [`Scope`] that gives them somewhere stable to
//! live across repeated renders of the same tree.
//!
//! # Scope
//!
//! Not built yet: `use_effect`, `use_ref`, per-component identity (needed
//! for keyed/conditional mounting), disposal, and automatic dependency
//! tracking for `use_memo` (deps are compared by equality, not inferred).
//! One `Scope` has one flat, call-ordered slot list, so every
//! order-sensitive hook call in the tree it renders must run in the same
//! order and count every time — `use_context` is the exception, since a
//! provided value is looked up by type, not call position. No event
//! wiring in `view!` yet either — [`Scope`] just exposes a dirty flag for
//! a host to poll.

mod context;
mod memo;
mod scope;
mod signal;

pub use context::{provide_context, use_context};
pub use memo::use_memo;
pub use scope::Scope;
pub use signal::{Signal, use_signal};

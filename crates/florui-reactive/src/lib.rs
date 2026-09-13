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
//! One `Scope` has one flat, call-ordered slot list, so every
//! order-sensitive hook call in the tree it renders must run in the same
//! order and count every time — `use_context` is the exception, since a
//! provided value is looked up by type, not call position. A host learns
//! about a [`Signal::set`] anywhere under its root either by polling
//! [`DirtyFlag::get`] or, better, registering a [`DirtyFlag::on_mark`]
//! callback to hear about it the instant it happens — [`batch`] coalesces
//! that notification across every write in one synchronous unit of work
//! (an event handler), rather than firing it once per [`Signal::set`].
//! `Signal::get`'s read-after-write contract is always synchronous,
//! batched or not: it returns whatever the most recent `set` stored,
//! immediately — `batch` defers *notifying a host*, never the write or a
//! subsequent read of it.

mod batch;
mod context;
mod dirty;
mod effect;
mod memo;
mod refs;
mod scope;
mod signal;

pub use batch::batch;
pub use context::{provide_context, use_context};
pub use dirty::DirtyFlag;
pub use effect::{Cleanup, use_effect};
pub use memo::use_memo;
pub use refs::{Ref, use_ref};
pub use scope::{Scope, render_once, use_child_scope};
pub use signal::{Signal, use_signal};

//! Persistent local component state: [`use_signal`], [`use_memo`],
//! [`use_context`], [`use_ref`], [`use_effect`], [`use_child_scope`]/
//! [`use_child_scope_keyed`], and the [`Scope`] that gives them somewhere
//! stable to live across repeated renders of the same tree.
//!
//! # Scope
//!
//! Not built yet: automatic dependency tracking for `use_memo`/`use_effect`
//! (deps are compared by equality, not inferred). One `Scope` has one
//! flat, call-ordered slot list for its *positional* hooks, so every
//! order-sensitive hook call in the tree it renders must run in the same
//! order and count every time — `use_context` is the exception (a
//! provided value is looked up by type, not call position), and so is
//! [`use_child_scope_keyed`] (a child looked up by [`Key`] instead of
//! position, so a list can reorder without losing each item's own state).
//! A host learns about a [`Signal::set`] anywhere under its root either by
//! polling [`DirtyFlag::get`] or, better, registering a
//! [`DirtyFlag::on_mark`] callback to hear about it the instant it
//! happens.

mod context;
mod dirty;
mod effect;
mod error_boundary;
pub mod executor;
mod key;
mod memo;
mod refs;
mod resource;
mod scope;
mod signal;
pub mod testing;

pub use context::{provide_context, use_context};
pub use dirty::DirtyFlag;
pub use effect::{Cleanup, use_effect};
pub use error_boundary::{ErrorBoundary, ErrorReporter, error_boundary, use_error_boundary};
pub use executor::Executor;
pub use key::Key;
pub use memo::use_memo;
pub use refs::{Ref, use_ref};
pub use resource::{Resource, ResourceHandle, use_resource};
pub use scope::{Scope, render_once, use_child_scope, use_child_scope_keyed};
pub use signal::{Signal, use_signal};

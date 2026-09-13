//! Component composition core: the element tree, the `view!` macro, and the
//! `#[component]` attribute. No styling, layout, text shaping, painting, or
//! rendering exists here yet.

mod children;
mod element;
mod handler;
mod into_nodes;
mod stylesheets;

pub use children::Children;
pub use element::{Element, ElementNode};
pub use florui_macros::{component, stylesheet, view};
pub use florui_reactive as reactive;
pub use handler::Handler;
pub use into_nodes::IntoNodes;
pub use stylesheets::{StylesheetSource, dedup as dedup_stylesheets};

pub mod prelude {
    pub use crate::reactive::{
        Cleanup, ErrorBoundary, ErrorReporter, Key, Ref, Scope, Signal, error_boundary,
        provide_context, render_once, use_child_scope, use_child_scope_keyed, use_context,
        use_effect, use_error_boundary, use_memo, use_ref, use_signal,
    };
    pub use crate::{Children, Element, Handler, IntoNodes, component, stylesheet, view};
}

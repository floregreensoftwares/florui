//! Component composition core: the element tree, the `view!` macro, and the
//! `#[component]` attribute. No styling, layout, text shaping, painting, or
//! rendering exists here yet.

mod children;
mod element;
mod into_nodes;
pub mod reactive;
mod stylesheets;

pub use children::Children;
pub use element::{Element, ElementNode};
pub use florui_macros::{component, stylesheet, view};
pub use into_nodes::IntoNodes;
pub use stylesheets::{StylesheetSource, dedup as dedup_stylesheets};

pub mod prelude {
    pub use crate::reactive::{Mounted, RefHandle, Signal, mount, use_effect, use_ref, use_signal};
    pub use crate::{Children, Element, IntoNodes, component, stylesheet, view};
}

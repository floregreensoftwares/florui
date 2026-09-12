//! The primitive tag registry shared by `florui-macros` (to validate
//! `view!` tags) and, eventually, `florui` (to attach default styles and
//! semantics once that engine exists). Kept dependency-free and separate
//! from both so neither has to own data the other also needs.

mod input;
mod registry;
mod suggest;

pub use input::{INITIAL_INPUT_TYPES, is_supported_input_type};
pub use registry::{Content, PRIMITIVES, Primitive, Status, find};
pub use suggest::suggest;

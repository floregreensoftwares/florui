//! [`use_viewport_size`]: the window's current logical size, reachable
//! from component code for the first time — [`crate::UiRuntime`] already
//! resolves a viewport internally but never handed it to a component. A
//! plain context value, not a registry: nothing to subscribe to.

use florui_reactive::use_context;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportSize {
    pub width: f32,
    pub height: f32,
}

/// The current window's own logical width/height. Requires a
/// [`ViewportSize`] in context, which [`crate::UiRuntime`] provides every
/// render — calling this outside one is a programming error, not a
/// recoverable condition.
///
/// # Panics
///
/// Panics if no [`ViewportSize`] is in context.
pub fn use_viewport_size() -> (f32, f32) {
    let viewport = use_context::<ViewportSize>().expect(
        "use_viewport_size needs a ViewportSize in context — only a UiRuntime-hosted render \
         provides one",
    );
    (viewport.width, viewport.height)
}

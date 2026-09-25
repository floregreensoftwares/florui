//! Real OS clipboard access for text-input copy/cut/paste — Ctrl+C/X/V,
//! handled entirely inside [`crate::desktop`]'s own keyboard dispatch, so
//! this is never reachable from component code via `use_context` the way
//! [`crate::window_controls::WindowControls`] is: nothing outside the
//! desktop host's own event handling needs it.
//!
//! One instance per process, not per window — a real OS clipboard is a
//! process-level resource, not scoped to any one window.

use std::cell::RefCell;

/// Wraps a real `arboard::Clipboard`. Construction can fail (no clipboard
/// service available) — logged once and left unavailable for the rest of
/// the process, the same "log and continue, don't crash" precedent
/// `crate::drag_drop::register`'s own doc already establishes for a
/// similar optional OS capability.
pub(crate) struct Clipboard(RefCell<Option<arboard::Clipboard>>);

impl Clipboard {
    pub(crate) fn new() -> Self {
        match arboard::Clipboard::new() {
            Ok(clipboard) => Self(RefCell::new(Some(clipboard))),
            Err(error) => {
                eprintln!("florui-platform: system clipboard unavailable: {error}");
                Self(RefCell::new(None))
            }
        }
    }

    /// The clipboard's current text, if it holds any and reading it
    /// succeeds — `None` either way is silently treated as "nothing to
    /// paste," not an error a caller needs to react to.
    pub(crate) fn get_text(&self) -> Option<String> {
        self.0.borrow_mut().as_mut()?.get_text().ok()
    }

    /// Writes `text` to the clipboard — a failure (or an unavailable
    /// clipboard) is logged, not surfaced: a copy/cut a real OS clipboard
    /// silently swallows is a real, sharp-edged case, but not one this
    /// slice's own caller (a keyboard shortcut with no error-reporting UI
    /// of its own) has anywhere to report it to.
    pub(crate) fn set_text(&self, text: String) {
        let mut clipboard = self.0.borrow_mut();
        let Some(clipboard) = clipboard.as_mut() else {
            return;
        };
        if let Err(error) = clipboard.set_text(text) {
            eprintln!("florui-platform: could not write to the system clipboard: {error}");
        }
    }
}

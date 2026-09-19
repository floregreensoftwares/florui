//! Opt-in single-instance detection plus the bounded queue of typed
//! activation events (ordinary launch, URL open, file open) a second
//! launch hands to the already-running process. Pure/testable types and
//! logic only -- see `crate::os::windows::single_instance` for the real
//! named-mutex/named-pipe mechanics this builds on.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use florui_reactive::use_context;
use serde::{Deserialize, Serialize};

/// The plain, already-resolved values an app passes in to opt into
/// single-instance behavior -- deliberately not `florui-config`'s own
/// `ActivationConfig`, the same reasoning as `WindowPersistence`: this
/// crate never gains a `florui-config` dependency, so the app resolves it
/// and hands over a plain string.
#[derive(Debug, Clone, PartialEq)]
pub struct SingleInstance {
    pub app_identifier: String,
    /// How long a second launch waits for the primary to acknowledge a
    /// handed-off activation event before giving up.
    pub handoff_timeout: Duration,
}

impl Default for SingleInstance {
    fn default() -> Self {
        Self {
            app_identifier: String::new(),
            handoff_timeout: Duration::from_secs(2),
        }
    }
}

/// A typed activation payload -- never a shell-joined string. `Launch`
/// carries the second process's own `argv` (its first element still the
/// executable path, matching `std::env::args()`) so the primary can
/// re-parse it exactly as it would have parsed its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ActivationEvent {
    Launch { args: Vec<String> },
    OpenUrl { url: String },
    OpenFiles { paths: Vec<PathBuf> },
}

/// Named-object charset for a Windows mutex/pipe name is broad, but kept
/// deliberately narrow here (matches what an `app.identifier` already
/// looks like -- reverse-DNS-style, see `florui-config`'s own schema) so
/// a stray character in a user-supplied identifier can never produce an
/// invalid or surprising object name.
fn sanitize_identifier(app_identifier: &str) -> String {
    app_identifier
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `Local\` already scopes a named kernel object to the caller's own
/// logon session (Windows transparently redirects an unprefixed or
/// `Local\`-prefixed name through `\Sessions\<n>\BaseNamedObjects`; only
/// `Global\` escapes that) -- so this needs no separate session-id
/// component, the same "already per-user by construction" shape
/// `window_state.rs` documents for `LOCALAPPDATA`. Distinct dev/production
/// identifiers (already how `WindowPersistence` avoids collisions) produce
/// distinct names for free.
pub(crate) fn mutex_name(app_identifier: &str) -> String {
    format!(
        "Local\\florui-single-instance-{}",
        sanitize_identifier(app_identifier)
    )
}

/// Named pipes resolve through the same per-session object namespace as a
/// named mutex, so this needs no separate session scoping either.
pub(crate) fn pipe_name(app_identifier: &str) -> String {
    format!(
        "\\\\.\\pipe\\florui-single-instance-{}",
        sanitize_identifier(app_identifier)
    )
}

/// How many not-yet-delivered activation events [`ActivationQueue`] holds
/// before it starts dropping the oldest -- bounds memory if a primary is
/// slow to mount its first window while several launches race in.
const MAX_QUEUED_EVENTS: usize = 16;

/// Activation events that arrived (from [`crate::os::windows::single_instance`]'s
/// pipe listener) before -- or between renders of -- the window that reads
/// them. Bounded: a `push` past [`MAX_QUEUED_EVENTS`] drops the oldest
/// queued event and logs a diagnostic, the same non-fatal-on-overflow
/// treatment this crate already gives corrupted window state.
#[derive(Debug, Default)]
pub(crate) struct ActivationQueue {
    events: VecDeque<ActivationEvent>,
}

impl ActivationQueue {
    pub(crate) fn push(&mut self, event: ActivationEvent) {
        if self.events.len() >= MAX_QUEUED_EVENTS {
            self.events.pop_front();
            eprintln!(
                "florui-platform: activation event queue full ({MAX_QUEUED_EVENTS}), dropping the oldest pending event"
            );
        }
        self.events.push_back(event);
    }

    pub(crate) fn take_pending(&mut self) -> Vec<ActivationEvent> {
        self.events.drain(..).collect()
    }
}

/// Reachable from the primary window's component tree via
/// [`use_activation_events`] from that window's very first render onward
/// -- see [`crate::desktop::DesktopHost`]'s own doc for why only the
/// primary window gets this context.
#[derive(Debug, Clone)]
pub struct ActivationEvents(pub(crate) Rc<RefCell<ActivationQueue>>);

impl ActivationEvents {
    /// Every event queued since the last call, oldest first. Draining
    /// (not peeking) is deliberate: this is the one window that owns this
    /// queue, so nothing else is left to read the same event twice.
    pub fn take_pending(&self) -> Vec<ActivationEvent> {
        self.0.borrow_mut().take_pending()
    }
}

/// Reads the [`ActivationEvents`] [`crate::desktop::DesktopHost`] provides
/// to the primary window every render. `None` outside a real desktop host,
/// or in any window other than the primary one, or when single-instance
/// activation was never opted into.
pub fn use_activation_events() -> Option<ActivationEvents> {
    use_context::<ActivationEvents>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_and_pipe_names_are_scoped_to_local_session_namespace() {
        assert!(mutex_name("com.floregreen.garden").starts_with("Local\\"));
        assert!(pipe_name("com.floregreen.garden").starts_with("\\\\.\\pipe\\"));
    }

    #[test]
    fn dev_and_production_identifiers_never_collide() {
        let prod = mutex_name("com.floregreen.garden");
        let dev = mutex_name("com.floregreen.garden.dev");
        assert_ne!(prod, dev);
        let prod_pipe = pipe_name("com.floregreen.garden");
        let dev_pipe = pipe_name("com.floregreen.garden.dev");
        assert_ne!(prod_pipe, dev_pipe);
    }

    #[test]
    fn sanitize_identifier_replaces_disallowed_characters() {
        assert_eq!(
            sanitize_identifier("com.floregreen.garden"),
            "com.floregreen.garden"
        );
        assert_eq!(
            sanitize_identifier("weird/name\\with:chars"),
            "weird_name_with_chars"
        );
    }

    #[test]
    fn activation_event_round_trips_through_json() {
        let events = [
            ActivationEvent::Launch {
                args: vec!["app.exe".to_owned(), "--flag".to_owned()],
            },
            ActivationEvent::OpenUrl {
                url: "florui://open?id=42".to_owned(),
            },
            ActivationEvent::OpenFiles {
                paths: vec![PathBuf::from("C:\\a b\\c.txt")],
            },
        ];
        for event in events {
            let json = serde_json::to_vec(&event).unwrap();
            let decoded: ActivationEvent = serde_json::from_slice(&json).unwrap();
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn queue_drains_in_fifo_order() {
        let mut queue = ActivationQueue::default();
        queue.push(ActivationEvent::OpenUrl {
            url: "a".to_owned(),
        });
        queue.push(ActivationEvent::OpenUrl {
            url: "b".to_owned(),
        });
        let drained = queue.take_pending();
        assert_eq!(
            drained,
            vec![
                ActivationEvent::OpenUrl {
                    url: "a".to_owned()
                },
                ActivationEvent::OpenUrl {
                    url: "b".to_owned()
                },
            ]
        );
        assert!(queue.take_pending().is_empty());
    }

    #[test]
    fn queue_drops_oldest_once_it_is_full() {
        let mut queue = ActivationQueue::default();
        for i in 0..MAX_QUEUED_EVENTS + 3 {
            queue.push(ActivationEvent::OpenUrl { url: i.to_string() });
        }
        let drained = queue.take_pending();
        assert_eq!(drained.len(), MAX_QUEUED_EVENTS);
        assert_eq!(
            drained.first(),
            Some(&ActivationEvent::OpenUrl {
                url: "3".to_owned()
            })
        );
        assert_eq!(
            drained.last(),
            Some(&ActivationEvent::OpenUrl {
                url: (MAX_QUEUED_EVENTS + 2).to_string()
            })
        );
    }
}

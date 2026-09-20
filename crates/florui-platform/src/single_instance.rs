//! Named-mutex ownership plus activation handoff — see
//! `crate::os::windows::single_instance` for the real implementation. On
//! any other platform, single-instance enforcement is silently
//! unenforced: every launch reports [`InstanceRole::Primary`] and a
//! handoff always [`HandoffOutcome::Failed`]s, since there is no
//! equivalent IPC mechanism here yet.

#[cfg(target_os = "windows")]
pub(crate) use crate::os::windows::single_instance::{
    HandoffOutcome, InstanceRole, acquire, handoff, probe_capability, spawn_activation_listener,
};

#[cfg(not(target_os = "windows"))]
pub(crate) use stub::{
    HandoffOutcome, InstanceRole, acquire, handoff, probe_capability, spawn_activation_listener,
};

#[cfg(not(target_os = "windows"))]
mod stub {
    use std::time::Duration;

    use crate::activation::ActivationEvent;

    /// `Primary`'s `()` payload stands in for the real build's
    /// `MutexOwnership` guard, so call sites can bind and hold it
    /// uniformly across platforms without caring what it actually is.
    pub(crate) enum InstanceRole {
        Primary(()),
        Secondary,
    }

    pub(crate) enum HandoffOutcome {
        Delivered,
        Failed,
    }

    pub(crate) fn acquire(_app_identifier: &str) -> std::io::Result<InstanceRole> {
        Ok(InstanceRole::Primary(()))
    }

    pub(crate) fn spawn_activation_listener(
        _app_identifier: &str,
        _on_event: impl Fn(ActivationEvent) + Send + 'static,
    ) -> std::io::Result<()> {
        Ok(())
    }

    pub(crate) fn handoff(
        _app_identifier: &str,
        _event: &ActivationEvent,
        _timeout: Duration,
    ) -> HandoffOutcome {
        HandoffOutcome::Failed
    }

    /// No equivalent mechanism exists on this platform yet -- see this
    /// module's own doc.
    pub(crate) fn probe_capability() -> bool {
        false
    }
}

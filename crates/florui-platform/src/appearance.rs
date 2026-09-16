//! Window appearance capability contracts: what a caller can ask a real
//! window for (application-drawn decorations, a transparent surface) and
//! what this host can actually confirm happened — advertised
//! independently per capability, not one yes/no for the whole window,
//! since an OS/compositor can honor one and ignore another.
//!
//! This crate's own render pipeline already supports alpha *within* the
//! painted scene (`background-color: rgba(...)`, `box-shadow`'s own
//! translucent colors); [`WindowCapability::TransparentSurface`] is a
//! different thing entirely — whether the *window itself* composites
//! over the desktop with its own alpha channel, an OS/compositor
//! decision this crate has no control over and, for some capabilities,
//! no reliable way to even confirm happened.
//!
//! [`DecorationMode`] itself *is* wired into production:
//! [`crate::run_with_options`]/[`crate::run_with_css_reload_and_options`]
//! pass it straight to `winit`'s own `with_decorations`, and
//! [`DecorationMode::Custom`] is exactly what makes
//! [`crate::use_window_controls`] meaningful — an application-drawn title
//! bar needs a real way to move, minimize, and maximize the window it
//! just took OS-drawn chrome away from. [`probe_appearance`] itself
//! remains a standalone probe against a window built directly from
//! [`probe_window_attributes`] — this module's own contract types, not
//! [`crate::desktop::DesktopHost`]'s real window, so a report from it
//! reflects only what that specific probe combination (`winit` +
//! `softbuffer`, see the section below) can confirm, not the production
//! host's own GPU-preferred presentation (see [`crate::gpu`]'s own doc).
//! Two of the five capabilities ([`WindowCapability::SystemBackdropMaterial`],
//! [`WindowCapability::SystemCaptionIntegration`]) still have no probe
//! here at all — [`crate::caption::override_caption_colors`] is real,
//! production code for the latter (a different, lighter capability than
//! [`WindowCapability::CustomDecorations`] — see its own doc), but this
//! module doesn't yet turn that into a probed [`CapabilityStatus`];
//! [`probe_appearance`] still always reports
//! [`CapabilityStatus::Unknown`] for both.
//!
//! # A real, verified finding: `TransparentSurface` can't work through `softbuffer`
//!
//! Running the `appearance_probe` example live: a window built from
//! [`probe_window_attributes`] with `transparent: true`, painted with a
//! real half-alpha frame through `softbuffer` directly (this probe's own
//! presentation, not [`crate::desktop::DesktopHost`]'s — see this
//! module's own doc above), opened as a plain opaque rectangle — no
//! desktop content showing through at all. The reason turned out not
//! to be platform-specific once traced to `softbuffer`'s own documented
//! buffer format: every pixel is a `u32` whose highest 8 bits "are to be
//! set to 0" — there is no alpha channel slot in the format at all, on
//! any platform, so there is no value this crate could ever write into a
//! `softbuffer` frame that would make one pixel more transparent than
//! another. (Windows' own backend independently confirms the same
//! outcome a second way: it presents via a plain GDI `BitBlt`, with no
//! `UpdateLayeredWindow` call or other per-pixel-alpha compositing path
//! at all — but the buffer format itself already rules this out before a
//! platform backend even gets involved.) This is real negative evidence
//! about `softbuffer` itself, not the general "no way to confirm" case
//! the rest of this module's own doc describes — which is why *this
//! probe's own* [`probe_appearance`] reports
//! [`CapabilityStatus::Unsupported`] for `TransparentSurface` whenever
//! it's requested, not the weaker [`CapabilityStatus::Attempted`] a
//! window-creation-didn't-error signal alone would justify.
//!
//! [`crate::desktop::DesktopHost`]'s own real presentation is no longer
//! `softbuffer`-only, though: [`crate::gpu::GpuPresenter`] now tries a
//! real `wgpu` adapter first, and on Windows can reach genuine
//! per-pixel-alpha compositing through it — real, human-confirmed
//! transparent-window compositing, desktop content visibly showing
//! through at both partial and full transparency, via a specific,
//! non-default `wgpu` configuration (DX12's `DxgiFromVisual` swap chain
//! plus a Windows-specific `with_no_redirection_bitmap(true)` window
//! attribute `winit` alone won't set) — see [`crate::gpu`]'s own doc for
//! the exact recipe and what's been validated live through
//! `DesktopHost` itself, not just this standalone probe.

use std::collections::HashMap;

use winit::window::{Window, WindowAttributes};

/// One independently-advertised window appearance capability — see this
/// module's own doc for why these are reported separately rather than as
/// one yes/no for the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowCapability {
    /// Application-drawn decorations (a custom title bar built from
    /// ordinary Florui components, the same composition model as any
    /// other content) instead of the OS's own system chrome.
    CustomDecorations,
    /// A window surface with a genuinely transparent (non-opaque) alpha
    /// channel, composited by the desktop's own window manager — see
    /// this module's own doc for how this differs from alpha within the
    /// painted scene, which this crate already supports regardless of
    /// this capability.
    TransparentSurface,
    /// A system-provided backdrop material (an OS blur-behind/acrylic/
    /// Mica-style effect) behind the window's own transparent regions —
    /// distinct from `TransparentSurface` itself: a window can have a
    /// transparent surface with no system material behind it at all.
    SystemBackdropMaterial,
    /// Live OS-driven resizing (edges/corners), independent of whether
    /// decorations are system- or application-drawn.
    Resizing,
    /// System caption integration — snapping, the system menu, and other
    /// OS chrome behavior that should keep working even once decorations
    /// are application-drawn.
    SystemCaptionIntegration,
}

/// Whether a [`WindowCapability`] is available on the current host, and
/// how confidently this crate actually knows that — deliberately not a
/// plain `bool`: a real probe can tell the difference between "the OS
/// visibly honored this" and "nothing contradicted the request, but
/// there's no way to be sure," and collapsing that distinction would
/// misrepresent exactly the risk this module exists to surface honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityStatus {
    /// This host requested the capability and observed a real signal
    /// consistent with it — the strongest claim this module makes, and
    /// still not proof of correct production behavior (an API-level
    /// check is not the same as a human confirming the desktop
    /// compositor actually did the right thing — see this module's own
    /// doc).
    Attempted,
    /// The OS/build/compositor visibly overrode or rejected the request.
    Unsupported,
    /// Nothing has probed this capability on this host yet — either it
    /// wasn't requested, or (see this module's own doc)
    /// `SystemBackdropMaterial`/`SystemCaptionIntegration` have no probe
    /// at all yet.
    Unknown,
}

/// Whether a window's own chrome is drawn by the OS or the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecorationMode {
    /// The OS's own title bar, buttons, and borders.
    #[default]
    System,
    /// No OS-drawn chrome — the application's own components own the
    /// title bar, using the same composition model as any other content.
    Custom,
}

/// What an application asks a real window for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppearanceRequest {
    pub decorations: DecorationMode,
    pub transparent: bool,
}

/// What was requested vs. what a real probe actually observed, plus
/// every capability's own independently-known status — the "requested
/// and effective appearance" contract a caller needs to explain or adapt
/// to whatever the OS didn't grant, instead of silently rendering as if
/// it had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceReport {
    pub requested: AppearanceRequest,
    pub effective: AppearanceRequest,
    pub capabilities: HashMap<WindowCapability, CapabilityStatus>,
}

/// Builds real `winit` window attributes from `request` — a caller
/// creating the probe window itself (see [`probe_appearance`]'s own doc
/// for why this module doesn't own window/event-loop creation) uses this
/// to build the attributes, then reads the result back with
/// [`probe_appearance`] once the window exists.
pub fn probe_window_attributes(request: AppearanceRequest) -> WindowAttributes {
    Window::default_attributes()
        .with_decorations(matches!(request.decorations, DecorationMode::System))
        .with_transparent(request.transparent)
        .with_resizable(true)
}

/// What a real, already-created window reports back about itself —
/// [`probe_appearance`]'s own raw material, split out so the report-
/// building logic itself ([`report_from_observation`]) is pure and
/// testable without a real window.
struct Observation {
    decorated: bool,
    resizable: bool,
}

/// Reads back what a real, already-created `window` (built from
/// [`probe_window_attributes`]) actually did with `request`, and turns
/// it into a full [`AppearanceReport`]. Takes an already-created
/// [`Window`] rather than owning window/event-loop creation itself —
/// the same split [`crate::desktop::DesktopHost`] already draws between
/// owning a real event loop and the window-independent
/// [`crate::UiRuntime`] it drives; this module is a capability probe,
/// not a second desktop host.
pub fn probe_appearance(window: &Window, request: AppearanceRequest) -> AppearanceReport {
    let observation = Observation {
        decorated: window.is_decorated(),
        resizable: window.is_resizable(),
    };
    report_from_observation(request, &observation)
}

fn report_from_observation(request: AppearanceRequest, observed: &Observation) -> AppearanceReport {
    let mut capabilities = HashMap::new();

    let effective_decorations = if observed.decorated {
        DecorationMode::System
    } else {
        DecorationMode::Custom
    };
    // `is_decorated` is a real, live query of the actual window (see its
    // own `winit` doc) — a trustworthy signal, but only meaningful when
    // custom decorations were actually requested; a plain system-chrome
    // request tells us nothing about whether this capability works.
    capabilities.insert(
        WindowCapability::CustomDecorations,
        match (request.decorations, effective_decorations) {
            (DecorationMode::Custom, DecorationMode::Custom) => CapabilityStatus::Attempted,
            (DecorationMode::Custom, DecorationMode::System) => CapabilityStatus::Unsupported,
            (DecorationMode::System, _) => CapabilityStatus::Unknown,
        },
    );

    // `is_resizable` is likewise a real, live query — always probed,
    // since `probe_window_attributes` always requests it.
    capabilities.insert(
        WindowCapability::Resizing,
        if observed.resizable {
            CapabilityStatus::Attempted
        } else {
            CapabilityStatus::Unsupported
        },
    );

    // No accessor exists on a real `winit::window::Window` to read
    // transparency back, so in general this crate could only ever report
    // `Attempted` when requested, never a stronger claim — but
    // `softbuffer` (this crate's own current desktop presentation
    // backend, on every platform) rules `TransparentSurface` out
    // entirely on its own: its own documented pixel format has no alpha
    // channel at all (every `u32`'s highest 8 bits "are to be set to 0"),
    // so there is no value this crate could ever write that would make
    // one pixel more transparent than another, regardless of what the
    // window's own attribute requested or the OS would otherwise allow.
    // That's real, verified negative evidence, not mere inability to
    // confirm — see this module's own doc — so a request reports
    // `Unsupported`, not `Attempted`. `Unknown` when transparency wasn't
    // requested at all, since "not requested" and "requested and
    // rejected" are different facts.
    capabilities.insert(
        WindowCapability::TransparentSurface,
        if request.transparent {
            CapabilityStatus::Unsupported
        } else {
            CapabilityStatus::Unknown
        },
    );

    // No `winit` API surface at all yet — see this module's own doc.
    capabilities.insert(
        WindowCapability::SystemBackdropMaterial,
        CapabilityStatus::Unknown,
    );
    capabilities.insert(
        WindowCapability::SystemCaptionIntegration,
        CapabilityStatus::Unknown,
    );

    AppearanceReport {
        requested: request,
        effective: AppearanceRequest {
            decorations: effective_decorations,
            // Definitively not achieved through this crate's own
            // `softbuffer` presentation — see `TransparentSurface`'s own
            // status above.
            transparent: false,
        },
        capabilities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(decorations: DecorationMode, transparent: bool) -> AppearanceRequest {
        AppearanceRequest {
            decorations,
            transparent,
        }
    }

    #[test]
    fn custom_decorations_granted_reports_attempted() {
        let report = report_from_observation(
            request(DecorationMode::Custom, false),
            &Observation {
                decorated: false,
                resizable: true,
            },
        );
        assert_eq!(
            report.capabilities[&WindowCapability::CustomDecorations],
            CapabilityStatus::Attempted
        );
        assert_eq!(report.effective.decorations, DecorationMode::Custom);
    }

    #[test]
    fn custom_decorations_overridden_by_the_os_reports_unsupported() {
        // Requested custom (undecorated), but the real window still came
        // back decorated — the OS/build ignored the request.
        let report = report_from_observation(
            request(DecorationMode::Custom, false),
            &Observation {
                decorated: true,
                resizable: true,
            },
        );
        assert_eq!(
            report.capabilities[&WindowCapability::CustomDecorations],
            CapabilityStatus::Unsupported
        );
        assert_eq!(
            report.effective.decorations,
            DecorationMode::System,
            "effective must reflect what the OS actually did, not what was requested"
        );
    }

    #[test]
    fn a_plain_system_decorations_request_never_probes_the_capability() {
        // Not requesting custom decorations at all tells us nothing
        // about whether the capability works — must not be reported as
        // either Attempted or Unsupported.
        let report = report_from_observation(
            request(DecorationMode::System, false),
            &Observation {
                decorated: true,
                resizable: true,
            },
        );
        assert_eq!(
            report.capabilities[&WindowCapability::CustomDecorations],
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn resizing_reflects_the_real_window_state_directly() {
        let attempted = report_from_observation(
            request(DecorationMode::System, false),
            &Observation {
                decorated: true,
                resizable: true,
            },
        );
        assert_eq!(
            attempted.capabilities[&WindowCapability::Resizing],
            CapabilityStatus::Attempted
        );

        let unsupported = report_from_observation(
            request(DecorationMode::System, false),
            &Observation {
                decorated: true,
                resizable: false,
            },
        );
        assert_eq!(
            unsupported.capabilities[&WindowCapability::Resizing],
            CapabilityStatus::Unsupported
        );
    }

    #[test]
    fn transparency_requested_reports_unsupported_with_real_evidence() {
        let report = report_from_observation(
            request(DecorationMode::System, true),
            &Observation {
                decorated: true,
                resizable: true,
            },
        );
        assert_eq!(
            report.capabilities[&WindowCapability::TransparentSurface],
            CapabilityStatus::Unsupported,
            "softbuffer's own pixel format has no alpha channel at all"
        );
        assert!(
            !report.effective.transparent,
            "effective must reflect the real (negative) finding, not the request"
        );
    }

    #[test]
    fn transparency_not_requested_reports_unknown_not_unsupported() {
        let report = report_from_observation(
            request(DecorationMode::System, false),
            &Observation {
                decorated: true,
                resizable: true,
            },
        );
        assert_eq!(
            report.capabilities[&WindowCapability::TransparentSurface],
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn capabilities_with_no_probe_at_all_always_report_unknown() {
        let report = report_from_observation(
            request(DecorationMode::Custom, true),
            &Observation {
                decorated: false,
                resizable: true,
            },
        );
        assert_eq!(
            report.capabilities[&WindowCapability::SystemBackdropMaterial],
            CapabilityStatus::Unknown
        );
        assert_eq!(
            report.capabilities[&WindowCapability::SystemCaptionIntegration],
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn requested_is_preserved_unchanged_alongside_effective() {
        let requested = request(DecorationMode::Custom, true);
        let report = report_from_observation(
            requested,
            &Observation {
                decorated: true, // overridden by the OS
                resizable: true,
            },
        );
        assert_eq!(report.requested, requested);
        assert_ne!(
            report.requested, report.effective,
            "this fixture's own point: requested and effective can genuinely differ"
        );
    }
}

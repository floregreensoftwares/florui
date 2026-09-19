//! Light/dark theme detection and explicit override — neither an
//! accessibility-API read (see [`crate::accessibility`]) nor a capability
//! probe (see [`crate::appearance`]): `winit` already abstracts the real
//! OS read cross-platform (`Window::theme`/`WindowEvent::ThemeChanged`),
//! so this module only resolves the application's own override against
//! it, nothing more.
//!
//! `winit::window::Window::theme()` conflates "explicit override" and "OS
//! truth": once [`winit::window::WindowAttributes::with_theme`]/
//! [`winit::window::Window::set_theme`] has been used with `Some(_)`,
//! `Window::theme()` permanently reports that override — there is no
//! separate way to recover what the OS actually prefers regardless of it
//! (confirmed against `winit`'s own Windows backend source: the real OS
//! registry read only ever happens when no override is set). So, unlike
//! `crate::accessibility::prefers_reduced_motion`'s OS-truth-plus-opt-out
//! pair, there is only ever one meaningful signal here: the *effective*
//! color scheme, override-or-OS, resolved once by [`effective_color_scheme`].

use winit::window::{Theme as WinitTheme, Window};

/// What a window asks for — mirrors [`crate::appearance::DecorationMode`]'s
/// own "system default vs. explicit override" shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreference {
    /// Follow the real OS/window preference, live.
    #[default]
    System,
    /// Always light, regardless of the OS preference.
    Light,
    /// Always dark, regardless of the OS preference.
    Dark,
}

/// The resolved scheme florui's own CSS cascade sees — see this module's
/// own doc for why this is the only signal, not a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    pub fn is_dark(self) -> bool {
        matches!(self, ColorScheme::Dark)
    }
}

impl From<WinitTheme> for ColorScheme {
    fn from(theme: WinitTheme) -> Self {
        match theme {
            WinitTheme::Light => ColorScheme::Light,
            WinitTheme::Dark => ColorScheme::Dark,
        }
    }
}

impl From<ColorScheme> for WinitTheme {
    fn from(scheme: ColorScheme) -> Self {
        match scheme {
            ColorScheme::Light => WinitTheme::Light,
            ColorScheme::Dark => WinitTheme::Dark,
        }
    }
}

/// `preference`'s own override if `Light`/`Dark`; otherwise `window`'s real
/// reported theme, defaulting to [`ColorScheme::Light`] when the platform
/// doesn't report one at all (X11/iOS/Android/Orbital, per `winit`'s own
/// documented coverage) — today's existing default, preserved as the
/// honest fallback rather than asserting a preference nothing confirmed.
pub fn effective_color_scheme(preference: ThemePreference, window: &Window) -> ColorScheme {
    match preference {
        ThemePreference::Light => ColorScheme::Light,
        ThemePreference::Dark => ColorScheme::Dark,
        ThemePreference::System => window
            .theme()
            .map(ColorScheme::from)
            .unwrap_or(ColorScheme::Light),
    }
}

/// The `winit` theme to request at window creation for an explicit
/// override — `None` for [`ThemePreference::System`], so `winit` follows
/// the real OS preference (and keeps firing `WindowEvent::ThemeChanged`
/// live) exactly as if this crate had never asked for anything.
pub fn requested_winit_theme(preference: ThemePreference) -> Option<WinitTheme> {
    match preference {
        ThemePreference::System => None,
        ThemePreference::Light => Some(WinitTheme::Light),
        ThemePreference::Dark => Some(WinitTheme::Dark),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_scheme_round_trips_through_winit_theme() {
        assert_eq!(ColorScheme::from(WinitTheme::Light), ColorScheme::Light);
        assert_eq!(ColorScheme::from(WinitTheme::Dark), ColorScheme::Dark);
        assert_eq!(WinitTheme::from(ColorScheme::Light), WinitTheme::Light);
        assert_eq!(WinitTheme::from(ColorScheme::Dark), WinitTheme::Dark);
    }

    #[test]
    fn is_dark_is_true_only_for_dark() {
        assert!(!ColorScheme::Light.is_dark());
        assert!(ColorScheme::Dark.is_dark());
    }

    #[test]
    fn requested_winit_theme_is_none_only_for_system() {
        assert_eq!(requested_winit_theme(ThemePreference::System), None);
        assert_eq!(
            requested_winit_theme(ThemePreference::Light),
            Some(WinitTheme::Light)
        );
        assert_eq!(
            requested_winit_theme(ThemePreference::Dark),
            Some(WinitTheme::Dark)
        );
    }

    #[test]
    fn theme_preference_defaults_to_system() {
        assert_eq!(ThemePreference::default(), ThemePreference::System);
    }
}

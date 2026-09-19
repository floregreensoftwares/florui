//! The public, fully-resolved shape `resolve()` produces: every field
//! populated (no `Option` where a default exists), plus where each value
//! actually came from.

use crate::location::SourceLocation;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Which native/build target `resolve()` should check assets against —
/// asset-existence checks are target-scoped ("resolve assets... for the
/// selected target"), unlike schema validation, which always runs in full
/// regardless of target. `None` (schema-only mode) skips asset checks
/// entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Native,
    Web,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub app: AppConfig,
    pub window: WindowConfig,
    pub bundle: BundleConfig,
    pub dev: DevConfig,
    pub web: WebConfig,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub identifier: Option<String>,
    /// The Cargo package name when `app.name` is unset.
    pub name: String,
    pub description: Option<String>,
    /// The Cargo-resolved concrete version when `app.version` is unset or
    /// requests `{ workspace = true }`.
    pub version: String,
    pub icons: IconsConfig,
    pub activation: ActivationConfig,
    pub locales: LocalesConfig,
}

/// `default_locale` is always populated, defaulting to `"en"` even with no
/// `[app.locales]` declared at all.
#[derive(Debug, Clone)]
pub struct LocalesConfig {
    pub default_locale: String,
    pub locales: BTreeMap<String, LocaleConfig>,
}

#[derive(Debug, Clone)]
pub struct LocaleConfig {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Typed declaration only -- no OS registration, no single-instance IPC.
/// See `crate::schema::RawActivation`'s doc comment for why.
#[derive(Debug, Clone, Default)]
pub struct ActivationConfig {
    pub single_instance: bool,
    pub url_schemes: Vec<String>,
    pub file_associations: Vec<FileAssociationConfig>,
}

#[derive(Debug, Clone)]
pub struct FileAssociationConfig {
    pub extension: String,
    pub mime_type: Option<String>,
    pub description: Option<String>,
    pub identity: String,
}

#[derive(Debug, Clone, Default)]
pub struct IconsConfig {
    /// Resolved relative to `florui.config.toml`'s own directory, never
    /// the process's current directory.
    pub source: Option<PathBuf>,
    pub windows: Option<PathBuf>,
    pub macos: Option<PathBuf>,
    pub linux: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct WindowConfig {
    /// `app.name` when `window.title` is unset.
    pub title: String,
    /// `None` means "no opinion" -- this crate does not invent a
    /// framework-wide default window size.
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub decorations: DecorationsSetting,
    pub transparent: bool,
    pub persistence: WindowPersistenceConfig,
}

/// Typed declaration only -- no bounds save/restore, no monitor
/// revalidation. See `crate::schema::RawWindowPersistence`'s doc comment.
#[derive(Debug, Clone)]
pub struct WindowPersistenceConfig {
    pub enabled: bool,
    /// Defaults to `"main"` when `persistence` is declared without a key.
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationsSetting {
    System,
    Custom,
}

#[derive(Debug, Clone, Default)]
pub struct BundleConfig {
    pub publisher: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DevConfig {
    /// The merged legacy-or-new dev example -- see `resolve.rs` for the
    /// duplicate-definition check that runs before this is populated.
    pub example: Option<String>,
}

/// Independent of native `[app]`/`[window]` -- `title`/`description` fall
/// back to `app.name`/`app.description`, never to native `window.title`,
/// and there is no implicit inheritance of `app.icons` into `icons`.
#[derive(Debug, Clone)]
pub struct WebConfig {
    pub title: String,
    pub description: Option<String>,
    pub base_path: String,
    pub icons: WebIconsConfig,
}

#[derive(Debug, Clone, Default)]
pub struct WebIconsConfig {
    /// Resolved relative to `florui.config.toml`'s own directory, same as
    /// `IconsConfig`'s fields.
    pub favicon: Option<PathBuf>,
    pub apple_touch_icon: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Provenance {
    /// From `florui.config.toml` itself; carries an exact location for the
    /// subset of fields that get post-parse semantic validation (window
    /// sizes, decorations, app.version, icon paths, dev.example) and `None`
    /// for plain fields that are validated for free by `toml`'s own type
    /// checking.
    ConfigFile(Option<SourceLocation>),
    /// Resolved from `cargo metadata` (e.g. `app.version` defaulting to the
    /// package's own Cargo-resolved version).
    CargoManifest,
    /// From `[package.metadata.florui.dev]`.
    LegacyMetadata,
    BuiltinDefault,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldProvenance {
    pub field: &'static str,
    pub provenance: Provenance,
}

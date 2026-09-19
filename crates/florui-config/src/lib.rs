//! Typed `florui.config.toml` schema, discovery, and resolution.
//!
//! [`resolve`] turns a [`CargoProjectFacts`] (from [`resolve_cargo_project`])
//! plus whatever `florui.config.toml` sits at its package root into a fully
//! defaulted [`ResolvedConfig`], with per-field [`Provenance`] and non-fatal
//! [`Diagnostic`]s. Covers `schema_version`, `[app]`, `[app.icons]`,
//! `[window]`, `[bundle]`, `[dev]` (the last only to the extent needed
//! to detect a conflict with the legacy `[package.metadata.florui.dev]`),
//! and the independent `[web]`/`[web.icons]`. Environments, activation,
//! window persistence, and locales are not part of this schema yet.

mod error;
mod location;
mod project;
mod resolve;
mod resolved;
mod schema;

pub use error::{
    ConfigError, Diagnostic, SemanticConfigError, Severity, WindowSizeError,
    WorkspaceInheritanceError,
};
pub use location::SourceLocation;
pub use project::{
    CargoProjectFacts, ProjectResolutionError, config_file_path, parse_cargo_project_facts,
    resolve_cargo_project,
};
pub use resolve::{Resolution, resolve};
pub use resolved::{
    AppConfig, BundleConfig, DecorationsSetting, DevConfig, FieldProvenance, IconsConfig,
    Provenance, ResolvedConfig, Target, WebConfig, WebIconsConfig, WindowConfig,
};

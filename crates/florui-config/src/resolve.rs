//! The top-level entry point: turns a [`CargoProjectFacts`] plus whatever
//! `florui.config.toml` (if any) sits at its `package_root` into a
//! [`Resolution`] -- validated, fully-defaulted config, field provenance,
//! and non-fatal diagnostics.

use crate::error::{
    ConfigError, Diagnostic, SemanticConfigError, Severity, WindowSizeError,
    WorkspaceInheritanceError,
};
use crate::location::LineIndex;
use crate::project::{CargoProjectFacts, config_file_path, parse_package_version};
use crate::resolved::{
    AppConfig, BundleConfig, DecorationsSetting, DevConfig, FieldProvenance, IconsConfig,
    Provenance, ResolvedConfig, Target, WebConfig, WebIconsConfig, WindowConfig,
};
use crate::schema::{
    RawConfig, RawDecorations, RawVersion, RawWeb, SUPPORTED_SCHEMA_VERSION, SchemaVersionProbe,
};
use std::path::Path;
use toml::Spanned;

#[derive(Debug)]
pub struct Resolution {
    pub config: ResolvedConfig,
    pub provenance: Vec<FieldProvenance>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Resolves `florui.config.toml` (if present) at `facts.package_root`
/// against `facts` and the selected `target`. A missing config file is not
/// an error -- it resolves to Cargo/legacy/built-in defaults, so an
/// unmigrated project keeps working without notice.
pub fn resolve(
    facts: &CargoProjectFacts,
    target: Option<Target>,
) -> Result<Resolution, ConfigError> {
    let config_path = config_file_path(&facts.package_root);
    let source = match std::fs::read_to_string(&config_path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(ConfigError::Io {
                path: config_path,
                source: err,
            });
        }
    };

    let raw = match &source {
        Some(text) => Some(parse_config(text, &config_path)?),
        None => None,
    };
    let lines = source.as_deref().map(LineIndex::new);

    let mut errors: Vec<SemanticConfigError> = Vec::new();

    let new_dev_example = raw
        .as_ref()
        .and_then(|c| c.dev.as_ref())
        .and_then(|d| d.example.as_ref());
    if let (Some(legacy), Some(new_)) = (&facts.legacy_dev_example, new_dev_example) {
        let manifest_lines = LineIndex::new(&facts.manifest_text);
        errors.push(SemanticConfigError::LegacyNewDuplicate {
            field: "dev.example",
            config_path: config_path.clone(),
            new_location: lines.as_ref().unwrap().locate_start(new_.span()),
            legacy_manifest_path: facts.manifest_path.clone(),
            legacy_location: manifest_lines.locate_start(legacy.span.clone()),
        });
    }

    let app_version = raw
        .as_ref()
        .and_then(|c| c.app.as_ref())
        .and_then(|a| a.version.as_ref());
    if let Some(version) = app_version {
        match version {
            RawVersion::Invalid { span } => {
                errors.push(SemanticConfigError::InvalidAppVersion {
                    config_path: config_path.clone(),
                    location: lines.as_ref().unwrap().locate_start(span.clone()),
                });
            }
            RawVersion::Workspace {
                requested: true, ..
            } => {
                if let Err(error) =
                    check_workspace_inheritance(&facts.manifest_path, &facts.manifest_text)
                {
                    errors.push(error);
                }
            }
            RawVersion::Workspace {
                requested: false,
                span,
            } => {
                errors.push(SemanticConfigError::InvalidAppVersion {
                    config_path: config_path.clone(),
                    location: lines.as_ref().unwrap().locate_start(span.clone()),
                });
            }
            RawVersion::Literal { .. } => {}
        }
    }

    if let Some(window) = raw.as_ref().and_then(|c| c.window.as_ref()) {
        let config_lines = lines.as_ref().unwrap();
        validate_window_size(
            window.width.as_ref(),
            "width",
            &config_path,
            config_lines,
            &mut errors,
        );
        validate_window_size(
            window.height.as_ref(),
            "height",
            &config_path,
            config_lines,
            &mut errors,
        );
        validate_window_size(
            window.min_width.as_ref(),
            "min_width",
            &config_path,
            config_lines,
            &mut errors,
        );
        validate_window_size(
            window.min_height.as_ref(),
            "min_height",
            &config_path,
            config_lines,
            &mut errors,
        );
        if let (Some(width), Some(min_width)) = (&window.width, &window.min_width)
            && min_width.get_ref() > width.get_ref()
        {
            errors.push(SemanticConfigError::InvalidWindowSize {
                config_path: config_path.clone(),
                field: "min_width",
                location: config_lines.locate_start(min_width.span()),
                reason: WindowSizeError::MinExceedsMax {
                    min_field: "min_width",
                    max_field: "width",
                },
            });
        }
        if let (Some(height), Some(min_height)) = (&window.height, &window.min_height)
            && min_height.get_ref() > height.get_ref()
        {
            errors.push(SemanticConfigError::InvalidWindowSize {
                config_path: config_path.clone(),
                field: "min_height",
                location: config_lines.locate_start(min_height.span()),
                reason: WindowSizeError::MinExceedsMax {
                    min_field: "min_height",
                    max_field: "height",
                },
            });
        }
    }

    if let Some(web) = raw.as_ref().and_then(|c| c.web.as_ref()) {
        let config_lines = lines.as_ref().unwrap();
        validate_web_base_path(web, &config_path, config_lines, &mut errors);
        validate_web_icon_formats(web, &config_path, config_lines, &mut errors);
    }

    if !errors.is_empty() {
        return Err(ConfigError::Semantic(errors));
    }

    // Every field below is now safe to resolve unconditionally -- nothing
    // that would have blocked resolution survived the validation pass
    // above.
    let raw_app = raw.as_ref().and_then(|c| c.app.as_ref());
    let raw_window = raw.as_ref().and_then(|c| c.window.as_ref());
    let raw_bundle = raw.as_ref().and_then(|c| c.bundle.as_ref());

    let mut provenance = Vec::new();

    let name = match raw_app.and_then(|a| a.name.as_deref()) {
        Some(name) => {
            provenance.push(field("app.name", Provenance::ConfigFile(None)));
            name.to_owned()
        }
        None => {
            provenance.push(field("app.name", Provenance::CargoManifest));
            facts.package_name.clone()
        }
    };

    let version = match raw_app.and_then(|a| a.version.as_ref()) {
        Some(RawVersion::Literal { value, span }) => {
            provenance.push(field(
                "app.version",
                Provenance::ConfigFile(Some(lines.as_ref().unwrap().locate_start(span.clone()))),
            ));
            value.clone()
        }
        Some(RawVersion::Workspace { .. }) => {
            provenance.push(field("app.version", Provenance::CargoManifest));
            facts.package_version.clone()
        }
        Some(RawVersion::Invalid { .. }) => {
            unreachable!("invalid app.version would have errored above")
        }
        None => {
            provenance.push(field("app.version", Provenance::CargoManifest));
            facts.package_version.clone()
        }
    };

    let identifier = optional_string_field(
        raw_app.and_then(|a| a.identifier.as_deref()),
        "app.identifier",
        &mut provenance,
    );
    let description = optional_string_field(
        raw_app.and_then(|a| a.description.as_deref()),
        "app.description",
        &mut provenance,
    );

    let icons_dir = config_path.parent().unwrap_or(&facts.package_root);
    let mut diagnostics = Vec::new();
    let icons = resolve_icons(
        raw_app.and_then(|a| a.icons.as_ref()),
        icons_dir,
        lines.as_ref(),
        target,
        &mut provenance,
        &mut diagnostics,
    );

    let app = AppConfig {
        identifier,
        name,
        description,
        version,
        icons,
    };

    let web = resolve_web(
        raw.as_ref().and_then(|c| c.web.as_ref()),
        &app.name,
        app.description.as_deref(),
        icons_dir,
        lines.as_ref(),
        target,
        &mut provenance,
        &mut diagnostics,
    );

    let title = match raw_window.and_then(|w| w.title.as_deref()) {
        Some(title) => {
            provenance.push(field("window.title", Provenance::ConfigFile(None)));
            title.to_owned()
        }
        None => {
            provenance.push(field("window.title", Provenance::BuiltinDefault));
            app.name.clone()
        }
    };
    let width = spanned_size_field(
        raw_window.and_then(|w| w.width.as_ref()),
        "window.width",
        lines.as_ref(),
        &mut provenance,
    );
    let height = spanned_size_field(
        raw_window.and_then(|w| w.height.as_ref()),
        "window.height",
        lines.as_ref(),
        &mut provenance,
    );
    let min_width = spanned_size_field(
        raw_window.and_then(|w| w.min_width.as_ref()),
        "window.min_width",
        lines.as_ref(),
        &mut provenance,
    );
    let min_height = spanned_size_field(
        raw_window.and_then(|w| w.min_height.as_ref()),
        "window.min_height",
        lines.as_ref(),
        &mut provenance,
    );
    let decorations = match raw_window.and_then(|w| w.decorations.as_ref()) {
        Some(spanned) => {
            provenance.push(field(
                "window.decorations",
                Provenance::ConfigFile(Some(lines.as_ref().unwrap().locate_start(spanned.span()))),
            ));
            match spanned.get_ref() {
                RawDecorations::System => DecorationsSetting::System,
                RawDecorations::Custom => DecorationsSetting::Custom,
            }
        }
        None => {
            provenance.push(field("window.decorations", Provenance::BuiltinDefault));
            DecorationsSetting::System
        }
    };
    let transparent = match raw_window.and_then(|w| w.transparent) {
        Some(value) => {
            provenance.push(field("window.transparent", Provenance::ConfigFile(None)));
            value
        }
        None => {
            provenance.push(field("window.transparent", Provenance::BuiltinDefault));
            false
        }
    };

    let window = WindowConfig {
        title,
        width,
        height,
        min_width,
        min_height,
        decorations,
        transparent,
    };

    let publisher = optional_string_field(
        raw_bundle.and_then(|b| b.publisher.as_deref()),
        "bundle.publisher",
        &mut provenance,
    );
    let bundle = BundleConfig { publisher };

    let example = match new_dev_example {
        Some(spanned) => {
            provenance.push(field(
                "dev.example",
                Provenance::ConfigFile(Some(lines.as_ref().unwrap().locate_start(spanned.span()))),
            ));
            Some(spanned.get_ref().clone())
        }
        None => match &facts.legacy_dev_example {
            Some(located) => {
                provenance.push(field("dev.example", Provenance::LegacyMetadata));
                Some(located.value.clone())
            }
            None => {
                provenance.push(field("dev.example", Provenance::BuiltinDefault));
                None
            }
        },
    };
    let dev = DevConfig { example };

    Ok(Resolution {
        config: ResolvedConfig {
            app,
            window,
            bundle,
            dev,
            web,
        },
        provenance,
        diagnostics,
    })
}

fn field(name: &'static str, provenance: Provenance) -> FieldProvenance {
    FieldProvenance {
        field: name,
        provenance,
    }
}

fn optional_string_field(
    raw: Option<&str>,
    name: &'static str,
    provenance: &mut Vec<FieldProvenance>,
) -> Option<String> {
    match raw {
        Some(value) => {
            provenance.push(field(name, Provenance::ConfigFile(None)));
            Some(value.to_owned())
        }
        None => {
            provenance.push(field(name, Provenance::BuiltinDefault));
            None
        }
    }
}

fn spanned_size_field(
    raw: Option<&toml::Spanned<f64>>,
    name: &'static str,
    lines: Option<&LineIndex<'_>>,
    provenance: &mut Vec<FieldProvenance>,
) -> Option<f64> {
    match raw {
        Some(spanned) => {
            provenance.push(field(
                name,
                Provenance::ConfigFile(Some(lines.unwrap().locate_start(spanned.span()))),
            ));
            Some(*spanned.get_ref())
        }
        None => {
            provenance.push(field(name, Provenance::BuiltinDefault));
            None
        }
    }
}

fn validate_window_size(
    raw: Option<&toml::Spanned<f64>>,
    field_name: &'static str,
    config_path: &Path,
    lines: &LineIndex<'_>,
    errors: &mut Vec<SemanticConfigError>,
) {
    let Some(spanned) = raw else { return };
    let value = *spanned.get_ref();
    let reason = if !value.is_finite() {
        Some(WindowSizeError::NotFinite)
    } else if value <= 0.0 {
        Some(WindowSizeError::NotPositive)
    } else {
        None
    };
    if let Some(reason) = reason {
        errors.push(SemanticConfigError::InvalidWindowSize {
            config_path: config_path.to_owned(),
            field: field_name,
            location: lines.locate_start(spanned.span()),
            reason,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_icons(
    raw: Option<&crate::schema::RawIcons>,
    icons_dir: &Path,
    lines: Option<&LineIndex<'_>>,
    target: Option<Target>,
    provenance: &mut Vec<FieldProvenance>,
    diagnostics: &mut Vec<Diagnostic>,
) -> IconsConfig {
    let checks_assets = matches!(target, Some(Target::Native));
    let mut resolve_one = |raw: Option<&toml::Spanned<String>>,
                           name: &'static str,
                           code: &'static str| match raw {
        Some(spanned) => {
            let location = lines.unwrap().locate_start(spanned.span());
            provenance.push(field(name, Provenance::ConfigFile(Some(location))));
            let path = icons_dir.join(spanned.get_ref());
            if checks_assets && !path.exists() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code,
                    message: format!("{name} points to {}, which does not exist", path.display()),
                    location: Some(location),
                    field: name,
                });
            }
            Some(path)
        }
        None => {
            provenance.push(field(name, Provenance::BuiltinDefault));
            None
        }
    };

    IconsConfig {
        source: resolve_one(
            raw.and_then(|i| i.source.as_ref()),
            "app.icons.source",
            "config.icon_asset_present.source",
        ),
        windows: resolve_one(
            raw.and_then(|i| i.windows.as_ref()),
            "app.icons.windows",
            "config.icon_asset_present.windows",
        ),
        macos: resolve_one(
            raw.and_then(|i| i.macos.as_ref()),
            "app.icons.macos",
            "config.icon_asset_present.macos",
        ),
        linux: resolve_one(
            raw.and_then(|i| i.linux.as_ref()),
            "app.icons.linux",
            "config.icon_asset_present.linux",
        ),
    }
}

fn validate_web_base_path(
    web: &RawWeb,
    config_path: &Path,
    lines: &LineIndex<'_>,
    errors: &mut Vec<SemanticConfigError>,
) {
    let Some(base_path) = web.base_path.as_ref() else {
        return;
    };
    if !base_path.get_ref().starts_with('/') {
        errors.push(SemanticConfigError::InvalidWebBasePath {
            config_path: config_path.to_owned(),
            location: lines.locate_start(base_path.span()),
        });
    }
}

fn validate_web_icon_formats(
    web: &RawWeb,
    config_path: &Path,
    lines: &LineIndex<'_>,
    errors: &mut Vec<SemanticConfigError>,
) {
    let Some(icons) = web.icons.as_ref() else {
        return;
    };
    check_web_icon_extension(
        icons.favicon.as_ref(),
        "favicon",
        &["svg", "png", "ico"],
        config_path,
        lines,
        errors,
    );
    check_web_icon_extension(
        icons.apple_touch_icon.as_ref(),
        "apple_touch_icon",
        &["png"],
        config_path,
        lines,
        errors,
    );
}

fn check_web_icon_extension(
    raw: Option<&Spanned<String>>,
    field_name: &'static str,
    allowed: &'static [&'static str],
    config_path: &Path,
    lines: &LineIndex<'_>,
    errors: &mut Vec<SemanticConfigError>,
) {
    let Some(spanned) = raw else {
        return;
    };
    let lower = spanned.get_ref().to_ascii_lowercase();
    let ok = allowed
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")));
    if !ok {
        errors.push(SemanticConfigError::InvalidWebIconFormat {
            config_path: config_path.to_owned(),
            field: field_name,
            location: lines.locate_start(spanned.span()),
            allowed,
        });
    }
}

/// `web.title`/`web.description` fall back to `app.name`/`app.description`
/// (never native `window.title`) with `Provenance::BuiltinDefault`, the same
/// treatment `window.title`'s own fallback to `app.name` already gets above
/// -- a field inheriting another resolved field's value isn't a distinct
/// provenance category in this model. `web.icons` never inherits
/// `app.icons`: an absent field simply resolves to `None`.
#[allow(clippy::too_many_arguments)]
fn resolve_web(
    raw: Option<&RawWeb>,
    app_name: &str,
    app_description: Option<&str>,
    icons_dir: &Path,
    lines: Option<&LineIndex<'_>>,
    target: Option<Target>,
    provenance: &mut Vec<FieldProvenance>,
    diagnostics: &mut Vec<Diagnostic>,
) -> WebConfig {
    let title = match raw.and_then(|w| w.title.as_deref()) {
        Some(title) => {
            provenance.push(field("web.title", Provenance::ConfigFile(None)));
            title.to_owned()
        }
        None => {
            provenance.push(field("web.title", Provenance::BuiltinDefault));
            app_name.to_owned()
        }
    };
    let description = match raw.and_then(|w| w.description.as_deref()) {
        Some(description) => {
            provenance.push(field("web.description", Provenance::ConfigFile(None)));
            Some(description.to_owned())
        }
        None => {
            provenance.push(field("web.description", Provenance::BuiltinDefault));
            app_description.map(str::to_owned)
        }
    };
    let base_path = match raw.and_then(|w| w.base_path.as_ref()) {
        Some(spanned) => {
            provenance.push(field(
                "web.base_path",
                Provenance::ConfigFile(Some(lines.unwrap().locate_start(spanned.span()))),
            ));
            spanned.get_ref().clone()
        }
        None => {
            provenance.push(field("web.base_path", Provenance::BuiltinDefault));
            "/".to_owned()
        }
    };

    let checks_assets = matches!(target, Some(Target::Web));
    let raw_icons = raw.and_then(|w| w.icons.as_ref());
    let mut resolve_one = |raw: Option<&toml::Spanned<String>>,
                           name: &'static str,
                           code: &'static str| match raw {
        Some(spanned) => {
            let location = lines.unwrap().locate_start(spanned.span());
            provenance.push(field(name, Provenance::ConfigFile(Some(location))));
            let path = icons_dir.join(spanned.get_ref());
            if checks_assets && !path.exists() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code,
                    message: format!("{name} points to {}, which does not exist", path.display()),
                    location: Some(location),
                    field: name,
                });
            }
            Some(path)
        }
        None => {
            provenance.push(field(name, Provenance::BuiltinDefault));
            None
        }
    };
    let icons = WebIconsConfig {
        favicon: resolve_one(
            raw_icons.and_then(|i| i.favicon.as_ref()),
            "web.icons.favicon",
            "config.web_icon_asset_present.favicon",
        ),
        apple_touch_icon: resolve_one(
            raw_icons.and_then(|i| i.apple_touch_icon.as_ref()),
            "web.icons.apple_touch_icon",
            "config.web_icon_asset_present.apple_touch_icon",
        ),
    };

    WebConfig {
        title,
        description,
        base_path,
        icons,
    }
}

/// Two-step parse: peeks `schema_version` first, so a genuinely newer or
/// older schema shape reports a clean "unsupported schema_version" error
/// rather than "unknown field" noise for every field this build doesn't
/// recognize yet. Two `toml::from_str` calls over the same short string is
/// simpler and more honest than trying to reuse one permissive pass.
fn parse_config(text: &str, config_path: &Path) -> Result<RawConfig, ConfigError> {
    let lines = LineIndex::new(text);
    let probe: SchemaVersionProbe =
        toml::from_str(text).map_err(|err| toml_error(config_path, &lines, err))?;
    let version = *probe.schema_version.get_ref();
    if version != SUPPORTED_SCHEMA_VERSION {
        return Err(ConfigError::Semantic(vec![
            SemanticConfigError::UnsupportedSchemaVersion {
                config_path: config_path.to_owned(),
                found: version,
                supported: SUPPORTED_SCHEMA_VERSION,
                location: lines.locate_start(probe.schema_version.span()),
            },
        ]));
    }
    toml::from_str::<RawConfig>(text).map_err(|err| toml_error(config_path, &lines, err))
}

fn toml_error(config_path: &Path, lines: &LineIndex<'_>, err: toml::de::Error) -> ConfigError {
    let location = err.span().map(|span| lines.locate_start(span));
    ConfigError::Toml {
        path: config_path.to_owned(),
        message: err.message().to_owned(),
        location,
    }
}

/// Verifies `manifest_path`'s own `[package]` table actually declares
/// `version.workspace = true` -- `cargo metadata` always reports an
/// already-flattened, concrete version regardless of how it was declared,
/// so this can't be checked from that JSON alone.
pub(crate) fn check_workspace_inheritance(
    manifest_path: &Path,
    manifest_text: &str,
) -> Result<(), SemanticConfigError> {
    let lines = LineIndex::new(manifest_text);
    let not_inherited = |found: String, span: std::ops::Range<usize>| {
        SemanticConfigError::WorkspaceVersionNotInherited {
            manifest_path: manifest_path.to_owned(),
            location: lines.locate_start(span),
            reason: WorkspaceInheritanceError::NotInherited { found },
        }
    };
    match parse_package_version(manifest_text) {
        Ok(Some(RawVersion::Workspace {
            requested: true, ..
        })) => Ok(()),
        Ok(Some(RawVersion::Workspace {
            requested: false,
            span,
        })) => Err(not_inherited("workspace = false".to_owned(), span)),
        Ok(Some(RawVersion::Literal { value, span })) => {
            Err(not_inherited(format!("version = \"{value}\""), span))
        }
        Ok(Some(RawVersion::Invalid { span })) => Err(not_inherited(
            "an unrecognized version value".to_owned(),
            span,
        )),
        Ok(None) | Err(_) => Err(SemanticConfigError::WorkspaceVersionNotInherited {
            manifest_path: manifest_path.to_owned(),
            location: lines.locate(0),
            reason: WorkspaceInheritanceError::MissingVersionKey,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::CargoProjectFacts;

    fn facts(package_root: &Path, manifest_text: &str) -> CargoProjectFacts {
        CargoProjectFacts {
            workspace_root: package_root.to_owned(),
            package_root: package_root.to_owned(),
            manifest_path: package_root.join("Cargo.toml"),
            manifest_text: manifest_text.to_owned(),
            target_dir: package_root.join("target"),
            package_name: "app".to_owned(),
            package_version: "0.1.0".to_owned(),
            example_targets: Vec::new(),
            legacy_dev_example: None,
        }
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn missing_config_file_falls_back_to_defaults_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.app.name, "app");
        assert_eq!(resolution.config.app.version, "0.1.0");
        assert_eq!(resolution.config.window.title, "app");
        assert_eq!(
            resolution.config.window.decorations,
            DecorationsSetting::System
        );
        assert!(!resolution.config.window.transparent);
    }

    #[test]
    fn unsupported_schema_version_is_rejected_with_a_location() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "florui.config.toml", "schema_version = 2\n");
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::UnsupportedSchemaVersion {
                        found: 2,
                        supported: 1,
                        ..
                    }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn unknown_top_level_key_is_a_hard_toml_error_with_a_location() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\nbogus = true\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Toml {
                location: Some(_), ..
            } => {}
            other => panic!("expected Toml with a location, got {other:?}"),
        }
    }

    #[test]
    fn app_name_defaults_to_the_cargo_package_name() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "florui.config.toml", "schema_version = 1\n");
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.app.name, "app");
        assert!(
            resolution
                .provenance
                .iter()
                .any(|p| p.field == "app.name" && p.provenance == Provenance::CargoManifest)
        );
    }

    #[test]
    fn app_name_from_config_takes_precedence_with_config_file_provenance() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\nname = \"Garden\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.app.name, "Garden");
        assert!(
            resolution
                .provenance
                .iter()
                .any(|p| p.field == "app.name" && p.provenance == Provenance::ConfigFile(None))
        );
    }

    #[test]
    fn app_version_literal_string_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\nversion = \"9.9.9\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.app.version, "9.9.9");
    }

    #[test]
    fn app_version_wrong_shape_is_a_semantic_error_with_a_location() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\nversion = 123\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidAppVersion { .. }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn app_version_workspace_true_resolves_to_the_cargo_version_when_manifest_inherits() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\nversion = { workspace = true }\n",
        );
        let manifest = "[package]\nname = \"app\"\nversion.workspace = true\n";
        let resolution = resolve(&facts(dir.path(), manifest), None).unwrap();
        assert_eq!(resolution.config.app.version, "0.1.0");
    }

    #[test]
    fn app_version_workspace_true_errors_when_the_manifest_uses_a_literal_version() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\nversion = { workspace = true }\n",
        );
        // The same shape examples/florui-example-app/Cargo.toml has today.
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let err = resolve(&facts(dir.path(), manifest), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::WorkspaceVersionNotInherited { .. }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn window_size_rejects_non_finite() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[window]\nwidth = nan\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidWindowSize {
                        reason: WindowSizeError::NotFinite,
                        ..
                    }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn window_size_rejects_non_positive() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[window]\nwidth = 0\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidWindowSize {
                        reason: WindowSizeError::NotPositive,
                        ..
                    }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn window_size_rejects_min_exceeding_max() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[window]\nwidth = 400\nmin_width = 800\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidWindowSize {
                        reason: WindowSizeError::MinExceedsMax { .. },
                        ..
                    }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn legacy_and_new_dev_example_both_declared_is_a_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[dev]\nexample = \"counter\"\n",
        );
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[package.metadata.florui.dev]\nexample = \"counter\"\n";
        let mut f = facts(dir.path(), manifest);
        let located = crate::location::LocatedValue {
            value: "counter".to_owned(),
            span: 0..7,
        };
        f.legacy_dev_example = Some(located);
        let err = resolve(&f, None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::LegacyNewDuplicate { .. }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn legacy_only_dev_example_resolves_with_legacy_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = facts(dir.path(), "");
        f.legacy_dev_example = Some(crate::location::LocatedValue {
            value: "counter".to_owned(),
            span: 0..7,
        });
        let resolution = resolve(&f, None).unwrap();
        assert_eq!(resolution.config.dev.example.as_deref(), Some("counter"));
        assert!(
            resolution
                .provenance
                .iter()
                .any(|p| p.field == "dev.example" && p.provenance == Provenance::LegacyMetadata)
        );
    }

    #[test]
    fn new_only_dev_example_resolves_with_config_file_provenance() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[dev]\nexample = \"counter\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.dev.example.as_deref(), Some("counter"));
    }

    #[test]
    fn icon_asset_missing_is_a_warning_diagnostic_not_an_error_under_native() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app.icons]\nsource = \"missing.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), Some(Target::Native)).unwrap();
        assert_eq!(resolution.diagnostics.len(), 1);
        assert_eq!(
            resolution.diagnostics[0].code,
            "config.icon_asset_present.source"
        );
    }

    #[test]
    fn icon_asset_check_is_skipped_under_target_web() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app.icons]\nsource = \"missing.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), Some(Target::Web)).unwrap();
        assert!(resolution.diagnostics.is_empty());
    }

    #[test]
    fn icon_asset_check_is_skipped_when_target_is_none() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app.icons]\nsource = \"missing.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert!(resolution.diagnostics.is_empty());
    }

    #[test]
    fn icon_asset_present_produces_no_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "icon.svg", "<svg/>");
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app.icons]\nsource = \"icon.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), Some(Target::Native)).unwrap();
        assert!(resolution.diagnostics.is_empty());
        assert_eq!(
            resolution.config.app.icons.source,
            Some(dir.path().join("icon.svg"))
        );
    }

    #[test]
    fn check_workspace_inheritance_confirms_a_real_workspace_true_manifest() {
        let manifest = "[package]\nname = \"app\"\nversion.workspace = true\n";
        assert!(check_workspace_inheritance(Path::new("Cargo.toml"), manifest).is_ok());
    }

    #[test]
    fn check_workspace_inheritance_rejects_a_literal_version_string() {
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let err = check_workspace_inheritance(Path::new("Cargo.toml"), manifest).unwrap_err();
        assert!(matches!(
            err,
            SemanticConfigError::WorkspaceVersionNotInherited {
                reason: WorkspaceInheritanceError::NotInherited { .. },
                ..
            }
        ));
    }

    #[test]
    fn web_title_falls_back_to_app_name_not_window_title() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\nname = \"Garden\"\n[window]\ntitle = \"A Different Title\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.web.title, "Garden");
        assert_eq!(resolution.config.window.title, "A Different Title");
    }

    #[test]
    fn web_description_falls_back_to_app_description() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app]\ndescription = \"A workspace for your ideas\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(
            resolution.config.web.description.as_deref(),
            Some("A workspace for your ideas")
        );
    }

    #[test]
    fn web_base_path_defaults_to_root() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "florui.config.toml", "schema_version = 1\n");
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.web.base_path, "/");
    }

    #[test]
    fn web_base_path_missing_leading_slash_is_a_semantic_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[web]\nbase_path = \"garden/\"\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidWebBasePath { .. }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn web_icon_asset_missing_is_a_warning_diagnostic_under_target_web() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[web.icons]\nfavicon = \"missing.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), Some(Target::Web)).unwrap();
        assert_eq!(resolution.diagnostics.len(), 1);
        assert_eq!(
            resolution.diagnostics[0].code,
            "config.web_icon_asset_present.favicon"
        );
    }

    #[test]
    fn web_icon_asset_check_is_skipped_outside_target_web() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[web.icons]\nfavicon = \"missing.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), Some(Target::Native)).unwrap();
        assert!(resolution.diagnostics.is_empty());
    }

    #[test]
    fn invalid_favicon_extension_is_a_semantic_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[web.icons]\nfavicon = \"favicon.gif\"\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidWebIconFormat {
                        field: "favicon",
                        ..
                    }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn invalid_apple_touch_icon_extension_is_a_semantic_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[web.icons]\napple_touch_icon = \"icon.svg\"\n",
        );
        let err = resolve(&facts(dir.path(), ""), None).unwrap_err();
        match err {
            ConfigError::Semantic(errors) => {
                assert!(matches!(
                    errors[0],
                    SemanticConfigError::InvalidWebIconFormat {
                        field: "apple_touch_icon",
                        ..
                    }
                ));
            }
            other => panic!("expected Semantic, got {other:?}"),
        }
    }

    #[test]
    fn absent_web_icons_resolve_to_none_with_no_native_icon_inheritance() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "florui.config.toml",
            "schema_version = 1\n[app.icons]\nsource = \"assets/app.svg\"\n",
        );
        let resolution = resolve(&facts(dir.path(), ""), None).unwrap();
        assert_eq!(resolution.config.web.icons.favicon, None);
        assert_eq!(resolution.config.web.icons.apple_touch_icon, None);
    }
}

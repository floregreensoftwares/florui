//! Typed `florui.config.toml` shape, deserialized straight off `toml`'s
//! own [`toml::Spanned`] where a field needs a source location for later
//! semantic validation (see `resolve.rs`) — everything else is a plain
//! `Option<T>`, already validated for free by `toml`'s own type checking.

use serde::Deserialize;
use std::ops::Range;
use toml::Spanned;

pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

/// First step of the two-step parse (see `resolve.rs`): reads only
/// `schema_version`, ignoring every other key — deliberately not
/// `deny_unknown_fields`, so a newer schema's added fields don't get in the
/// way of first checking whether this build can even understand the file.
#[derive(Deserialize)]
pub(crate) struct SchemaVersionProbe {
    pub(crate) schema_version: Spanned<i64>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    // Already consumed by `SchemaVersionProbe` before this struct is ever
    // parsed; kept (rather than dropped) so `deny_unknown_fields` still
    // recognizes the key instead of rejecting it as unknown.
    #[serde(rename = "schema_version")]
    pub(crate) _schema_version: i64,
    pub(crate) app: Option<RawApp>,
    pub(crate) window: Option<RawWindow>,
    pub(crate) bundle: Option<RawBundle>,
    pub(crate) dev: Option<RawDev>,
    pub(crate) web: Option<RawWeb>,
}

/// `[web]` -- independent browser metadata/assets, never inheriting native
/// `[window]`/`app.icons` (see `resolve.rs`'s `resolve_web`).
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWeb {
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) base_path: Option<Spanned<String>>,
    pub(crate) icons: Option<RawWebIcons>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWebIcons {
    pub(crate) favicon: Option<Spanned<String>>,
    pub(crate) apple_touch_icon: Option<Spanned<String>>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawApp {
    pub(crate) identifier: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) version: Option<RawVersion>,
    pub(crate) icons: Option<RawIcons>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawIcons {
    pub(crate) source: Option<Spanned<String>>,
    pub(crate) windows: Option<Spanned<String>>,
    pub(crate) macos: Option<Spanned<String>>,
    pub(crate) linux: Option<Spanned<String>>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWindow {
    pub(crate) title: Option<String>,
    pub(crate) width: Option<Spanned<f64>>,
    pub(crate) height: Option<Spanned<f64>>,
    pub(crate) min_width: Option<Spanned<f64>>,
    pub(crate) min_height: Option<Spanned<f64>>,
    pub(crate) decorations: Option<Spanned<RawDecorations>>,
    pub(crate) transparent: Option<bool>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RawDecorations {
    System,
    Custom,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBundle {
    pub(crate) publisher: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDev {
    pub(crate) example: Option<Spanned<String>>,
}

/// `app.version`'s two accepted shapes (a literal version string, or
/// `{ workspace = true }`). NOT `#[serde(untagged)]`: an untagged enum
/// whose variants wrap `toml::Spanned<T>` fails to deserialize even the
/// valid cases (confirmed with a standalone experiment before adopting
/// `toml` — every variant's span-capturing `Deserialize` impl reports "did
/// not match any variant" instead of the specific problem). Deserializing
/// into `Spanned<toml::Value>` and dispatching on the value's shape here
/// sidesteps that entirely. `Invalid` is never a deserialize error — a
/// shape this doesn't recognize becomes a semantic error in `resolve.rs`
/// with the same one-place-cites-a-location treatment as every other
/// semantic problem, rather than a raw TOML type error.
#[derive(Debug)]
pub(crate) enum RawVersion {
    Literal { value: String, span: Range<usize> },
    Workspace { requested: bool, span: Range<usize> },
    Invalid { span: Range<usize> },
}

impl<'de> Deserialize<'de> for RawVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let spanned: Spanned<toml::Value> = Deserialize::deserialize(deserializer)?;
        let span = spanned.span();
        Ok(match spanned.into_inner() {
            toml::Value::String(value) => RawVersion::Literal { value, span },
            toml::Value::Table(table) if table.len() == 1 => match table.get("workspace") {
                Some(toml::Value::Boolean(requested)) => RawVersion::Workspace {
                    requested: *requested,
                    span,
                },
                _ => RawVersion::Invalid { span },
            },
            _ => RawVersion::Invalid { span },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_probe_ignores_unrelated_keys() {
        let probe: SchemaVersionProbe =
            toml::from_str("schema_version = 1\n[app]\nname = \"Garden\"\n").unwrap();
        assert_eq!(*probe.schema_version.get_ref(), 1);
    }

    #[test]
    fn raw_config_rejects_an_unknown_top_level_key() {
        let err = toml::from_str::<RawConfig>("schema_version = 1\nbogus = true\n").unwrap_err();
        assert!(err.message().contains("bogus"));
    }

    #[test]
    fn raw_config_rejects_an_unknown_key_in_a_nested_table() {
        let err =
            toml::from_str::<RawConfig>("schema_version = 1\n[window]\nbogus = 1\n").unwrap_err();
        assert!(err.message().contains("bogus"));
    }

    #[test]
    fn raw_version_accepts_a_literal_string() {
        #[derive(Deserialize)]
        struct Probe {
            version: RawVersion,
        }
        let probe: Probe = toml::from_str("version = \"1.2.3\"\n").unwrap();
        match probe.version {
            RawVersion::Literal { value, .. } => assert_eq!(value, "1.2.3"),
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn raw_version_accepts_a_workspace_table() {
        #[derive(Deserialize)]
        struct Probe {
            version: RawVersion,
        }
        let probe: Probe = toml::from_str("version = { workspace = true }\n").unwrap();
        match probe.version {
            RawVersion::Workspace { requested, .. } => assert!(requested),
            _ => panic!("expected Workspace"),
        }
    }

    #[test]
    fn raw_version_marks_an_unrecognized_shape_invalid_rather_than_erroring() {
        #[derive(Deserialize)]
        struct Probe {
            version: RawVersion,
        }
        let probe: Probe = toml::from_str("version = 123\n").unwrap();
        assert!(matches!(probe.version, RawVersion::Invalid { .. }));
    }

    #[test]
    fn invalid_decorations_value_fails_to_parse() {
        let err = toml::from_str::<RawConfig>(
            "schema_version = 1\n[window]\ndecorations = \"diagonal\"\n",
        )
        .unwrap_err();
        assert!(err.span().is_some());
    }
}

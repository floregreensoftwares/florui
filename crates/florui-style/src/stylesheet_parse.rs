//! Parses CSS text via Stylo's real parser — real CSS's own error-recovery
//! model applies: a malformed declaration or rule is skipped, not
//! rejected, matching how a browser behaves, not the previous hand-rolled
//! parser's closed-subset rejection. Selectors and properties this crate
//! doesn't yet render (a child combinator, `border-radius`, `!important`)
//! parse successfully; they simply have no visible effect until
//! `florui-layout`/`florui-paint` grow support for them.

use std::sync::{Arc, LazyLock, Mutex};

use style::media_queries::MediaList;
use style::servo_arc::Arc as StyloArc;
use style::stylesheets::{AllowImportRules, Origin, Stylesheet};

use crate::error::StyleError;
use crate::height_media_adapter::substitute_height_features;
use crate::stylo::shared_lock;

/// Stylo gates `display: grid`/`inline-grid` and the `grid-*` longhands
/// behind a runtime pref, off by default in the `servo` build this crate
/// uses — forced on once, process-wide, before any CSS is parsed.
static GRID_ENABLED: LazyLock<()> =
    LazyLock::new(|| stylo_config::set_bool("layout.grid.enabled", true));

/// `backdrop-filter` shares this pref with several properties this crate
/// doesn't read (`contain`, `mask-image`, `text-overflow`, ...), so
/// enabling it has no other observable effect here.
static BACKDROP_FILTER_ENABLED: LazyLock<()> =
    LazyLock::new(|| stylo_config::set_bool("layout.unimplemented", true));

/// How many distinct substituted-text results a [`RuleKind::HeightSensitive`]
/// keeps parsed at once. A resize only produces a new entry when it
/// crosses a `min-height`/`max-height` breakpoint, so real usage rarely
/// needs more than one or two; this just bounds the worst case.
const HEIGHT_CACHE_CAPACITY: usize = 8;

/// One parsed stylesheet. Opaque: `florui-style` is the only crate that
/// reads what's inside — everything else only holds, clones, and passes
/// this to [`crate::cascade::compute`]. Cloning is cheap and shares the
/// same underlying state (an [`Arc`]), including the height cache below.
#[derive(Clone)]
pub struct Rule(Arc<RuleKind>);

enum RuleKind {
    /// The common case: nothing in the authored CSS could possibly be a
    /// `height` media feature, so this is the same parsed stylesheet
    /// every version of this crate has always produced, reused as-is
    /// regardless of viewport.
    Static(StyloArc<Stylesheet>),
    /// The CSS mentions `height` inside an `@media` prelude — see
    /// [`crate::height_media_adapter`]. Real parsing is deferred to the
    /// first [`Rule::stylesheet`] call for a given viewport height, and
    /// cached by the resulting substituted text: two heights that
    /// resolve every height condition the same way produce identical
    /// text, so they safely share one parse.
    HeightSensitive {
        css: String,
        origin: Origin,
        cache: Mutex<Vec<(String, StyloArc<Stylesheet>)>>,
    },
}

impl Rule {
    pub(crate) fn stylesheet(&self, viewport_height: f32) -> StyloArc<Stylesheet> {
        match &*self.0 {
            RuleKind::Static(sheet) => sheet.clone(),
            RuleKind::HeightSensitive { css, origin, cache } => {
                let substituted = substitute_height_features(css, viewport_height);
                let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(pos) = cache
                    .iter()
                    .position(|(key, _)| key == substituted.as_ref())
                {
                    let (key, sheet) = cache.remove(pos);
                    cache.push((key, sheet.clone()));
                    return sheet;
                }
                let sheet = parse_str(&substituted, *origin);
                if cache.len() >= HEIGHT_CACHE_CAPACITY {
                    cache.remove(0);
                }
                cache.push((substituted.into_owned(), sheet.clone()));
                sheet
            }
        }
    }
}

/// Parses `css` as `Origin::Author` — every application/component
/// stylesheet, always. [`crate::default_stylesheet`] is the only other
/// caller of CSS parsing in this crate, and it needs a different origin
/// (`Origin::UserAgent`), which is why the actual parsing logic lives in
/// [`parse_stylesheet_with_origin`] instead of being inlined here.
pub fn parse_stylesheet(css: &str) -> Result<Vec<Rule>, StyleError> {
    parse_stylesheet_with_origin(css, Origin::Author).map(|rule| vec![rule])
}

pub(crate) fn parse_stylesheet_with_origin(css: &str, origin: Origin) -> Result<Rule, StyleError> {
    LazyLock::force(&GRID_ENABLED);
    LazyLock::force(&BACKDROP_FILTER_ENABLED);
    if css.to_ascii_lowercase().contains("height") {
        return Ok(Rule(Arc::new(RuleKind::HeightSensitive {
            css: css.to_string(),
            origin,
            cache: Mutex::new(Vec::new()),
        })));
    }
    Ok(Rule(Arc::new(RuleKind::Static(parse_str(css, origin)))))
}

fn parse_str(css: &str, origin: Origin) -> StyloArc<Stylesheet> {
    let lock = shared_lock();
    let url = url::Url::parse("about:florui").expect("a fixed, valid URL literal");
    let sheet = Stylesheet::from_str(
        css,
        url.into(),
        origin,
        StyloArc::new(lock.wrap(MediaList::empty())),
        lock.clone(),
        None,
        None,
        style::context::QuirksMode::NoQuirks,
        AllowImportRules::No,
    );
    StyloArc::new(sheet)
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;
    use crate::cascade::{Viewport, compute};
    use crate::color::Rgba;
    use crate::interaction::InteractionState;
    use crate::tree::Arena;

    #[test]
    fn a_class_selector_resolves_a_property() {
        let tree: Element = view! { <div class="card" /> };
        let arena = Arena::build(&tree);
        let rules = parse_stylesheet(".card { background-color: #42734f; }").unwrap();
        let computed = compute(
            &arena,
            &rules,
            &InteractionState::new(),
            Viewport::default(),
        );
        assert_eq!(
            computed[&arena.roots()[0]].background_color,
            Rgba::opaque(0x42, 0x73, 0x4f)
        );
    }

    #[test]
    fn a_stylesheet_can_be_parsed_and_reused_across_multiple_computes() {
        let tree: Element = view! { <div class="card" /> };
        let arena = Arena::build(&tree);
        let rules = parse_stylesheet(".card { background-color: #111111; }").unwrap();
        let first = compute(
            &arena,
            &rules,
            &InteractionState::new(),
            Viewport::default(),
        );
        let second = compute(
            &arena,
            &rules,
            &InteractionState::new(),
            Viewport::default(),
        );
        assert_eq!(
            first[&arena.roots()[0]].background_color,
            second[&arena.roots()[0]].background_color
        );
    }

    #[test]
    fn a_property_florui_does_not_yet_render_still_parses_successfully() {
        // Real CSS: an unrendered property is not a parse error.
        assert!(parse_stylesheet(".a { border: 1px solid black; }").is_ok());
    }

    #[test]
    fn malformed_css_does_not_error_the_whole_stylesheet() {
        // Real CSS's own error-recovery model: an unclosed block or a
        // stray declaration is dropped, not rejected outright.
        assert!(parse_stylesheet(".a { color: #fff;").is_ok());
    }
}

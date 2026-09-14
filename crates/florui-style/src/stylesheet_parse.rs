//! Parses CSS text via Stylo's real parser — real CSS's own error-recovery
//! model applies: a malformed declaration or rule is skipped, not
//! rejected, matching how a browser behaves, not the previous hand-rolled
//! parser's closed-subset rejection. Selectors and properties this crate
//! doesn't yet render (a child combinator, `border-radius`, `!important`)
//! parse successfully; they simply have no visible effect until
//! `florui-layout`/`florui-paint` grow support for them.

use std::sync::LazyLock;

use style::media_queries::MediaList;
use style::servo_arc::Arc as StyloArc;
use style::stylesheets::{AllowImportRules, Origin, Stylesheet};

use crate::error::StyleError;
use crate::stylo::shared_lock;

/// Stylo gates `display: grid`/`inline-grid` and the `grid-*` longhands
/// behind a runtime pref, off by default in the `servo` build this crate
/// uses — forced on once, process-wide, before any CSS is parsed.
static GRID_ENABLED: LazyLock<()> =
    LazyLock::new(|| stylo_config::set_bool("layout.grid.enabled", true));

/// One parsed stylesheet. Opaque: `florui-style` is the only crate that
/// reads what's inside — everything else only holds, clones, and passes
/// this to [`crate::cascade::compute`].
#[derive(Clone)]
pub struct Rule(StyloArc<Stylesheet>);

impl Rule {
    pub(crate) fn stylesheet(&self) -> StyloArc<Stylesheet> {
        self.0.clone()
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
    Ok(Rule(StyloArc::new(sheet)))
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;
    use crate::cascade::compute;
    use crate::color::Rgba;
    use crate::interaction::InteractionState;
    use crate::tree::Arena;

    #[test]
    fn a_class_selector_resolves_a_property() {
        let tree: Element = view! { <div class="card" /> };
        let arena = Arena::build(&tree);
        let rules = parse_stylesheet(".card { background-color: #42734f; }").unwrap();
        let computed = compute(&arena, &rules, &InteractionState::new());
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
        let first = compute(&arena, &rules, &InteractionState::new());
        let second = compute(&arena, &rules, &InteractionState::new());
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

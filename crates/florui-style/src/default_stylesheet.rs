//! The framework's own default element stylesheet — `stylesheets.md`'s
//! cascade-ordering step 1 ("Apply the framework's default element
//! stylesheet at its documented default-style precedence"), which nothing
//! previously registered. Parsed once, as real CSS through the same
//! [`crate::stylesheet_parse`] path an application's own stylesheets go
//! through (so it's checked by the same parser, not hand-built `Rule`s),
//! and always appended to the `Stylist` under `Origin::UserAgent` —
//! Stylo's own cascade-origin support, so an application rule of *any*
//! specificity overrides it, exactly like a real browser's UA stylesheet,
//! without this crate hand-rolling priority.
//!
//! Values below are Chromium's own well-established defaults (its `html.css`
//! / the WHATWG HTML "suggested rendering" defaults both browsers converge
//! on), not invented — `h1`–`h6`'s font-size/margin scale and `p`'s margin
//! are exact matches. Scoped to the tags `product.md` already names as
//! having "contracts defined by Florui" (`div`, `span`, `h2`, `button`),
//! extended to the rest of `h1`–`h6` and `p` for a coherent element set,
//! rather than attempting every HTML element up front.
//!
//! `span { display: inline; }` and `button { display: inline-block; }`
//! resolve to [`crate::cascade::Display::Inline`]/`InlineBlock` (added
//! alongside `florui-layout`'s own real inline formatting context), and
//! genuinely flow inline rather than stacking as blocks — see
//! `florui_layout`'s own module doc for that algorithm's exact, honest
//! bound (one level of mixed inline content; a plain `Inline` child, e.g.
//! a bare `<span>`, doesn't get its own box yet, only `InlineBlock`).
//!
//! `button`'s `1px solid #767676` border approximates Chromium's actual
//! default control border color (a `ButtonBorder`/`buttonborder` system
//! color, not invented) the same way the embedded sans-serif font
//! approximates Arial — a real, fixed value rather than querying the OS's
//! own theme, which this crate has no mechanism for. Default padding isn't
//! added here yet — tracked separately, not attempted in this slice.

use std::sync::LazyLock;

use style::stylesheets::Origin;

use crate::stylesheet_parse::{Rule, parse_stylesheet_with_origin};

const CSS: &str = "
    div, p, h1, h2, h3, h4, h5, h6 {
        display: block;
    }

    span {
        display: inline;
    }

    button {
        display: inline-block;
        border: 1px solid #767676;
    }

    h1, h2, h3, h4, h5, h6 {
        font-weight: bold;
    }

    h1 { font-size: 2em; margin-top: 0.67em; margin-bottom: 0.67em; }
    h2 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em; }
    h3 { font-size: 1.17em; margin-top: 1em; margin-bottom: 1em; }
    h4 { font-size: 1em; margin-top: 1.33em; margin-bottom: 1.33em; }
    h5 { font-size: 0.83em; margin-top: 1.67em; margin-bottom: 1.67em; }
    h6 { font-size: 0.67em; margin-top: 2.33em; margin-bottom: 2.33em; }

    p { margin-top: 1em; margin-bottom: 1em; }
";

/// The parsed default stylesheet, built once and reused — `Rule` wraps a
/// cheaply-`Clone`able `Arc`, so every [`crate::cascade::compute`] call
/// reuses this same parse rather than re-parsing static CSS text on every
/// render.
pub(crate) fn rule() -> Rule {
    static RULE: LazyLock<Rule> = LazyLock::new(|| {
        parse_stylesheet_with_origin(CSS, Origin::UserAgent)
            .expect("the framework's own default stylesheet is always valid CSS")
    });
    RULE.clone()
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use crate::cascade::{Display, compute};
    use crate::interaction::InteractionState;
    use crate::stylesheet_parse::parse_stylesheet;
    use crate::tree::Arena;

    // `to_computed_style`'s margin resolution goes through Stylo's own
    // `em`-to-pixel math, not this crate's — a tight tolerance catches a
    // real regression without chasing float-rounding noise.
    const TOLERANCE: f32 = 0.01;

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < TOLERANCE,
            "{what}: expected {expected}, got {actual}"
        );
    }

    /// `compute` always registers this module's `rule()` under
    /// `Origin::UserAgent` before any author rules
    /// (`crate::stylo::compute_in_layout_state`) — so zero author CSS still
    /// exercises the default stylesheet, not a bare initial-value fallback.
    fn computed_style_of(tag_markup: Element) -> crate::cascade::ComputedStyle {
        let arena = Arena::build(&tag_markup);
        let rules = parse_stylesheet("").unwrap();
        let computed = compute(&arena, &rules, &InteractionState::new());
        computed[&arena.roots()[0]]
    }

    #[test]
    fn div_defaults_to_block_with_zero_author_css() {
        let style = computed_style_of(view! { <div /> });
        assert_eq!(style.display, Display::Block);
    }

    #[test]
    fn p_defaults_to_block_with_one_em_margins() {
        let style = computed_style_of(view! { <p /> });
        assert_eq!(style.display, Display::Block);
        assert_close(style.margin.top.unwrap(), 16.0, "p margin-top");
        assert_close(style.margin.bottom.unwrap(), 16.0, "p margin-bottom");
    }

    /// Nested under a `<div>`, not at the tree root — real CSS blockifies a
    /// root element's `display` regardless of what it's authored as
    /// (<https://drafts.csswg.org/css-display/#blockify>; Stylo applies
    /// this on its own, not something this crate implements), so a bare
    /// `<span>`/`<button>` with no parent would read back as `Block` for a
    /// reason that has nothing to do with this module's own rules — nesting
    /// them is what actually exercises `span { display: inline; }`/
    /// `button { display: inline-block; }`.
    #[test]
    fn span_and_button_resolve_inline_and_inline_block() {
        let tree: Element = view! {
            <div>
                <span />
                <button />
            </div>
        };
        let arena = Arena::build(&tree);
        let div = arena.roots()[0];
        let span = arena.children(div)[0];
        let button = arena.children(div)[1];
        let rules = parse_stylesheet("").unwrap();
        let computed = compute(&arena, &rules, &InteractionState::new());

        assert_eq!(computed[&span].display, Display::Inline);
        assert_eq!(computed[&button].display, Display::InlineBlock);
    }

    #[test]
    fn button_resolves_a_visible_default_border_on_every_side_with_zero_author_css() {
        let tree: Element = view! {
            <div>
                <button />
            </div>
        };
        let arena = Arena::build(&tree);
        let div = arena.roots()[0];
        let button = arena.children(div)[0];
        let rules = parse_stylesheet("").unwrap();
        let computed = compute(&arena, &rules, &InteractionState::new());

        let border = computed[&button].border;
        for side in [border.top, border.right, border.bottom, border.left] {
            assert_close(side.width, 1.0, "button border-width");
            assert_eq!(side.color, crate::color::Rgba::opaque(0x76, 0x76, 0x76));
        }
    }

    #[test]
    fn div_has_no_default_border() {
        let style = computed_style_of(view! { <div /> });
        for side in [
            style.border.top,
            style.border.right,
            style.border.bottom,
            style.border.left,
        ] {
            assert_eq!(side.width, 0.0, "div has no default border, unlike button");
        }
    }

    #[test]
    fn h1_through_h6_resolve_the_chromium_font_size_and_margin_scale() {
        let cases: [(Element, f32, f32); 6] = [
            (view! { <h1 /> }, 32.0, 21.44),
            (view! { <h2 /> }, 24.0, 19.92),
            (view! { <h3 /> }, 18.72, 18.72),
            (view! { <h4 /> }, 16.0, 21.28),
            (view! { <h5 /> }, 13.28, 22.1776),
            (view! { <h6 /> }, 10.72, 24.9776),
        ];
        for (markup, expected_font_size, expected_margin) in cases {
            let style = computed_style_of(markup);
            assert_eq!(style.display, Display::Block);
            assert_close(style.font_size, expected_font_size, "font-size");
            assert_eq!(style.font_weight, 700.0, "h1-h6 are bold by default");
            assert_close(style.margin.top.unwrap(), expected_margin, "margin-top");
            assert_close(
                style.margin.bottom.unwrap(),
                expected_margin,
                "margin-bottom",
            );
        }
    }

    #[test]
    fn p_and_div_are_not_bold_by_default() {
        assert_eq!(computed_style_of(view! { <div /> }).font_weight, 400.0);
        assert_eq!(computed_style_of(view! { <p /> }).font_weight, 400.0);
    }

    /// The actual point of `Origin::UserAgent`: an author rule overrides
    /// the default stylesheet regardless of specificity, not merely because
    /// it happens to match with equal or higher specificity. `*` has zero
    /// specificity — far below `h1`'s type-selector specificity — yet still
    /// wins, because origin precedence outranks specificity in the real CSS
    /// cascade. If this crate ever regressed to a hand-rolled "last one
    /// wins" merge instead of real `Origin`-aware cascading, this is the
    /// test that would catch it; equal-specificity author-vs-author cases
    /// wouldn't.
    #[test]
    fn a_lower_specificity_author_rule_still_overrides_the_higher_specificity_default_rule() {
        let tree: Element = view! { <h1 /> };
        let arena = Arena::build(&tree);
        let rules = parse_stylesheet("* { font-size: 40px; }").unwrap();
        let computed = compute(&arena, &rules, &InteractionState::new());
        let style = computed[&arena.roots()[0]];
        assert_close(style.font_size, 40.0, "author-overridden font-size");
    }
}

//! The used-value types every other crate consumes — [`ComputedStyle`],
//! [`Edges`] — and [`compute`], the entry point that resolves them for
//! every node in a tree. The actual selector matching, cascade, and
//! inheritance behind `compute` is Stylo's, in [`crate::stylo`]; this
//! module owns the public shape, not the resolution logic.

use std::collections::HashMap;

use crate::color::Rgba;
use crate::interaction::InteractionState;
use crate::stylesheet_parse::Rule;
use crate::stylo;
use crate::tree::{Arena, NodeId};

/// One edge's value on each of the four sides of the box, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

/// Which of this crate's two embedded font families to shape/measure text
/// with — real CSS's own `font-family` is a whole comma-separated
/// preference list of specific names and generics; this crate only
/// distinguishes the one pair it actually has fonts for. A specific named
/// family (`"Helvetica"`, `"Georgia"`) it can't back with an embedded font
/// resolves to [`Self::SansSerif`], the same as an unspecified
/// `font-family` — real CSS's own initial value is itself UA-dependent,
/// and a browser's is typically a sans-serif system font (often Arial on
/// Windows); this crate's stand-in for that default is its own embedded
/// Open Sans, not a redistribution of Arial itself (proprietary, so this
/// crate cannot embed it) and not metrically matched to it either — see
/// `florui_text`'s own `fonts/NOTICE.md` for that tradeoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontFamily {
    #[default]
    SansSerif,
    Monospace,
}

/// How this node participates in its parent's formatting context
/// (`Block`/`Inline`/`InlineBlock`, real CSS's `display-outside` plus
/// `inline-block`'s special case), *and*, for `Block`/`Flex`/`Grid`, which
/// algorithm lays out its own children (real CSS's `display-inside`) —
/// this crate conflates the two into one field rather than splitting them
/// the way real CSS's two-value `display` syntax does, since nothing here
/// yet needs an `outside`/`inside` combination beyond the five this enum
/// already names. `Inline`'s own children (if it somehow has element
/// children, not just text) and `InlineBlock`'s own children both use the
/// same `Block` algorithm real CSS itself uses for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Display {
    #[default]
    Block,
    Flex,
    /// `display: inline` — participates in a surrounding inline formatting
    /// context (mixed with text and other inline-level siblings, wrapping
    /// at the container's available width) rather than stacking as its own
    /// block. See `florui-layout`'s own module docs for this slice's
    /// documented bounds (one level of mixed inline content, no bidi, no
    /// `vertical-align` beyond baseline).
    Inline,
    /// `display: inline-block` — participates inline like [`Self::Inline`],
    /// but as a single opaque box sized from its own content (like a block
    /// box would be), not fragmented across lines.
    InlineBlock,
    Grid,
}

/// One track's sizing function, from `grid-template-columns`/`-rows` —
/// bounded to what a single (non-`repeat()`) track can be: `repeat()`,
/// named lines, `grid-template-areas`, and `fit-content()`/`minmax()`
/// beyond their max side aren't resolved here yet (real CSS still cascades
/// and parses them through Stylo; this crate just doesn't read them back).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrackSize {
    Length(f32),
    Fr(f32),
    Auto,
    MinContent,
    MaxContent,
}

/// One line of a `grid-column`/`grid-row` placement — `grid-*-start`/`-end`
/// each resolve to one of these. Named lines aren't resolved (a `<custom-
/// ident>` falls back to [`Self::Auto`], the same as an unrecognized name
/// would in real CSS once no line actually carries that name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridPlacement {
    #[default]
    Auto,
    Line(i16),
    Span(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

/// Shared by `justify-content`/`align-content` — real CSS resolves both
/// through the same `content-distribution` value space. `None` means
/// `normal`: packed at the start with no extra distribution, and (unlike
/// every other `Option<...>` field here) not the same as an explicit
/// `flex-start`, since `normal` is genuinely a distinct initial value with
/// no equivalent keyword of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentAlignment {
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Shared by `align-items`/`align-self` — same reasoning as
/// [`ContentAlignment`] for why this is `Option`, not a `#[default]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAlignment {
    Stretch,
    FlexStart,
    FlexEnd,
    Start,
    End,
    Center,
    Baseline,
}

/// One side's resolved border — solid-only, the minimum needed for a
/// visible default control outline (`stylesheets.md`'s own scope for this
/// property). Real CSS's other border styles (`dashed`, `dotted`, `double`,
/// …) still parse and cascade correctly through Stylo; this crate paints
/// every non-`none`/`hidden` style as a plain solid line, the same
/// "supported syntax, simplified rendering" tradeoff `stylesheet_parse`'s
/// own doc already documents for other unrendered CSS. `width` is always
/// `0.0` for `border-style: none`/`hidden` (real CSS's own initial style,
/// which makes a border invisible regardless of its width/color) — a
/// zero-width side needs no separate "is it visible" flag downstream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderSide {
    pub width: f32,
    pub color: Rgba,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    pub background_color: Rgba,
    pub color: Rgba,
    /// `None` means `auto` — or, since real CSS now parses here, any
    /// value this crate can't yet resolve to a concrete pixel length
    /// (a percentage, a `calc()`); see [`crate::stylo`]'s conversion
    /// notes.
    pub width: Option<f32>,
    /// `None` means `auto`; see [`Self::width`].
    pub height: Option<f32>,
    /// Each edge is `None` for an explicit `auto` (enabling the usual
    /// auto-margin centering behavior), `Some(0.0)` when nothing set it.
    pub margin: Edges<Option<f32>>,
    /// Always a concrete value; real CSS padding has no `auto`.
    pub padding: Edges<f32>,
    /// Inherits; initial `16.0`.
    pub font_size: f32,
    /// Inherits; initial [`FontFamily::SansSerif`]. See [`FontFamily`]'s
    /// own doc for what "resolves to" means here.
    pub font_family: FontFamily,
    /// Inherits; initial `400.0` (`normal`), CSS's numeric 1–1000 scale.
    pub font_weight: f32,
    /// This node's own layout algorithm, applied to *its children* — a
    /// leaf's `display` never affects how its own box is placed by its
    /// parent (that's [`Self::flex_grow`]/[`Self::flex_shrink`]/
    /// [`Self::flex_basis`]/[`Self::align_self`] instead).
    pub display: Display,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    /// `justify-content`: main-axis distribution of this flex container's
    /// own children. Meaningless when [`Self::display`] isn't
    /// [`Display::Flex`].
    pub justify_content: Option<ContentAlignment>,
    /// `align-content`: cross-axis distribution across wrapped flex lines.
    /// Meaningless when [`Self::display`] isn't [`Display::Flex`].
    pub align_content: Option<ContentAlignment>,
    /// `align-items`: this flex container's default cross-axis alignment
    /// for its children, unless a child overrides it with
    /// [`Self::align_self`]. Meaningless when [`Self::display`] isn't
    /// [`Display::Flex`].
    pub align_items: Option<ItemAlignment>,
    /// `align-self`: this node's *own* cross-axis alignment within
    /// whichever flex container it's a child of, overriding that
    /// container's [`Self::align_items`]. Meaningless when this node's
    /// parent isn't a flex container.
    pub align_self: Option<ItemAlignment>,
    /// How much of a flex container's remaining free space this node
    /// claims, relative to its flex siblings. `0.0` (the CSS initial
    /// value) means it does not grow.
    pub flex_grow: f32,
    /// How much this node shrinks when a flex container's children
    /// collectively overflow it, relative to its flex siblings. `1.0` is
    /// the CSS initial value — flex items shrink by default.
    pub flex_shrink: f32,
    /// The size a flex item starts from before growing/shrinking
    /// distributes remaining space. `None` means `auto` (fall back to
    /// [`Self::width`]/[`Self::height`], on whichever axis is the main
    /// one).
    pub flex_basis: Option<f32>,
    /// `column-gap`, between adjacent children on the main axis for a row
    /// flex container (or the cross axis for a column one).
    pub column_gap: f32,
    /// `row-gap`, the same on the other axis.
    pub row_gap: f32,
    /// `border-*-width`/`-style`/`-color` per side — see [`BorderSide`]'s
    /// own doc for the solid-only scope and the `none`/`hidden` collapse.
    pub border: Edges<BorderSide>,
    /// `grid-template-columns`. Meaningless when [`Self::display`] isn't
    /// [`Display::Grid`]. See [`GridTrackSize`]'s own doc for the bound.
    pub grid_template_columns: Vec<GridTrackSize>,
    /// `grid-template-rows`, the same on the other axis.
    pub grid_template_rows: Vec<GridTrackSize>,
    /// `grid-column-start`/`grid-column-end`.
    pub grid_column: (GridPlacement, GridPlacement),
    /// `grid-row-start`/`grid-row-end`.
    pub grid_row: (GridPlacement, GridPlacement),
}

/// Resolves every node in `arena` against `rules` and `state` — real
/// selector matching, cascade, and inheritance, via Stylo.
pub fn compute(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
) -> HashMap<NodeId, ComputedStyle> {
    stylo::compute(arena, rules, state)
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;
    use crate::stylesheet_parse::parse_stylesheet;

    fn styles(
        tree: &Element,
        css: &str,
        state: &InteractionState,
    ) -> (Arena, HashMap<NodeId, ComputedStyle>) {
        let arena = Arena::build(tree);
        let rules = parse_stylesheet(css).unwrap();
        let computed = compute(&arena, &rules, state);
        (arena, computed)
    }

    #[test]
    fn a_matching_class_rule_sets_background_color() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, computed) = styles(
            &tree,
            ".card { background-color: #1e1e22; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].background_color,
            Rgba::opaque(0x1e, 0x1e, 0x22)
        );
    }

    #[test]
    fn background_color_does_not_inherit() {
        let tree: Element = view! {
            <div class="card">
                <span>{"x"}</span>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".card { background-color: #1e1e22; }",
            &InteractionState::new(),
        );
        let span = arena.find(|a, id| a.tag(id) == "span").unwrap();
        assert_eq!(computed[&span].background_color, Rgba::TRANSPARENT);
    }

    #[test]
    fn color_inherits_by_default() {
        let tree: Element = view! {
            <div class="card">
                <span>{"x"}</span>
            </div>
        };
        let (arena, computed) =
            styles(&tree, ".card { color: #ffffff; }", &InteractionState::new());
        let span = arena.find(|a, id| a.tag(id) == "span").unwrap();
        assert_eq!(computed[&span].color, Rgba::opaque(0xff, 0xff, 0xff));
    }

    #[test]
    fn explicit_inherit_pulls_a_non_inheriting_property_from_the_parent() {
        let tree: Element = view! {
            <div class="card">
                <span class="mirror">{"x"}</span>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".card { background-color: #1e1e22; } .mirror { background-color: inherit; }",
            &InteractionState::new(),
        );
        let span = arena.find(|a, id| a.tag(id) == "span").unwrap();
        assert_eq!(
            computed[&span].background_color,
            Rgba::opaque(0x1e, 0x1e, 0x22)
        );
    }

    #[test]
    fn higher_specificity_wins_regardless_of_source_order() {
        let tree: Element = view! { <div id="main" class="card" /> };
        let (arena, computed) = styles(
            &tree,
            "#main { background-color: #ff0000; } .card { background-color: #00ff00; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].background_color, Rgba::opaque(0xff, 0, 0));
    }

    #[test]
    fn later_source_order_wins_a_specificity_tie() {
        let tree: Element = view! { <div class="a b" /> };
        let (arena, computed) = styles(
            &tree,
            ".a { background-color: #ff0000; } .b { background-color: #00ff00; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].background_color, Rgba::opaque(0, 0xff, 0));
    }

    #[test]
    fn hover_state_overrides_the_base_rule_via_higher_specificity() {
        let tree: Element = view! { <button class="primary">{"Go"}</button> };
        let css =
            ".primary { background-color: #42734f; } .primary:hover { background-color: #345c3e; }";

        let (arena, base) = styles(&tree, css, &InteractionState::new());
        let button = arena.roots()[0];
        assert_eq!(
            base[&button].background_color,
            Rgba::opaque(0x42, 0x73, 0x4f)
        );

        let hovered_state = InteractionState::new().with_hovered(button);
        let hovered = compute(
            &arena,
            &crate::stylesheet_parse::parse_stylesheet(css).unwrap(),
            &hovered_state,
        );
        assert_eq!(
            hovered[&button].background_color,
            Rgba::opaque(0x34, 0x5c, 0x3e)
        );
    }

    #[test]
    fn unmatched_node_gets_initial_values() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(
            &tree,
            ".unused { background-color: #ff0000; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].background_color, Rgba::TRANSPARENT);
        assert_eq!(computed[&node].color, Rgba::opaque(0, 0, 0));
    }

    #[test]
    fn width_and_height_default_to_auto() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].width, None);
        assert_eq!(computed[&node].height, None);
    }

    #[test]
    fn explicit_size_and_box_model_resolve_correctly() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, computed) = styles(
            &tree,
            ".card { width: 200px; height: 100px; padding-top: 8px; margin-left: 4px; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        let style = &computed[&node];
        assert_eq!(style.width, Some(200.0));
        assert_eq!(style.height, Some(100.0));
        assert_eq!(style.padding.top, 8.0);
        assert_eq!(style.padding.left, 0.0, "unset padding edges default to 0");
        assert_eq!(style.margin.left, Some(4.0));
        assert_eq!(
            style.margin.top,
            Some(0.0),
            "unset margin edges default to 0, not auto"
        );
    }

    #[test]
    fn explicit_auto_margin_is_distinguishable_from_unset() {
        let tree: Element = view! { <div class="centered" /> };
        let (arena, computed) = styles(
            &tree,
            ".centered { margin-left: auto; margin-right: auto; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].margin.left, None);
        assert_eq!(computed[&node].margin.right, None);
    }

    #[test]
    fn size_and_box_properties_do_not_inherit() {
        let tree: Element = view! {
            <div class="card">
                <span>{"x"}</span>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".card { width: 200px; padding-top: 8px; }",
            &InteractionState::new(),
        );
        let span = arena.find(|a, id| a.tag(id) == "span").unwrap();
        assert_eq!(computed[&span].width, None);
        assert_eq!(computed[&span].padding.top, 0.0);
    }

    #[test]
    fn font_size_inherits_but_an_explicit_value_overrides_it() {
        let tree: Element = view! {
            <div class="card">
                <span>{"inherited"}</span>
                <span class="big">{"overridden"}</span>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".card { font-size: 24px; } .big { font-size: 32px; }",
            &InteractionState::new(),
        );
        let card = arena.roots()[0];
        assert_eq!(computed[&card].font_size, 24.0);

        let mut spans = arena.children(card).iter().copied();
        let plain = spans.next().unwrap();
        let big = spans.next().unwrap();
        assert_eq!(computed[&plain].font_size, 24.0);
        assert_eq!(computed[&big].font_size, 32.0);
    }

    #[test]
    fn font_weight_defaults_to_400_and_resolves_bold_from_real_css() {
        let tree: Element = view! { <div class="bold" /> };
        let (arena, computed) = styles(
            &tree,
            ".bold { font-weight: bold; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].font_weight, 700.0);

        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        assert_eq!(computed[&arena.roots()[0]].font_weight, 400.0);
    }

    #[test]
    fn font_size_defaults_to_sixteen_pixels() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].font_size, 16.0);
    }

    #[test]
    fn descendant_combinator_matches_any_depth_not_just_direct_children() {
        let tree: Element = view! {
            <div class="card">
                <div>
                    <button>{"Go"}</button>
                </div>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".card button { background-color: #42734f; }",
            &InteractionState::new(),
        );
        let button = arena.find(|a, id| a.tag(id) == "button").unwrap();
        assert_eq!(
            computed[&button].background_color,
            Rgba::opaque(0x42, 0x73, 0x4f)
        );
    }

    #[test]
    fn descendant_combinator_requires_the_ancestor_to_exist() {
        let tree: Element = view! {
            <div>
                <button>{"Go"}</button>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".card button { background-color: #42734f; }",
            &InteractionState::new(),
        );
        let button = arena.find(|a, id| a.tag(id) == "button").unwrap();
        assert_eq!(computed[&button].background_color, Rgba::TRANSPARENT);
    }

    #[test]
    fn display_defaults_to_block() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].display, Display::Block);
    }

    #[test]
    fn display_flex_is_read_back_from_real_css() {
        let tree: Element = view! { <div class="row" /> };
        let (arena, computed) = styles(&tree, ".row { display: flex; }", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].display, Display::Flex);
    }

    #[test]
    fn display_does_not_inherit() {
        let tree: Element = view! {
            <div class="row">
                <span>{"x"}</span>
            </div>
        };
        let (arena, computed) = styles(&tree, ".row { display: flex; }", &InteractionState::new());
        let span = arena.find(|a, id| a.tag(id) == "span").unwrap();
        assert_eq!(
            computed[&span].display,
            Display::Block,
            "a flex container's own display must not leak onto its children"
        );
    }

    #[test]
    fn flex_direction_and_wrap_resolve_from_real_css() {
        let tree: Element = view! { <div class="row" /> };
        let (arena, computed) = styles(
            &tree,
            ".row { flex-direction: column-reverse; flex-wrap: wrap; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].flex_direction, FlexDirection::ColumnReverse);
        assert_eq!(computed[&node].flex_wrap, FlexWrap::Wrap);
    }

    #[test]
    fn flex_direction_and_wrap_default_to_row_and_nowrap() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].flex_direction, FlexDirection::Row);
        assert_eq!(computed[&node].flex_wrap, FlexWrap::NoWrap);
    }

    #[test]
    fn justify_content_and_align_items_resolve_from_real_css() {
        let tree: Element = view! { <div class="row" /> };
        let (arena, computed) = styles(
            &tree,
            ".row { justify-content: space-between; align-items: center; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].justify_content,
            Some(ContentAlignment::SpaceBetween)
        );
        assert_eq!(computed[&node].align_items, Some(ItemAlignment::Center));
    }

    #[test]
    fn justify_content_and_align_items_default_to_none() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].justify_content, None,
            "CSS's own initial value is `normal`, not an explicit keyword"
        );
        assert_eq!(computed[&node].align_items, None);
    }

    #[test]
    fn align_self_resolves_independently_of_the_parents_align_items() {
        let tree: Element = view! {
            <div class="row">
                <span class="odd-one-out">{"x"}</span>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".row { align-items: flex-start; } .odd-one-out { align-self: flex-end; }",
            &InteractionState::new(),
        );
        let span = arena.find(|a, id| a.tag(id) == "span").unwrap();
        assert_eq!(computed[&span].align_self, Some(ItemAlignment::FlexEnd));
    }

    #[test]
    fn flex_grow_shrink_and_basis_resolve_from_real_css() {
        let tree: Element = view! { <div class="item" /> };
        let (arena, computed) = styles(
            &tree,
            ".item { flex-grow: 2; flex-shrink: 0; flex-basis: 50px; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].flex_grow, 2.0);
        assert_eq!(computed[&node].flex_shrink, 0.0);
        assert_eq!(computed[&node].flex_basis, Some(50.0));
    }

    #[test]
    fn flex_grow_shrink_and_basis_default_to_css_initial_values() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].flex_grow, 0.0, "CSS's own initial value");
        assert_eq!(
            computed[&node].flex_shrink, 1.0,
            "flex items shrink by default in real CSS"
        );
        assert_eq!(computed[&node].flex_basis, None, "auto by default");
    }

    #[test]
    fn gap_resolves_from_real_css() {
        let tree: Element = view! { <div class="row" /> };
        let (arena, computed) = styles(
            &tree,
            ".row { column-gap: 12px; row-gap: 4px; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].column_gap, 12.0);
        assert_eq!(computed[&node].row_gap, 4.0);
    }

    #[test]
    fn gap_defaults_to_zero() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].column_gap, 0.0);
        assert_eq!(computed[&node].row_gap, 0.0);
    }

    #[test]
    fn font_family_defaults_to_sans_serif() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].font_family, FontFamily::SansSerif);
    }

    #[test]
    fn font_family_monospace_is_read_back_from_real_css() {
        let tree: Element = view! { <div class="code" /> };
        let (arena, computed) = styles(
            &tree,
            ".code { font-family: monospace; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].font_family, FontFamily::Monospace);
    }

    #[test]
    fn font_family_named_falls_back_to_sans_serif() {
        // This crate has no embedded font backing an arbitrary requested
        // name — it must fall back to its own default rather than erroring
        // or silently picking something else unpredictable.
        let tree: Element = view! { <div class="fancy" /> };
        let (arena, computed) = styles(
            &tree,
            ".fancy { font-family: Helvetica; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].font_family, FontFamily::SansSerif);
    }

    #[test]
    fn font_family_inherits_but_an_explicit_value_overrides_it() {
        let tree: Element = view! {
            <div class="code">
                <span>{"inherited"}</span>
                <span class="prose">{"overridden"}</span>
            </div>
        };
        let (arena, computed) = styles(
            &tree,
            ".code { font-family: monospace; } .prose { font-family: sans-serif; }",
            &InteractionState::new(),
        );
        let code = arena.roots()[0];
        assert_eq!(computed[&code].font_family, FontFamily::Monospace);

        let mut spans = arena.children(code).iter().copied();
        let inherited = spans.next().unwrap();
        let overridden = spans.next().unwrap();
        assert_eq!(computed[&inherited].font_family, FontFamily::Monospace);
        assert_eq!(computed[&overridden].font_family, FontFamily::SansSerif);
    }

    #[test]
    fn border_resolves_width_and_color_per_side_from_real_css() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, computed) = styles(
            &tree,
            ".card { border-top-width: 2px; border-top-style: solid; border-top-color: #ff0000; \
             border-left-width: 3px; border-left-style: solid; border-left-color: #00ff00; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        let border = computed[&node].border;
        assert_eq!(border.top.width, 2.0);
        assert_eq!(border.top.color, Rgba::opaque(0xff, 0x00, 0x00));
        assert_eq!(border.left.width, 3.0);
        assert_eq!(border.left.color, Rgba::opaque(0x00, 0xff, 0x00));
    }

    /// Real CSS's own initial `border-style` is `none`, which makes a
    /// border invisible regardless of any `border-width`/`border-color`
    /// also set — an explicit width with no style set must still resolve
    /// to a `0.0`-width side, not a visible one.
    #[test]
    fn a_border_with_no_style_declared_is_invisible_despite_an_explicit_width() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, computed) = styles(
            &tree,
            ".card { border-top-width: 5px; border-top-color: #ff0000; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].border.top.width, 0.0,
            "no border-style means border-style: none, which is always invisible"
        );
    }

    /// `border-style: none` explicitly set must behave the same as never
    /// setting a style at all.
    #[test]
    fn a_border_explicitly_set_to_none_is_invisible() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, computed) = styles(
            &tree,
            ".card { border-top-width: 5px; border-top-style: none; border-top-color: #ff0000; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(computed[&node].border.top.width, 0.0);
    }

    #[test]
    fn border_color_of_currentcolor_resolves_against_this_elements_own_color() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, computed) = styles(
            &tree,
            ".card { color: #123456; border-top-width: 1px; border-top-style: solid; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].border.top.color,
            Rgba::opaque(0x12, 0x34, 0x56),
            "no border-color declared means currentcolor, the real CSS initial value"
        );
    }

    #[test]
    fn border_defaults_to_invisible_on_every_side_with_zero_author_css() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        let border = computed[&node].border;
        assert_eq!(border.top.width, 0.0);
        assert_eq!(border.right.width, 0.0);
        assert_eq!(border.bottom.width, 0.0);
        assert_eq!(border.left.width, 0.0);
    }

    #[test]
    fn display_grid_resolves_from_real_css() {
        let tree: Element = view! { <div class="g" /> };
        let (arena, computed) = styles(&tree, ".g { display: grid; }", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].display, Display::Grid);
    }

    #[test]
    fn grid_template_columns_resolves_lengths_and_fr_units() {
        let tree: Element = view! { <div class="g" /> };
        let (arena, computed) = styles(
            &tree,
            ".g { display: grid; grid-template-columns: 100px 1fr 2fr; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].grid_template_columns,
            vec![
                GridTrackSize::Length(100.0),
                GridTrackSize::Fr(1.0),
                GridTrackSize::Fr(2.0),
            ]
        );
    }

    #[test]
    fn grid_template_rows_resolves_auto_and_keyword_tracks() {
        let tree: Element = view! { <div class="g" /> };
        let (arena, computed) = styles(
            &tree,
            ".g { display: grid; grid-template-rows: auto min-content max-content; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].grid_template_rows,
            vec![
                GridTrackSize::Auto,
                GridTrackSize::MinContent,
                GridTrackSize::MaxContent,
            ]
        );
    }

    #[test]
    fn grid_template_tracks_default_to_empty_with_zero_author_css() {
        let tree: Element = view! { <div class="g" /> };
        let (arena, computed) = styles(&tree, ".g { display: grid; }", &InteractionState::new());
        let node = arena.roots()[0];
        assert!(computed[&node].grid_template_columns.is_empty());
        assert!(computed[&node].grid_template_rows.is_empty());
    }

    #[test]
    fn grid_column_and_row_resolve_line_and_span_placement() {
        let tree: Element = view! { <div class="item" /> };
        let (arena, computed) = styles(
            &tree,
            ".item { grid-column: 2 / 4; grid-row: 1 / span 2; }",
            &InteractionState::new(),
        );
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].grid_column,
            (GridPlacement::Line(2), GridPlacement::Line(4))
        );
        assert_eq!(
            computed[&node].grid_row,
            (GridPlacement::Line(1), GridPlacement::Span(2))
        );
    }

    #[test]
    fn grid_column_and_row_default_to_auto() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(
            computed[&node].grid_column,
            (GridPlacement::Auto, GridPlacement::Auto)
        );
        assert_eq!(
            computed[&node].grid_row,
            (GridPlacement::Auto, GridPlacement::Auto)
        );
    }
}

//! Turns a [`florui_style::Arena`] plus its computed styles into geometry,
//! via [Taffy](https://github.com/DioxusLabs/taffy) — validated against
//! Taffy 0.14's actual current API rather than assumed from memory.
//!
//! # Scope
//!
//! Block and flex layout — a node's own `display` selects which algorithm
//! lays out *its children*; a node's own box within its *parent* additionally
//! depends on `flex-grow`/`flex-shrink`/`flex-basis`/`align-self` when that
//! parent is a flex container, regardless of the node's own `display`. Grid
//! is not implemented yet, even though Taffy itself already supports it —
//! `florui-style` has no `grid-*` properties to translate from.
//!
//! `width`/`height` are content-box, explicitly, since that is real CSS's
//! actual default (before any reset stylesheet opts into border-box) —
//! Taffy's own default is border-box and silently relying on that would
//! make an explicit size shrink to fit its own padding instead of the
//! padding adding to it. `florui-style` has no `box-sizing` property to
//! override this yet.
//!
//! A leaf node (no element children) with its own direct text is measured
//! via [`florui_text`] — real shaping, not a guess — and that intrinsic
//! size is used wherever the node's own `width`/`height` don't already
//! settle it. A leaf with no text still lays out at `0x0` when it has no
//! explicit size, since there is nothing to measure it from.
//!
//! Taffy positions are relative to the parent's content box, matching
//! Taffy's own convention; see [`absolute_position`] to accumulate them
//! into a position relative to the layout root.

use std::collections::HashMap;

use florui_style::{
    Arena, ComputedStyle, ContentAlignment, Display as StyleDisplay,
    FlexDirection as StyleFlexDirection, FlexWrap as StyleFlexWrap, ItemAlignment, NodeId,
};
use taffy::prelude::*;
use taffy::{Baselines, compute_leaf_layout};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxLayout {
    /// Relative to the parent's content box (`(0, 0)` for a root).
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug)]
pub struct LayoutError(taffy::TaffyError);

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "layout failed: {}", self.0)
    }
}

impl std::error::Error for LayoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// A leaf's own text plus what's needed to measure it — see
/// [`florui_text`]'s own scope notes for what "measure" does and doesn't
/// cover yet (no rasterization).
struct TextContext {
    text: String,
    font_size: f32,
    font_family: florui_text::FontFamily,
}

fn to_text_font_family(value: florui_style::FontFamily) -> florui_text::FontFamily {
    match value {
        florui_style::FontFamily::SansSerif => florui_text::FontFamily::SansSerif,
        florui_style::FontFamily::Monospace => florui_text::FontFamily::Monospace,
    }
}

/// Computes block-layout geometry for every node in `arena`, using
/// `styles` for sizing/spacing. `available` is the space the layout root
/// itself is given (e.g. the preview window's content area).
pub fn compute_layout(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    available: Size<AvailableSpace>,
) -> Result<HashMap<NodeId, BoxLayout>, LayoutError> {
    let mut tree: TaffyTree<TextContext> = TaffyTree::new();
    let mut taffy_ids: HashMap<NodeId, taffy::NodeId> = HashMap::new();

    for &root in arena.roots() {
        build_node(arena, styles, root, &mut tree, &mut taffy_ids).map_err(LayoutError)?;
    }

    // A synthetic block container wraps every root so multiple top-level
    // elements (view! can produce a Fragment) have somewhere to stack;
    // its own id is never looked up, only real `arena` nodes are.
    let root_children: Vec<taffy::NodeId> = arena.roots().iter().map(|id| taffy_ids[id]).collect();
    let synthetic_root = tree
        .new_with_children(
            taffy::Style {
                display: Display::Block,
                ..Default::default()
            },
            &root_children,
        )
        .map_err(LayoutError)?;

    // Fresh every call, not cached across renders — a real app-registered
    // extra font (`florui_text::Font::register`, e.g. for a script neither
    // embedded font covers) has no way to reach this instance yet, since
    // nothing here persists one across calls to register it on. Until that
    // wiring exists, "font updates invalidate dependent layout" holds
    // trivially at this layer: there is nothing long-lived here to go
    // stale in the first place.
    let mut font = florui_text::Font::load_embedded();
    tree.compute_layout_with_measure(
        synthetic_root,
        available,
        |inputs, _node_id, context, style| {
            // `compute_leaf_layout`'s own measure closure only ever
            // returns a `Size` — extract what the baseline needs from
            // `context` first (cheap: a string clone plus two copy
            // fields), since the closure below moves `context` into
            // `measure` and it isn't available again after.
            let baseline_source = context
                .as_ref()
                .map(|c| (c.text.clone(), c.font_size, c.font_family));

            let mut output = compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known_dimensions, available_space| {
                    measure(&mut font, context, known_dimensions, available_space)
                },
            );

            // Set regardless of `run_mode`: `compute_leaf_layout` skips
            // calling its own measure closure when both dimensions are
            // already known (an explicit width *and* height), but a
            // baseline is still meaningful there — real CSS still aligns
            // an explicitly-sized text box by its text's baseline, not by
            // treating it as baseline-less. Wrap width is irrelevant here:
            // see `florui_text::TextMetrics::baseline`'s own doc for why.
            if let Some((text, font_size, font_family)) = baseline_source {
                let baseline = font.measure(font_family, &text, font_size).baseline;
                output.baselines = Baselines::from_first(Some(baseline));
            }
            output
        },
    )
    .map_err(LayoutError)?;

    let mut result = HashMap::with_capacity(taffy_ids.len());
    for (&node_id, &tid) in &taffy_ids {
        let layout = tree.layout(tid).map_err(LayoutError)?;
        result.insert(
            node_id,
            BoxLayout {
                x: layout.location.x,
                y: layout.location.y,
                width: layout.size.width,
                height: layout.size.height,
            },
        );
    }
    Ok(result)
}

/// A leaf with no text measures at `0x0` — `compute_leaf_layout` only
/// falls back to this for an axis that neither the node's own style nor
/// its parent's known dimensions already settled.
///
/// Wraps when a width is actually known: either the node's own explicit
/// style already resolved one (`known_dimensions.width`, e.g. a `width:
/// 200px` on this very leaf), or the space it's being measured within is
/// definite (`available_space.width`, e.g. a flex/block container with its
/// own resolved width). Neither being true means this measurement is for
/// an intrinsic/max-content size (nothing yet constrains this axis), where
/// real CSS's own behavior is also unwrapped — there is nothing to wrap
/// against. A genuine min-content query (the width of the single longest
/// unbreakable word) isn't distinguished from that case yet; see
/// `florui_text`'s own module docs for that tracked gap.
fn measure(
    font: &mut florui_text::Font,
    context: Option<&mut TextContext>,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> Size<f32> {
    let Some(context) = context else {
        return Size::ZERO;
    };

    let wrap_width = known_dimensions.width.or(match available_space.width {
        AvailableSpace::Definite(width) => Some(width),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
    });

    let metrics = match wrap_width {
        Some(width) => {
            font.measure_wrapped(context.font_family, &context.text, context.font_size, width)
        }
        None => font.measure(context.font_family, &context.text, context.font_size),
    };
    Size {
        width: metrics.width,
        height: metrics.height,
    }
}

fn build_node(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    node: NodeId,
    tree: &mut TaffyTree<TextContext>,
    taffy_ids: &mut HashMap<NodeId, taffy::NodeId>,
) -> Result<taffy::NodeId, taffy::TaffyError> {
    let style = to_taffy_style(styles.get(&node));
    let arena_children = arena.children(node);

    let id = if arena_children.is_empty() {
        let text = arena.text_content(node);
        if text.is_empty() {
            tree.new_leaf(style)?
        } else {
            let font_size = styles.get(&node).map_or(16.0, |s| s.font_size);
            let font_family = styles
                .get(&node)
                .map_or(florui_text::FontFamily::SansSerif, |s| {
                    to_text_font_family(s.font_family)
                });
            tree.new_leaf_with_context(
                style,
                TextContext {
                    text: text.to_string(),
                    font_size,
                    font_family,
                },
            )?
        }
    } else {
        let children: Vec<taffy::NodeId> = arena_children
            .iter()
            .map(|&child| build_node(arena, styles, child, tree, taffy_ids))
            .collect::<Result<_, _>>()?;
        tree.new_with_children(style, &children)?
    };

    taffy_ids.insert(node, id);
    Ok(id)
}

fn to_taffy_style(style: Option<&ComputedStyle>) -> taffy::Style {
    let Some(style) = style else {
        return taffy::Style {
            display: Display::Block,
            ..Default::default()
        };
    };

    taffy::Style {
        display: to_display(style.display),
        // Real CSS's actual default (before any reset stylesheet opts into
        // border-box) is content-box: padding adds to a declared width/
        // height rather than being carved out of it. Taffy's own default is
        // border-box, so this must be set explicitly to match.
        box_sizing: BoxSizing::ContentBox,
        size: Size {
            width: to_dimension(style.width),
            height: to_dimension(style.height),
        },
        margin: Rect {
            left: to_length_percentage_auto(style.margin.left),
            right: to_length_percentage_auto(style.margin.right),
            top: to_length_percentage_auto(style.margin.top),
            bottom: to_length_percentage_auto(style.margin.bottom),
        },
        // These four only affect *this node's own children*, and only take
        // effect at all when `display` above is `Flex` — Taffy ignores them
        // for a block container, so setting them unconditionally is safe.
        flex_direction: to_flex_direction(style.flex_direction),
        flex_wrap: to_flex_wrap(style.flex_wrap),
        justify_content: to_content_alignment(style.justify_content),
        align_content: to_content_alignment(style.align_content),
        align_items: to_item_alignment(style.align_items),
        gap: Size {
            width: LengthPercentage::length(style.column_gap),
            height: LengthPercentage::length(style.row_gap),
        },
        // These three instead describe how *this node itself* behaves as a
        // flex item — meaningful only when this node's *parent* is a flex
        // container, regardless of this node's own `display`.
        align_self: to_item_alignment(style.align_self),
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        flex_basis: to_dimension(style.flex_basis),
        padding: Rect {
            left: LengthPercentage::length(style.padding.left),
            right: LengthPercentage::length(style.padding.right),
            top: LengthPercentage::length(style.padding.top),
            bottom: LengthPercentage::length(style.padding.bottom),
        },
        ..Default::default()
    }
}

fn to_dimension(value: Option<f32>) -> Dimension {
    match value {
        Some(length) => Dimension::length(length),
        None => Dimension::auto(),
    }
}

fn to_length_percentage_auto(value: Option<f32>) -> LengthPercentageAuto {
    match value {
        Some(length) => LengthPercentageAuto::length(length),
        None => LengthPercentageAuto::auto(),
    }
}

fn to_display(value: StyleDisplay) -> Display {
    match value {
        StyleDisplay::Block => Display::Block,
        StyleDisplay::Flex => Display::Flex,
    }
}

fn to_flex_direction(value: StyleFlexDirection) -> FlexDirection {
    match value {
        StyleFlexDirection::Row => FlexDirection::Row,
        StyleFlexDirection::RowReverse => FlexDirection::RowReverse,
        StyleFlexDirection::Column => FlexDirection::Column,
        StyleFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
    }
}

fn to_flex_wrap(value: StyleFlexWrap) -> FlexWrap {
    match value {
        StyleFlexWrap::NoWrap => FlexWrap::NoWrap,
        StyleFlexWrap::Wrap => FlexWrap::Wrap,
        StyleFlexWrap::WrapReverse => FlexWrap::WrapReverse,
    }
}

/// Shared by `justify-content`/`align-content` — Taffy's own `JustifyContent`
/// is a type alias of `AlignContent`, so one conversion serves both fields.
fn to_content_alignment(value: Option<ContentAlignment>) -> Option<AlignContent> {
    value.map(|value| match value {
        ContentAlignment::Start => AlignContent::START,
        ContentAlignment::End => AlignContent::END,
        ContentAlignment::FlexStart => AlignContent::FLEX_START,
        ContentAlignment::FlexEnd => AlignContent::FLEX_END,
        ContentAlignment::Center => AlignContent::CENTER,
        ContentAlignment::Stretch => AlignContent::STRETCH,
        ContentAlignment::SpaceBetween => AlignContent::SPACE_BETWEEN,
        ContentAlignment::SpaceAround => AlignContent::SPACE_AROUND,
        ContentAlignment::SpaceEvenly => AlignContent::SPACE_EVENLY,
    })
}

/// Shared by `align-items`/`align-self` — Taffy's own `AlignSelf` is a type
/// alias of `AlignItems`, so one conversion serves both fields.
fn to_item_alignment(value: Option<ItemAlignment>) -> Option<AlignItems> {
    value.map(|value| match value {
        ItemAlignment::Stretch => AlignItems::STRETCH,
        ItemAlignment::FlexStart => AlignItems::FLEX_START,
        ItemAlignment::FlexEnd => AlignItems::FLEX_END,
        ItemAlignment::Start => AlignItems::START,
        ItemAlignment::End => AlignItems::END,
        ItemAlignment::Center => AlignItems::CENTER,
        ItemAlignment::Baseline => AlignItems::BASELINE,
    })
}

/// Accumulates `node`'s ancestors' [`BoxLayout`] offsets into a position
/// relative to the layout root, since Taffy itself only ever reports a
/// position relative to the immediate parent.
pub fn absolute_position(
    arena: &Arena,
    layouts: &HashMap<NodeId, BoxLayout>,
    node: NodeId,
) -> (f32, f32) {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut current = Some(node);
    while let Some(id) = current {
        if let Some(layout) = layouts.get(&id) {
            x += layout.x;
            y += layout.y;
        }
        current = arena.parent(id);
    }
    (x, y)
}

/// The topmost node whose box contains `(x, y)` (both relative to the
/// layout root, the same space [`absolute_position`] reports) — "topmost"
/// meaning whichever one `florui_paint` would have painted last, since an
/// overlap always resolves to the box on top. Real block layout has no
/// overlapping siblings yet, so in practice this only matters for nested
/// containment, but the rule generalizes to whatever layout produces.
pub fn hit_test(
    arena: &Arena,
    layouts: &HashMap<NodeId, BoxLayout>,
    x: f32,
    y: f32,
) -> Option<NodeId> {
    let mut hit = None;
    for &root in arena.roots() {
        hit_test_node(arena, layouts, root, x, y, &mut hit);
    }
    hit
}

fn hit_test_node(
    arena: &Arena,
    layouts: &HashMap<NodeId, BoxLayout>,
    node: NodeId,
    x: f32,
    y: f32,
    hit: &mut Option<NodeId>,
) {
    if let Some(&layout) = layouts.get(&node) {
        let (ax, ay) = absolute_position(arena, layouts, node);
        if x >= ax && x < ax + layout.width && y >= ay && y < ay + layout.height {
            hit.replace(node);
        }
    }
    for &child in arena.children(node) {
        hit_test_node(arena, layouts, child, x, y, hit);
    }
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;
    use florui_style::InteractionState;

    use super::*;

    fn layout_for(tree: &Element, css: &str) -> (Arena, HashMap<NodeId, BoxLayout>) {
        let arena = Arena::build(tree);
        let rules = florui_style::parse_stylesheet(css).unwrap();
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
        let layouts = compute_layout(&arena, &styles, Size::MAX_CONTENT).unwrap();
        (arena, layouts)
    }

    #[test]
    fn an_explicitly_sized_node_gets_that_size() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, layouts) = layout_for(&tree, ".card { width: 200px; height: 100px; }");
        let node = arena.roots()[0];
        assert_eq!(layouts[&node].width, 200.0);
        assert_eq!(layouts[&node].height, 100.0);
    }

    /// Real CSS's actual default is content-box: padding adds to a
    /// declared width/height rather than being carved out of it. Taffy's
    /// own default is border-box, where this same stylesheet would render
    /// at exactly 200x100 with zero content-box room left over.
    #[test]
    fn padding_adds_to_an_explicit_size_instead_of_shrinking_its_content_box() {
        let tree: Element = view! { <div class="card" /> };
        let (arena, layouts) = layout_for(
            &tree,
            ".card { width: 200px; height: 100px; padding-top: 20px; padding-left: 10px; }",
        );
        let node = arena.roots()[0];
        assert_eq!(
            layouts[&node].width, 210.0,
            "200 declared + 10 padding-left"
        );
        assert_eq!(
            layouts[&node].height, 120.0,
            "100 declared + 20 padding-top"
        );
    }

    #[test]
    fn a_node_with_no_explicit_size_lays_out_at_zero_by_zero() {
        let tree: Element = view! { <div /> };
        let (arena, layouts) = layout_for(&tree, "");
        let node = arena.roots()[0];
        assert_eq!(layouts[&node].width, 0.0);
        assert_eq!(layouts[&node].height, 0.0);
    }

    #[test]
    fn padding_and_margin_position_a_child_inside_its_parent() {
        let tree: Element = view! {
            <div class="card">
                <div class="child" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .card { width: 200px; height: 100px; padding-top: 8px; padding-left: 8px; }
            .child { width: 50px; height: 30px; margin-top: 10px; margin-left: 5px; }
            ",
        );
        let card = arena.roots()[0];
        let child = arena.children(card)[0];

        assert_eq!(layouts[&child].x, 13.0, "padding-left 8 + margin-left 5");
        assert_eq!(layouts[&child].y, 18.0, "padding-top 8 + margin-top 10");
        assert_eq!(layouts[&child].width, 50.0);
    }

    #[test]
    fn block_children_stack_vertically_in_source_order() {
        let tree: Element = view! {
            <div class="card">
                <div class="a" />
                <div class="b" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .card { width: 200px; }
            .a { width: 200px; height: 40px; }
            .b { width: 200px; height: 20px; }
            ",
        );
        let card = arena.roots()[0];
        let a = arena.children(card)[0];
        let b = arena.children(card)[1];

        assert_eq!(layouts[&a].y, 0.0);
        assert_eq!(
            layouts[&b].y, 40.0,
            "block layout stacks the second child below the first one's height"
        );
    }

    #[test]
    fn absolute_position_accumulates_ancestor_offsets() {
        let tree: Element = view! {
            <div class="card">
                <div class="inner">
                    <div class="leaf" />
                </div>
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .card { width: 200px; height: 200px; padding-top: 10px; padding-left: 10px; }
            .inner { width: 100px; height: 100px; padding-top: 5px; padding-left: 5px; }
            .leaf { width: 10px; height: 10px; }
            ",
        );
        let card = arena.roots()[0];
        let inner = arena.children(card)[0];
        let leaf = arena.children(inner)[0];

        assert_eq!(absolute_position(&arena, &layouts, leaf), (15.0, 15.0));
    }

    /// Taffy rounds final layout to whole pixels by default, so a value
    /// measured by `florui_text` (which does not round) is compared with a
    /// sub-pixel tolerance rather than for exact equality.
    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1.0,
            "expected {actual} to be within 1px of {expected}"
        );
    }

    #[test]
    fn a_text_bearing_leaf_with_no_explicit_size_gets_its_measured_intrinsic_size() {
        let tree: Element = view! { <h2>{"Hi"}</h2> };
        let (arena, layouts) = layout_for(&tree, "");
        let node = arena.roots()[0];

        // h2's real font-size is 24px (1.5em) by default — the framework's
        // own default element stylesheet, not the bare 16px initial value
        // an unstyled element with no matching default rule would get.
        let expected = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::SansSerif,
            "Hi",
            24.0,
        );
        assert_close(layouts[&node].width, expected.width);
        assert_close(layouts[&node].height, expected.height);
        assert!(expected.width > 0.0, "the font actually measured something");
    }

    #[test]
    fn an_explicit_size_overrides_measured_text_size() {
        let tree: Element = view! { <h2 class="title">{"Hi"}</h2> };
        let (arena, layouts) = layout_for(&tree, ".title { width: 300px; height: 50px; }");
        let node = arena.roots()[0];
        assert_eq!(layouts[&node].width, 300.0);
        assert_eq!(layouts[&node].height, 50.0);
    }

    #[test]
    fn font_size_changes_the_measured_text_size() {
        let tree: Element = view! { <h2 class="big">{"Hi"}</h2> };
        let (arena, layouts) = layout_for(&tree, ".big { font-size: 40px; }");
        let node = arena.roots()[0];

        let expected = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::SansSerif,
            "Hi",
            40.0,
        );
        assert_close(layouts[&node].width, expected.width);
        assert_close(layouts[&node].height, expected.height);
    }

    #[test]
    fn hit_test_finds_the_deepest_node_containing_the_point() {
        let tree: Element = view! {
            <div class="card">
                <div class="button" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .card { width: 100px; height: 60px; padding-top: 10px; padding-left: 10px; }
            .button { width: 40px; height: 20px; }
            ",
        );
        let card = arena.roots()[0];
        let button = arena.children(card)[0];

        assert_eq!(
            hit_test(&arena, &layouts, 20.0, 20.0),
            Some(button),
            "(20, 20) is inside the button, nested inside the card"
        );
        assert_eq!(
            hit_test(&arena, &layouts, 5.0, 5.0),
            Some(card),
            "(5, 5) is inside the card's padding, outside the button"
        );
        assert_eq!(
            hit_test(&arena, &layouts, 200.0, 200.0),
            None,
            "outside every box"
        );
    }

    #[test]
    fn nested_text_is_measured_on_its_own_node_not_the_ancestor() {
        let tree: Element = view! {
            <div>
                <span>{"Hi"}</span>
            </div>
        };
        let (arena, layouts) = layout_for(&tree, "");
        let card = arena.roots()[0];
        let span = arena.children(card)[0];

        let expected = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::SansSerif,
            "Hi",
            16.0,
        );
        assert_close(layouts[&span].width, expected.width);
        assert_close(layouts[&span].height, expected.height);
    }

    #[test]
    fn text_wraps_and_grows_taller_inside_a_narrow_explicitly_sized_container() {
        let tree: Element = view! {
            <div class="card">
                <h2>{"one two three four five six seven eight nine ten"}</h2>
            </div>
        };
        // No explicit width on the h2 itself — it must still wrap, inheriting
        // its available width from the block container's own resolved
        // content width, the same as a real browser's default block
        // formatting context.
        let (arena, layouts) = layout_for(&tree, ".card { width: 100px; }");
        let card = arena.roots()[0];
        let h2 = arena.children(card)[0];

        let mut font = florui_text::Font::load_embedded();
        let unwrapped = font.measure(
            florui_text::FontFamily::SansSerif,
            "one two three four five six seven eight nine ten",
            16.0,
        );

        assert!(
            layouts[&h2].height > unwrapped.height,
            "wrapping across a 100px container must take more than one line's height"
        );
        assert!(
            layouts[&h2].width <= 100.0 + 1.0,
            "a wrapped leaf must not exceed its container's own width"
        );
    }

    #[test]
    fn an_explicit_width_on_the_text_node_itself_also_wraps_it() {
        let tree: Element = view! { <h2 class="narrow">{"one two three four five"}</h2> };
        let (arena, layouts) = layout_for(&tree, ".narrow { width: 60px; }");
        let node = arena.roots()[0];

        let mut font = florui_text::Font::load_embedded();
        let unwrapped = font.measure(
            florui_text::FontFamily::SansSerif,
            "one two three four five",
            16.0,
        );

        assert_eq!(layouts[&node].width, 60.0, "the explicit width still wins");
        assert!(
            layouts[&node].height > unwrapped.height,
            "an explicit width on the leaf itself must also trigger wrapping"
        );
    }

    #[test]
    fn a_wide_enough_container_does_not_wrap_short_text() {
        let tree: Element = view! {
            <div class="card">
                <h2>{"Hi"}</h2>
            </div>
        };
        let (arena, layouts) = layout_for(&tree, ".card { width: 400px; }");
        let card = arena.roots()[0];
        let h2 = arena.children(card)[0];

        // h2's default font-size is 24px (1.5em) — see the identical note
        // on `a_text_bearing_leaf_with_no_explicit_size_gets_its_measured_intrinsic_size`.
        let expected = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::SansSerif,
            "Hi",
            24.0,
        );
        assert_close(layouts[&h2].height, expected.height);
    }

    #[test]
    fn font_family_monospace_measures_with_the_monospace_embedded_font() {
        let tree: Element = view! { <span class="code">{"AAAAA"}</span> };
        let (arena, layouts) = layout_for(&tree, ".code { font-family: monospace; }");
        let node = arena.roots()[0];

        let expected = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::Monospace,
            "AAAAA",
            16.0,
        );
        assert_close(layouts[&node].width, expected.width);
    }

    #[test]
    fn no_font_family_declared_measures_with_the_sans_serif_default() {
        let tree: Element = view! { <span>{"AAAAA"}</span> };
        let (arena, layouts) = layout_for(&tree, "");
        let node = arena.roots()[0];

        let sans_serif = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::SansSerif,
            "AAAAA",
            16.0,
        );
        let monospace = florui_text::Font::load_embedded().measure(
            florui_text::FontFamily::Monospace,
            "AAAAA",
            16.0,
        );
        assert_close(layouts[&node].width, sans_serif.width);
        assert!(
            (layouts[&node].width - monospace.width).abs() > 1.0,
            "must actually be measuring with the proportional sans-serif default, \
             not coincidentally matching the monospace width"
        );
    }

    #[test]
    fn a_flex_row_lays_out_children_left_to_right() {
        let tree: Element = view! {
            <div class="row">
                <div class="a" />
                <div class="b" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; width: 300px; height: 50px; }
            .a { width: 100px; height: 50px; }
            .b { width: 80px; height: 50px; }
            ",
        );
        let row = arena.roots()[0];
        let a = arena.children(row)[0];
        let b = arena.children(row)[1];

        assert_eq!(layouts[&a].x, 0.0);
        assert_eq!(layouts[&b].x, 100.0, "b starts right where a's 100px ends");
        assert_eq!(
            layouts[&a].y, 0.0,
            "a flex row keeps children on the same cross-axis line by default"
        );
    }

    #[test]
    fn a_flex_column_lays_out_children_top_to_bottom() {
        let tree: Element = view! {
            <div class="col">
                <div class="a" />
                <div class="b" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .col { display: flex; flex-direction: column; width: 100px; height: 300px; }
            .a { width: 100px; height: 40px; }
            .b { width: 100px; height: 20px; }
            ",
        );
        let col = arena.roots()[0];
        let a = arena.children(col)[0];
        let b = arena.children(col)[1];

        assert_eq!(layouts[&a].y, 0.0);
        assert_eq!(layouts[&b].y, 40.0, "b starts right where a's 40px ends");
    }

    #[test]
    fn justify_content_space_between_pushes_children_to_the_edges() {
        let tree: Element = view! {
            <div class="row">
                <div class="a" />
                <div class="b" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; justify-content: space-between; width: 300px; height: 50px; }
            .a { width: 50px; height: 50px; }
            .b { width: 50px; height: 50px; }
            ",
        );
        let row = arena.roots()[0];
        let a = arena.children(row)[0];
        let b = arena.children(row)[1];

        assert_eq!(
            layouts[&a].x, 0.0,
            "the first child stays flush with the start"
        );
        assert_eq!(
            layouts[&b].x, 250.0,
            "the last child is flush with the end: 300 container - 50 own width"
        );
    }

    #[test]
    fn align_items_center_centers_a_shorter_child_on_the_cross_axis() {
        let tree: Element = view! {
            <div class="row">
                <div class="short" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; align-items: center; width: 100px; height: 100px; }
            .short { width: 20px; height: 20px; }
            ",
        );
        let row = arena.roots()[0];
        let short = arena.children(row)[0];

        assert_eq!(
            layouts[&short].y, 40.0,
            "centered in a 100px-tall row: (100 - 20) / 2"
        );
    }

    #[test]
    fn flex_grow_distributes_remaining_space_proportionally() {
        let tree: Element = view! {
            <div class="row">
                <div class="one" />
                <div class="two" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; width: 300px; height: 50px; }
            .one { flex-grow: 1; height: 50px; }
            .two { flex-grow: 2; height: 50px; }
            ",
        );
        let row = arena.roots()[0];
        let one = arena.children(row)[0];
        let two = arena.children(row)[1];

        assert_eq!(
            layouts[&one].width, 100.0,
            "1 share of 300px free space (both start at 0 width)"
        );
        assert_eq!(layouts[&two].width, 200.0, "2 shares of the same 300px");
    }

    #[test]
    fn flex_shrink_zero_keeps_a_child_at_its_basis_even_when_siblings_overflow() {
        let tree: Element = view! {
            <div class="row">
                <div class="fixed" />
                <div class="flexible" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; width: 150px; height: 50px; }
            .fixed { width: 100px; height: 50px; flex-shrink: 0; }
            .flexible { width: 100px; height: 50px; }
            ",
        );
        let row = arena.roots()[0];
        let fixed = arena.children(row)[0];

        assert_eq!(
            layouts[&fixed].width, 100.0,
            "flex-shrink: 0 must not shrink even though the row is 50px too narrow \
             for both children's own widths"
        );
    }

    #[test]
    fn gap_adds_space_between_flex_children_without_affecting_the_first_ones_position() {
        let tree: Element = view! {
            <div class="row">
                <div class="a" />
                <div class="b" />
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; column-gap: 10px; width: 300px; height: 50px; }
            .a { width: 50px; height: 50px; }
            .b { width: 50px; height: 50px; }
            ",
        );
        let row = arena.roots()[0];
        let a = arena.children(row)[0];
        let b = arena.children(row)[1];

        assert_eq!(layouts[&a].x, 0.0);
        assert_eq!(layouts[&b].x, 60.0, "a's 50px width + 10px column-gap");
    }

    /// A node's own `display` never affects how its parent places *it* —
    /// only how it places its own children.
    #[test]
    fn a_block_child_inside_a_flex_row_is_still_laid_out_by_the_flex_algorithm() {
        let tree: Element = view! {
            <div class="row">
                <div class="block-child">
                    <div class="grandchild" />
                </div>
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; width: 200px; height: 50px; }
            .block-child { width: 80px; height: 50px; }
            .grandchild { width: 20px; height: 20px; }
            ",
        );
        let row = arena.roots()[0];
        let block_child = arena.children(row)[0];

        assert_eq!(
            layouts[&block_child].x, 0.0,
            "the flex row still positions its block-display child as a flex item"
        );
    }

    #[test]
    fn align_items_baseline_lines_up_differently_sized_text_by_their_shared_baseline() {
        let tree: Element = view! {
            <div class="row">
                <span class="small">{"Hg"}</span>
                <span class="big">{"Hg"}</span>
            </div>
        };
        let (arena, layouts) = layout_for(
            &tree,
            "
            .row { display: flex; align-items: baseline; height: 100px; }
            .small { font-size: 16px; }
            .big { font-size: 40px; }
            ",
        );
        let row = arena.roots()[0];
        let small = arena.children(row)[0];
        let big = arena.children(row)[1];

        let mut font = florui_text::Font::load_embedded();
        let small_metrics = font.measure(florui_text::FontFamily::SansSerif, "Hg", 16.0);
        let big_metrics = font.measure(florui_text::FontFamily::SansSerif, "Hg", 40.0);

        // Real baseline alignment: each item's own (y + its baseline offset)
        // must land on the same line — not the same `y`, and not simply
        // top- or center-aligned, both of which would put these at
        // different combined baselines since the two font sizes have
        // different ascents.
        assert_close(
            layouts[&small].y + small_metrics.baseline,
            layouts[&big].y + big_metrics.baseline,
        );
        assert!(
            layouts[&small].y > layouts[&big].y,
            "the smaller text's shorter ascent means it starts lower, not at the same y \
             (small starts at {}, big at {})",
            layouts[&small].y,
            layouts[&big].y
        );
    }
}

//! Turns a [`florui_style::Arena`] plus its computed styles into geometry,
//! via [Taffy](https://github.com/DioxusLabs/taffy) — validated against
//! Taffy 0.14's actual current API rather than assumed from memory.
//!
//! # Scope
//!
//! Block-level stacking only: every node is laid out with
//! `Display::Block`, explicitly, since Taffy's own default (with its
//! default feature set) is `Display::Flex` and silently relying on that
//! would not match this crate's declared scope. Flexbox/Grid need more
//! properties in `florui-style` (`display`, `flex-*`, `grid-*`) before
//! there is anything real to translate for them.
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

use florui_style::{Arena, ComputedStyle, NodeId};
use taffy::compute_leaf_layout;
use taffy::prelude::*;

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

/// The one embedded font this crate measures leaf text with — see
/// [`florui_text`]'s own scope notes for what "measure" does and doesn't
/// cover yet (no rasterization, no wrapping, one font).
struct TextContext {
    text: String,
    font_size: f32,
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

    let mut font = florui_text::Font::load_embedded();
    tree.compute_layout_with_measure(
        synthetic_root,
        available,
        |inputs, _node_id, context, style| {
            compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |_, _| measure(&mut font, context),
            )
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
fn measure(font: &mut florui_text::Font, context: Option<&mut TextContext>) -> Size<f32> {
    match context {
        Some(context) => {
            let metrics = font.measure(&context.text, context.font_size);
            Size {
                width: metrics.width,
                height: metrics.height,
            }
        }
        None => Size::ZERO,
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
            tree.new_leaf_with_context(
                style,
                TextContext {
                    text: text.to_string(),
                    font_size,
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
        display: Display::Block,
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

        let expected = florui_text::Font::load_embedded().measure("Hi", 16.0);
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

        let expected = florui_text::Font::load_embedded().measure("Hi", 40.0);
        assert_close(layouts[&node].width, expected.width);
        assert_close(layouts[&node].height, expected.height);
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

        let expected = florui_text::Font::load_embedded().measure("Hi", 16.0);
        assert_close(layouts[&span].width, expected.width);
        assert_close(layouts[&span].height, expected.height);
    }
}

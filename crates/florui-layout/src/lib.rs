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
//! There is no intrinsic (content-based) sizing: nothing here measures
//! text, since no text-shaping engine exists yet. A node with no explicit
//! `width`/`height` lays out at `0x0` rather than guessing a size from
//! content that was never actually measured.
//!
//! Taffy positions are relative to the parent's content box, matching
//! Taffy's own convention; see [`absolute_position`] to accumulate them
//! into a position relative to the layout root.

use std::collections::HashMap;

use florui_style::{Arena, ComputedStyle, NodeId};
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

/// Computes block-layout geometry for every node in `arena`, using
/// `styles` for sizing/spacing. `available` is the space the layout root
/// itself is given (e.g. the preview window's content area).
pub fn compute_layout(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    available: Size<AvailableSpace>,
) -> Result<HashMap<NodeId, BoxLayout>, LayoutError> {
    let mut tree: TaffyTree<()> = TaffyTree::new();
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

    tree.compute_layout(synthetic_root, available)
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

fn build_node(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    node: NodeId,
    tree: &mut TaffyTree<()>,
    taffy_ids: &mut HashMap<NodeId, taffy::NodeId>,
) -> Result<taffy::NodeId, taffy::TaffyError> {
    let children: Vec<taffy::NodeId> = arena
        .children(node)
        .iter()
        .map(|&child| build_node(arena, styles, child, tree, taffy_ids))
        .collect::<Result<_, _>>()?;

    let style = to_taffy_style(styles.get(&node));
    let id = tree.new_with_children(style, &children)?;
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
}

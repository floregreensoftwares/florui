//! Resolves declared values into used values for every node: matches
//! rules, picks the winning declaration per property by (specificity,
//! source order), then applies each property's own inheritance rule.

use std::collections::HashMap;

use crate::color::Rgba;
use crate::interaction::InteractionState;
use crate::matching::matches_selector;
use crate::selector::{Specificity, specificity_of};
use crate::stylesheet_parse::Rule;
use crate::tree::{Arena, NodeId};
use crate::value::{Property, Value};

/// One edge's value on each of the four sides of the box, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputedStyle {
    pub background_color: Rgba,
    pub color: Rgba,
    /// `None` means `auto`.
    pub width: Option<f32>,
    /// `None` means `auto`.
    pub height: Option<f32>,
    /// Each edge is `None` for an explicit `auto` (enabling the usual
    /// auto-margin centering behavior), `Some(0.0)` when nothing set it.
    pub margin: Edges<Option<f32>>,
    /// Always a concrete value; real CSS padding has no `auto`.
    pub padding: Edges<f32>,
    /// Inherits; initial `16.0`.
    pub font_size: f32,
}

pub fn compute(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
) -> HashMap<NodeId, ComputedStyle> {
    let mut result = HashMap::new();
    for &root in arena.roots() {
        compute_node(arena, rules, state, root, None, &mut result);
    }
    result
}

fn compute_node(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
    node: NodeId,
    parent: Option<&ComputedStyle>,
    result: &mut HashMap<NodeId, ComputedStyle>,
) {
    let style = resolve_style(arena, rules, state, node, parent);
    result.insert(node, style);
    for &child in arena.children(node) {
        compute_node(arena, rules, state, child, Some(&style), result);
    }
}

fn resolve_style(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
    node: NodeId,
    parent: Option<&ComputedStyle>,
) -> ComputedStyle {
    let color = |property, initial, parent_value: fn(&ComputedStyle) -> Rgba| {
        resolve(
            arena,
            rules,
            state,
            node,
            parent,
            property,
            property.inherits(),
            initial,
            as_color,
            parent_value,
        )
    };
    let size = |property, parent_value: fn(&ComputedStyle) -> Option<f32>| {
        resolve(
            arena,
            rules,
            state,
            node,
            parent,
            property,
            false,
            None,
            as_optional_length,
            parent_value,
        )
    };
    let margin_edge = |property, parent_value: fn(&ComputedStyle) -> Option<f32>| {
        resolve(
            arena,
            rules,
            state,
            node,
            parent,
            property,
            false,
            Some(0.0),
            as_optional_length,
            parent_value,
        )
    };
    let padding_edge = |property, parent_value: fn(&ComputedStyle) -> f32| {
        resolve(
            arena,
            rules,
            state,
            node,
            parent,
            property,
            false,
            0.0,
            as_length,
            parent_value,
        )
    };

    ComputedStyle {
        background_color: color(Property::BackgroundColor, Rgba::TRANSPARENT, |s| {
            s.background_color
        }),
        color: color(Property::Color, Rgba::opaque(0, 0, 0), |s| s.color),
        width: size(Property::Width, |s| s.width),
        height: size(Property::Height, |s| s.height),
        margin: Edges {
            top: margin_edge(Property::MarginTop, |s| s.margin.top),
            right: margin_edge(Property::MarginRight, |s| s.margin.right),
            bottom: margin_edge(Property::MarginBottom, |s| s.margin.bottom),
            left: margin_edge(Property::MarginLeft, |s| s.margin.left),
        },
        padding: Edges {
            top: padding_edge(Property::PaddingTop, |s| s.padding.top),
            right: padding_edge(Property::PaddingRight, |s| s.padding.right),
            bottom: padding_edge(Property::PaddingBottom, |s| s.padding.bottom),
            left: padding_edge(Property::PaddingLeft, |s| s.padding.left),
        },
        font_size: resolve(
            arena,
            rules,
            state,
            node,
            parent,
            Property::FontSize,
            true,
            16.0,
            as_length,
            |s| s.font_size,
        ),
    }
}

fn as_color(value: Value) -> Rgba {
    match value {
        Value::Color(color) => color,
        other => unreachable!("a color property never resolves a non-color value: {other:?}"),
    }
}

fn as_optional_length(value: Value) -> Option<f32> {
    match value {
        Value::Length(length) => Some(length),
        Value::Auto => None,
        other => unreachable!("a length-or-auto property never resolves {other:?}"),
    }
}

fn as_length(value: Value) -> f32 {
    match value {
        Value::Length(length) => length,
        other => unreachable!("a length-only property never resolves {other:?}"),
    }
}

/// Resolves one property to its used value `T`: the winning declaration's
/// value if any (honoring explicit `initial`/`inherit` keywords), else
/// this property's own default (inherited from the parent, or `initial`).
#[allow(clippy::too_many_arguments)]
fn resolve<T: Copy>(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
    node: NodeId,
    parent: Option<&ComputedStyle>,
    property: Property,
    inherits_by_default: bool,
    initial: T,
    to_value: impl Fn(Value) -> T,
    parent_value: impl Fn(&ComputedStyle) -> T,
) -> T {
    let inherited = || parent.map(&parent_value).unwrap_or(initial);
    match winning_value(arena, rules, state, node, property) {
        Some(Value::Initial) => initial,
        Some(Value::Inherit) => inherited(),
        Some(other) => to_value(other),
        None if inherits_by_default => inherited(),
        None => initial,
    }
}

fn winning_value(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
    node: NodeId,
    property: Property,
) -> Option<Value> {
    let mut best: Option<(Specificity, usize, Value)> = None;
    for rule in rules {
        if !matches_selector(arena, node, &rule.selector, state) {
            continue;
        }
        let specificity = specificity_of(&rule.selector);
        for declaration in &rule.declarations {
            if declaration.property != property {
                continue;
            }
            let candidate_key = (specificity, rule.source_order);
            let replace = match &best {
                None => true,
                Some((s, o, _)) => candidate_key >= (*s, *o),
            };
            if replace {
                best = Some((specificity, rule.source_order, declaration.value));
            }
        }
    }
    best.map(|(_, _, value)| value)
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
        let style = computed[&node];
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
    fn font_size_defaults_to_sixteen_pixels() {
        let tree: Element = view! { <div /> };
        let (arena, computed) = styles(&tree, "", &InteractionState::new());
        let node = arena.roots()[0];
        assert_eq!(computed[&node].font_size, 16.0);
    }
}

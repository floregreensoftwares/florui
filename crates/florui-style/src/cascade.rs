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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputedStyle {
    pub background_color: Rgba,
    pub color: Rgba,
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
    ComputedStyle {
        background_color: resolve_property(
            arena,
            rules,
            state,
            node,
            parent,
            Property::BackgroundColor,
            |s| s.background_color,
        ),
        color: resolve_property(arena, rules, state, node, parent, Property::Color, |s| {
            s.color
        }),
    }
}

fn resolve_property(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
    node: NodeId,
    parent: Option<&ComputedStyle>,
    property: Property,
    parent_value: impl Fn(&ComputedStyle) -> Rgba,
) -> Rgba {
    let inherited = || {
        parent
            .map(&parent_value)
            .unwrap_or_else(|| property.initial())
    };

    match winning_value(arena, rules, state, node, property) {
        Some(Value::Color(color)) => color,
        Some(Value::Initial) => property.initial(),
        Some(Value::Inherit) => inherited(),
        None if property.inherits() => inherited(),
        None => property.initial(),
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
}

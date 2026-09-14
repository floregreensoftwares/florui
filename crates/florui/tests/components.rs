mod support;

use florui::prelude::*;
use support::{Button, ButtonProps, Card, CardProps, Label, LabelProps};

#[test]
fn self_closing_component_has_no_children_field() {
    let el = render_once(|| {
        Button(ButtonProps {
            label: "Open projects".to_string(),
        })
    });
    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    assert_eq!(node.tag, "button");
    assert_eq!(
        node.attrs,
        vec![("class".to_string(), "button".to_string())]
    );
    assert_eq!(node.children, vec![Element::text("Open projects")]);
}

#[test]
fn component_forwards_children_without_a_wrapper() {
    let el = render_once(|| {
        Card(CardProps {
            title: "Hello".to_string(),
            children: Children::from(vec![Element::text("body")]),
        })
    });
    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    assert_eq!(node.tag, "div");
    assert_eq!(
        node.children,
        vec![
            Element::node("h2", vec![], vec![Element::text("Hello")]),
            Element::text("body"),
        ]
    );
}

#[test]
fn view_with_multiple_roots_becomes_a_fragment() {
    let el: Element = view! {
        {"a"}
        {"b"}
    };
    assert_eq!(
        el,
        Element::Fragment(vec![Element::text("a"), Element::text("b")])
    );
}

#[test]
fn view_calls_components_by_capitalized_tag() {
    let el: Element = render_once(|| {
        view! {
            <div>
                <Label text={"hi".to_string()} />
            </div>
        }
    });
    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    assert_eq!(node.tag, "div");
    assert_eq!(
        node.children,
        vec![Element::node("span", vec![], vec![Element::text("hi")])]
    );
}

use florui::prelude::*;

#[test]
fn bare_words_join_with_a_single_space() {
    let el: Element = view! { <p>Open source UI</p> };
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    assert_eq!(node.children, vec![Element::text("Open source UI")]);
}

#[test]
fn punctuation_attaches_without_a_leading_space() {
    let el: Element = view! { <p>Hello, world!</p> };
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    assert_eq!(node.children, vec![Element::text("Hello, world!")]);
}

#[test]
fn hyphens_glue_without_surrounding_spaces() {
    let el: Element = view! { <p>a well-known example</p> };
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    assert_eq!(node.children, vec![Element::text("a well-known example")]);
}

#[test]
fn bare_text_and_expressions_stay_as_separate_siblings() {
    let name = "Ada";
    let el: Element = view! { <p>Hello {name}!</p> };
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    // Mirrors JSX: adjacent text/expression children are not merged into
    // one string, only rendered next to each other.
    assert_eq!(
        node.children,
        vec![
            Element::text("Hello"),
            Element::text("Ada"),
            Element::text("!")
        ]
    );
}

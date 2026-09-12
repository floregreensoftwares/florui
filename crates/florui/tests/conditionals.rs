use florui::prelude::*;

#[test]
fn some_renders_the_inner_value() {
    let shown: Option<Element> = Some(Element::text("shown"));
    let el: Element = view! { <div>{shown}</div> };
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    assert_eq!(node.children, vec![Element::text("shown")]);
}

#[test]
fn none_renders_nothing() {
    let hidden: Option<Element> = None;
    let el: Element = view! { <div>{hidden}</div> };
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    assert!(node.children.is_empty());
}

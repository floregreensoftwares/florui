//! `value={binding}` on a primitive tag: a typed `Binding<String>`
//! write-back channel, carried alongside the plain string snapshot every
//! other attribute consumer already reads.

use std::cell::RefCell;
use std::rc::Rc;

use florui::prelude::*;

#[test]
fn value_expr_populates_both_the_string_attr_and_the_binding() {
    let binding = Binding::new("Ada".to_string(), |_| {});
    let el: Element = view! { <input type="text" value={binding} /> };

    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    assert_eq!(
        node.attrs
            .iter()
            .find(|(k, _)| k == "value")
            .map(|(_, v)| v.as_str()),
        Some("Ada"),
        "the plain string form must still be there for measurement/paint"
    );
    assert_eq!(node.bindings.len(), 1);
    assert_eq!(node.bindings[0].0, "value");
}

#[test]
fn value_literal_stays_a_plain_attribute_with_no_binding() {
    let el: Element = view! { <input type="text" value="static" /> };
    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    assert_eq!(
        node.attrs
            .iter()
            .find(|(k, _)| k == "value")
            .map(|(_, v)| v.as_str()),
        Some("static")
    );
    assert!(
        node.bindings.is_empty(),
        "a string literal has nothing to write back to"
    );
}

#[test]
fn writing_through_the_carried_binding_reaches_the_owner() {
    let owner = Rc::new(RefCell::new("Ada".to_string()));
    let owner_in_binding = Rc::clone(&owner);
    let binding = Binding::new("Ada".to_string(), move |value| {
        *owner_in_binding.borrow_mut() = value;
    });
    let el: Element = view! { <input type="text" value={binding} /> };

    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    let (_key, carried_binding) = &node.bindings[0];
    carried_binding.request_update("Grace".to_string());
    assert_eq!(*owner.borrow(), "Grace");
}

#[test]
fn oninput_alongside_a_plain_value_uses_the_explicit_contract_not_a_binding() {
    let current_name = "Ada".to_string();
    let el: Element = view! {
        <input type="text" value={current_name.clone()} oninput={|_: String| ()} />
    };

    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    assert_eq!(
        node.attrs
            .iter()
            .find(|(k, _)| k == "value")
            .map(|(_, v)| v.as_str()),
        Some("Ada")
    );
    assert!(
        node.bindings.is_empty(),
        "a plain value with oninput must not be treated as a Binding"
    );
    assert_eq!(node.value_handlers.len(), 1);
    assert_eq!(node.value_handlers[0].0, "value");
}

#[test]
fn the_carried_value_handler_reports_the_new_value() {
    let received = Rc::new(RefCell::new(None));
    let received_in_handler = Rc::clone(&received);
    let el: Element = view! {
        <input
            type="text"
            value={"Ada".to_string()}
            oninput={move |value: String| *received_in_handler.borrow_mut() = Some(value)}
        />
    };

    let Element::Node(node) = &el else {
        panic!("expected a node");
    };
    let (_key, handler) = &node.value_handlers[0];
    handler.call("Grace".to_string());
    assert_eq!(*received.borrow(), Some("Grace".to_string()));
}

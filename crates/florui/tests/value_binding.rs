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

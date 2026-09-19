//! The spec's literal acceptance test: two components declaring the same
//! local class name (`.box`) through separate `stylesheet_scoped!`
//! declarations must resolve independently, not collide.

mod card {
    use florui::prelude::*;

    stylesheet_scoped!("./fixtures/scoped_card.css");

    /// Its own separate `view!`, no `stylesheet_scoped!`/`scope` of its
    /// own — used to prove `Card`'s scope does not leak across the
    /// component-call boundary into a child component's own template.
    #[component]
    pub fn Chip() -> Element {
        view! { <span class="chip">{"chip"}</span> }
    }

    #[component]
    pub fn Card() -> Element {
        view! {
            <div class="box" scope={SCOPE}>
                <span class="label">{"card"}</span>
                <Chip />
            </div>
        }
    }
}

mod widget {
    use florui::prelude::*;

    stylesheet_scoped!("./fixtures/scoped_widget.css");

    #[component]
    pub fn Widget() -> Element {
        view! { <div class="box" scope={SCOPE}>{"widget"}</div> }
    }
}

use florui::prelude::*;

fn class_of(el: &Element) -> String {
    let Element::Node(node) = el else {
        panic!("expected a node");
    };
    node.attrs
        .iter()
        .find(|(name, _)| name == "class")
        .map(|(_, value)| value.clone())
        .expect("a class attribute")
}

#[test]
fn two_components_using_the_same_local_class_name_do_not_collide() {
    let card_class = class_of(&render_once(|| card::Card(card::CardProps {})));
    let widget_class = class_of(&render_once(|| widget::Widget(widget::WidgetProps {})));

    assert_ne!(
        card_class, widget_class,
        "each component's scoped `.box` must resolve to a distinct class"
    );
    assert!(card_class.starts_with("box--"));
    assert!(widget_class.starts_with("box--"));
}

#[test]
fn the_scope_id_matches_the_stylesheets_own_declared_scope() {
    let card_class = class_of(&render_once(|| card::Card(card::CardProps {})));
    let expected_suffix = card::SCOPE.suffix();
    assert_eq!(card_class, format!("box{expected_suffix}"));
    assert_eq!(
        card::__FLORUI_STYLESHEET.scope,
        Some(card::SCOPE),
        "the StylesheetSource's own scope must be the same value view! applied"
    );
}

#[test]
fn scope_propagates_to_literal_descendants() {
    let card = render_once(|| card::Card(card::CardProps {}));
    let Element::Node(node) = &card else {
        panic!("expected a node");
    };
    let label_class = class_of(&node.children[0]);
    assert_eq!(label_class, format!("label{}", card::SCOPE.suffix()));
}

#[test]
fn scope_does_not_leak_into_a_nested_components_own_template() {
    let card = render_once(|| card::Card(card::CardProps {}));
    let Element::Node(node) = &card else {
        panic!("expected a node");
    };
    // `<Chip />` is the second child; its own `<span class="chip">` comes
    // from Chip's separate `view!`, which never saw Card's scope.
    let chip_class = class_of(&node.children[1]);
    assert_eq!(
        chip_class, "chip",
        "a child component's own template must be untouched by its caller's scope"
    );
}

#[test]
fn a_global_stylesheet_declaration_is_unaffected() {
    mod unscoped {
        use florui::prelude::*;
        stylesheet!("./fixtures/button.css");
    }
    assert_eq!(unscoped::__FLORUI_STYLESHEET.scope, None);
}

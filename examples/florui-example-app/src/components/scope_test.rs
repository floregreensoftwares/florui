//! A visual check for explicit style scoping: two components each declare
//! their own `.box`/`.label` through a separate `stylesheet_scoped!`, with
//! completely different colors — the same local class name, resolved
//! independently through the real Stylo cascade, no collision.

use florui::prelude::*;

stylesheet!("./scope_test.css");

pub mod red_card {
    use florui::prelude::*;

    stylesheet_scoped!("./scope_test_red.css");

    #[component]
    pub fn RedCard() -> Element {
        view! {
            <div class="box" scope={SCOPE}>
                <span class="label">{"RED"}</span>
            </div>
        }
    }
}

pub mod blue_card {
    use florui::prelude::*;

    stylesheet_scoped!("./scope_test_blue.css");

    #[component]
    pub fn BlueCard() -> Element {
        view! {
            <div class="box" scope={SCOPE}>
                <span class="label">{"BLUE"}</span>
            </div>
        }
    }
}

use blue_card::{BlueCard, BlueCardProps};
use red_card::{RedCard, RedCardProps};

#[component]
pub fn ScopeTest() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"Both cards below are styled by a completely separate \
                  stylesheet_scoped!(\"...\") declaration, and both declare \
                  the exact same local class name -- \".box\" and \".label\". \
                  If scoping worked, they render with their own colors below. \
                  If it silently collided, both boxes would show whichever \
                  stylesheet's rule happened to cascade last."}
            </span>
            <div class="row">
                <RedCard />
                <BlueCard />
            </div>
        </div>
    }
}

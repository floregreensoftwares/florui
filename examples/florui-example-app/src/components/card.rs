use florui::prelude::*;

use super::button::{Button, ButtonProps};

stylesheet!("./card.css");

#[component]
pub fn Card(title: String) -> Element {
    view! {
        <div class="card">
            <h2>{title}</h2>
            <Button label={"Open projects".to_string()} />
        </div>
    }
}

use florui::prelude::*;

use super::button::{Button, ButtonProps};

stylesheet!("./card.css");

#[component]
pub fn Card(title: String) -> Element {
    view! {
        <div class="card" style="background-color: #1e1e22;">
            <h2>{title}</h2>
            <Button label={"Open projects".to_string()} />
        </div>
    }
}

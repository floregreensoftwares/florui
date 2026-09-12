use florui::prelude::*;

stylesheet!("./button.css");

#[component]
pub fn Button(label: String) -> Element {
    view! {
        <button class="button" style="background-color: #42734f;">{label}</button>
    }
}

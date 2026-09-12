use florui::prelude::*;

#[component]
pub fn Button(label: String) -> Element {
    view! {
        <button class="button">{label}</button>
    }
}

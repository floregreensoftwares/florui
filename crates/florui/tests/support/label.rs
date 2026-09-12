use florui::prelude::*;

#[component]
pub fn Label(text: String) -> Element {
    view! { <span>{text}</span> }
}

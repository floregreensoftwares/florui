use florui::prelude::*;

#[component]
pub fn Card(title: String, children: Children) -> Element {
    view! {
        <div class="card">
            <h2>{title}</h2>
            {children}
        </div>
    }
}

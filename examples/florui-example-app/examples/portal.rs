//! A real portal-based overlay, live: "Open dialog" renders a `<Portal>`
//! whose content becomes an independent, unclipped overlay root, painted
//! on top of the document and winning hit-testing -- not a simulated
//! z-index trick. Clicking the backdrop closes it (a plain `onclick` on
//! the backdrop itself); clicking the dialog box does not, since
//! `dispatch_click` targets exactly the hit-tested node with no
//! bubbling.
//!
//! `cargo run --example portal -p florui-example-app`

use florui::prelude::*;
use florui_platform::{Portal, PortalProps};
use florui_reactive::use_signal;
use florui_style::Rgba;

const PORTAL_CSS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/portal.css");

fn main() {
    florui_platform::run_with_css_reload(
        "Florui portal",
        PORTAL_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        app,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn app() -> Element {
    let open = use_signal(|| false);
    let open_dialog = open.clone();
    let close_from_backdrop = open.clone();
    let close_from_button = open.clone();

    view! {
        <div class="page">
            <p class="instructions">
                {"Open the dialog, then click the backdrop (closes) vs. the dialog box \
                  itself (does not)."}
            </p>
            <p class="status">
                {if open.get() { "Dialog is open" } else { "Dialog is closed" }}
            </p>
            <button class="action" onclick={move || open_dialog.set(true)}>
                {"Open dialog"}
            </button>
            {open.get().then(move || view! {
                <Portal>
                    <div class="backdrop" onclick={move || close_from_backdrop.set(false)}>
                        <div class="dialog">
                            <p class="dialog-title">{"Real portal content"}</p>
                            <p class="dialog-body">
                                {"This renders outside the page's own clipping area, as an \
                                  independent overlay root."}
                            </p>
                            <button
                                class="action"
                                onclick={move || close_from_button.set(false)}
                            >
                                {"Close"}
                            </button>
                        </div>
                    </div>
                </Portal>
            })}
        </div>
    }
}

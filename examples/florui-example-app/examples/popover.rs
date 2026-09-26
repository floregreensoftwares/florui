//! A real anchor-tracked, non-modal `Popover`, live:
//!
//! - "Open menu" opens a popover anchored below it. Resize the window
//!   small (or scroll it near an edge) and reopen to see it flip above
//!   the button, or shift sideways, rather than running off the window.
//! - "Submenu" (inside the menu) opens a second, nested popover anchored
//!   to its own right edge. Clicking inside the submenu's own content
//!   does not close the parent menu -- only a click genuinely outside
//!   both closes them. Escape closes only the submenu first.
//! - Clicking "elsewhere" (or anywhere else outside every open popover)
//!   dismisses whatever is open.
//! - Unlike `Dialog` (see the `portal` example), Tab/Shift+Tab are never
//!   trapped inside an open popover -- there is no focus trap here at
//!   all.
//!
//! `cargo run --example popover -p florui-example-app`

use florui::prelude::*;
use florui_platform::{Align, Placement, Popover, PopoverProps, Side};
use florui_reactive::use_signal;
use florui_style::Rgba;

const POPOVER_CSS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/popover.css");

fn main() {
    florui_platform::run_with_css_reload(
        "Florui popover",
        POPOVER_CSS_PATH,
        Rgba::opaque(0x10, 0x10, 0x14),
        app,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn app() -> Element {
    let menu_open = use_signal(|| false);
    let menu_toggle = menu_open.clone();
    let menu_dismiss = menu_open.clone();

    let submenu_open = use_signal(|| false);
    let submenu_toggle = submenu_open.clone();
    let submenu_dismiss = submenu_open.clone();

    view! {
        <div class="page">
            <p class="instructions">
                {"Open the menu, then its own submenu. Clicking inside the submenu's content \
                  never closes the parent -- only a click genuinely outside both does. Resize \
                  the window small and reopen near an edge to see the menu flip/shift instead \
                  of running off it. Tab moves through the whole page even while open: no \
                  focus trap here, unlike Dialog."}
            </p>
            <Popover
                id={"menu".to_string()}
                open={menu_open.get()}
                placement={Placement::new(Side::Bottom, Align::Start)}
                ondismiss={Handler::new(move || menu_dismiss.set(false))}
                trigger={view! {
                    <button
                        class="action"
                        onclick={move || menu_toggle.set(!menu_toggle.get())}
                    >
                        {"Open menu"}
                    </button>
                }}
            >
                <div class="menu">
                    <p class="menu-item-text">{"Item one"}</p>
                    <p class="menu-item-text">{"Item two"}</p>
                    <Popover
                        id={"submenu".to_string()}
                        open={submenu_open.get()}
                        placement={Placement::new(Side::Right, Align::Start)}
                        ondismiss={Handler::new(move || submenu_dismiss.set(false))}
                        trigger={view! {
                            <button
                                class="menu-item"
                                onclick={move || submenu_toggle.set(!submenu_toggle.get())}
                            >
                                {"Submenu \u{25B8}"}
                            </button>
                        }}
                    >
                        <div class="menu">
                            <p class="menu-item-text">{"Submenu item"}</p>
                        </div>
                    </Popover>
                </div>
            </Popover>
            <p class="elsewhere">{"Click here (or anywhere outside) to dismiss"}</p>
        </div>
    }
}

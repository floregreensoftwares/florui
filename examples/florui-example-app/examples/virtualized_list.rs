//! A real virtualized list: 10,000 rows, but only a bounded window ever
//! mounted at once -- scroll with a real mouse wheel or trackpad and watch
//! the status line above (which reads `VirtualListHandle::offset`) move,
//! while the rest of the dataset never materializes as real `Element`s or
//! `ComponentScope`s at all.
//!
//! `cargo run --example virtualized_list -p florui-example-app`

use florui::Element;
use florui_platform::{ItemHeight, Overscan, use_virtual_list};
use florui_reactive::Key;
use florui_style::Rgba;

const CSS: &str = include_str!("virtualized_list.css");
const ITEM_COUNT: usize = 10_000;
const ITEM_HEIGHT: f32 = 32.0;

fn main() {
    florui_platform::run(
        "Florui virtualized list",
        CSS,
        Rgba::opaque(0x10, 0x10, 0x14),
        widget,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn row(i: usize) -> Element {
    Element::node(
        "div",
        vec![("class".to_string(), "row".to_string())],
        vec![Element::text(format!("Row {i}"))],
    )
}

fn widget() -> Element {
    let (content, handle) = use_virtual_list(
        "virtual-list",
        ITEM_COUNT,
        (),
        ItemHeight::Fixed(ITEM_HEIGHT),
        Overscan::default(),
        Key::from,
        row,
    );

    let (_, scroll_top) = handle.offset();
    let status = Element::node(
        "div",
        vec![("class".to_string(), "status".to_string())],
        vec![Element::text(format!(
            "near row {} of {ITEM_COUNT} -- scroll offset: {scroll_top:.0}",
            (scroll_top / ITEM_HEIGHT) as usize
        ))],
    );
    let scroll_box = Element::node(
        "div",
        vec![
            ("id".to_string(), "virtual-list".to_string()),
            ("class".to_string(), "scroll-box".to_string()),
        ],
        vec![content],
    );

    Element::node(
        "div",
        vec![("class".to_string(), "page".to_string())],
        vec![status, scroll_box],
    )
}

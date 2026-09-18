//! A transparent, borderless desktop widget -- the window itself has no
//! background at all (`Rgba::TRANSPARENT` canvas, `DecorationMode::Custom`),
//! only the cards paint anything, so the real desktop shows through every
//! gap. One button pins the window always-on-top and switches it to
//! `InputMode::Selective`: the header (title, drag region, and the
//! button itself, marked `WINDOW_INPUT_REGION_CLASS`) stays clickable
//! forever, while the cards and every transparent gap pass clicks
//! through to whatever's behind -- no timed auto-revert needed, since
//! the button marking itself interactive is what keeps it reachable.
//!
//! `cargo run --example weather_widget -p florui-example-app`
//!
//! # A note on transparency and `InputMode::Selective`
//!
//! Which pixels are *painted* transparent (the cards' own translucent
//! background, the empty gaps) is unrelated to which pixels *accept
//! input* -- CSS `background-color` alpha never changes what
//! `InputMode` targets, and marking a region `WINDOW_INPUT_REGION_CLASS`
//! doesn't require it to be opaque. This widget's header happens to be
//! both, but that's a choice, not a rule.

use florui::prelude::*;
use florui_platform::appearance::DecorationMode;
use florui_platform::{
    InputMode, WINDOW_DRAG_REGION_ID, WINDOW_INPUT_REGION_CLASS, WindowOptions, use_window_controls,
};
use florui_reactive::use_signal;
use florui_style::Rgba;

const CSS: &str = include_str!("weather_widget.css");

struct Reading {
    place: &'static str,
    temp: &'static str,
    condition: &'static str,
}

const READINGS: [Reading; 3] = [
    Reading {
        place: "Sao Paulo",
        temp: "24C",
        condition: "Ensolarado",
    },
    Reading {
        place: "Tokyo",
        temp: "18C",
        condition: "Nublado",
    },
    Reading {
        place: "Londres",
        temp: "12C",
        condition: "Chuva",
    },
];

fn main() {
    florui_platform::run_with_options(
        "Florui weather widget",
        CSS,
        Rgba::TRANSPARENT,
        WindowOptions {
            decorations: DecorationMode::Custom,
            ..WindowOptions::default()
        },
        widget,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn widget() -> Element {
    let controls = use_window_controls();
    let pinned = use_signal(|| false);

    let toggle_controls = controls.clone();
    let toggle_pinned = pinned.clone();

    view! {
        <div class="widget">
            <div class={format!("header {WINDOW_INPUT_REGION_CLASS}")}>
                <span class="title">{"Clima"}</span>
                <div id={WINDOW_DRAG_REGION_ID} class="drag-region" />
                <button
                    class="pin-button"
                    onclick={move || {
                        let next = !toggle_pinned.get();
                        toggle_pinned.set(next);
                        if let Some(controls) = &toggle_controls {
                            controls.set_always_on_top(next);
                            controls.set_input_mode(if next {
                                InputMode::Selective
                            } else {
                                InputMode::Normal
                            });
                        }
                    }}
                >
                    {if pinned.get() { "Fixado (so o cabecalho recebe clique)" } else { "Fixar no topo" }}
                </button>
            </div>
            <div class="cards">
                {READINGS.iter().map(|reading| view! {
                    <div class="card">
                        <span class="card-label">{reading.place}</span>
                        <span class="card-value">{reading.temp}</span>
                        <span class="card-condition">{reading.condition}</span>
                    </div>
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}

//! Real native open/save file dialogs: click a button, a real Windows
//! dialog opens, and the result -- selected path(s), cancellation, or
//! failure -- shows up in the window once the dialog closes. Both calls
//! are asynchronous: the button click returns immediately, and the
//! window stays fully responsive while the dialog is open.
//!
//! `cargo run --example file_dialogs -p florui-example-app`

use florui::prelude::*;
use florui_platform::{
    FileDialogFilter, OpenFileDialogOptions, OpenFileDialogOutcome, SaveFileDialogOptions,
    SaveFileDialogOutcome, use_window_controls,
};
use florui_reactive::use_signal;
use florui_style::Rgba;

const CSS: &str = include_str!("two_windows.css");

fn main() {
    florui_platform::run(
        "Florui -- native file dialogs",
        CSS,
        Rgba::opaque(0x1e, 0x1e, 0x22),
        root,
    )
    .expect("event loop should not fail on a real desktop session");
}

fn describe_open(outcome: &OpenFileDialogOutcome) -> String {
    match outcome {
        OpenFileDialogOutcome::Selected(paths) => paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        OpenFileDialogOutcome::Cancelled => "(cancelled)".to_owned(),
        OpenFileDialogOutcome::Unavailable => "(unavailable on this platform)".to_owned(),
        OpenFileDialogOutcome::Failed(message) => format!("(failed: {message})"),
    }
}

fn describe_save(outcome: &SaveFileDialogOutcome) -> String {
    match outcome {
        SaveFileDialogOutcome::Selected(path) => path.display().to_string(),
        SaveFileDialogOutcome::Cancelled => "(cancelled)".to_owned(),
        SaveFileDialogOutcome::Unavailable => "(unavailable on this platform)".to_owned(),
        SaveFileDialogOutcome::Failed(message) => format!("(failed: {message})"),
    }
}

fn root() -> Element {
    let controls = use_window_controls();
    let last_open_result = use_signal(|| "(none yet)".to_owned());
    let last_save_result = use_signal(|| "(none yet)".to_owned());

    let open_controls = controls.clone();
    let open_result = last_open_result.clone();
    let handle_open = move || {
        let Some(controls) = &open_controls else {
            return;
        };
        let result = open_result.clone();
        controls.open_file_dialog(
            OpenFileDialogOptions {
                title: Some("Pick one or more images".to_owned()),
                filters: vec![FileDialogFilter {
                    label: "Images".to_owned(),
                    extensions: vec!["png".to_owned(), "jpg".to_owned(), "jpeg".to_owned()],
                }],
                allow_multiple: true,
                starting_directory: None,
            },
            move |outcome| result.set(describe_open(&outcome)),
        );
    };

    let save_controls = controls.clone();
    let save_result = last_save_result.clone();
    let handle_save = move || {
        let Some(controls) = &save_controls else {
            return;
        };
        let result = save_result.clone();
        controls.save_file_dialog(
            SaveFileDialogOptions {
                title: Some("Choose a save destination".to_owned()),
                filters: vec![FileDialogFilter {
                    label: "Text".to_owned(),
                    extensions: vec!["txt".to_owned()],
                }],
                suggested_file_name: Some("florui-demo.txt".to_owned()),
                starting_directory: None,
            },
            move |outcome| result.set(describe_save(&outcome)),
        );
    };

    view! {
        <div class="page">
            <p class="label">{"Native file dialogs"}</p>
            <p class="hint">
                {"Both dialogs are asynchronous -- the window stays responsive while one is open."}
            </p>
            <button class="button" onclick={handle_open}>{"Open file(s)..."}</button>
            <p class="hint">{format!("Last open result:\n{}", last_open_result.get())}</p>
            <button class="button" onclick={handle_save}>{"Save file..."}</button>
            <p class="hint">{format!("Last save result:\n{}", last_save_result.get())}</p>
        </div>
    }
}

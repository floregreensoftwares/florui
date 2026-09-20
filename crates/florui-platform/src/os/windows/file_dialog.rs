//! Native open/save file dialogs via the modern Common Item Dialog
//! (`IFileOpenDialog`/`IFileSaveDialog`) — see `crate::file_dialog`'s own
//! doc for why this needs the `windows` crate instead of this crate's
//! usual `windows-sys` (which has no COM interface support at all: no
//! `Interface` trait, no generated vtables, confirmed by grepping its own
//! source before writing any of this). Every real interaction here —
//! selecting a file, cancelling, multi-select, a suggested save name and
//! starting folder, and running the blocking `Show` call on its own
//! thread while a "caller" keeps running — was confirmed against a real,
//! human-driven dialog in a disposable scratch experiment first.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IBindCtx,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog,
    IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HSTRING, PCWSTR};
use winit::window::Window;

use crate::file_dialog::{
    FileDialogFilter, OpenFileDialogOptions, OpenFileDialogOutcome, SaveFileDialogOptions,
    SaveFileDialogOutcome, filter_patterns,
};
use crate::os::windows::raw_hwnd;

/// The exact HRESULT `IFileDialog::Show` returns when the user closes the
/// dialog without choosing anything -- confirmed against a real Cancel
/// click in the scratch experiment this module's own doc mentions.
const ERROR_CANCELLED_HRESULT: i32 = 0x800704C7u32 as i32;

/// A raw HWND value carried across the background thread this module
/// spawns. Sound because the real `winit::window::Window` is never
/// touched from that thread -- only this plain value, already extracted
/// from it on the caller's own (UI) thread before spawning.
struct SendableHwnd(HWND);
// Safety: see this struct's own doc.
unsafe impl Send for SendableHwnd {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn spawn_open_dialog(
    window: &Window,
    options: OpenFileDialogOptions,
    on_complete: impl Fn(OpenFileDialogOutcome) + Send + 'static,
) {
    let Some(hwnd) = raw_hwnd(window) else {
        on_complete(OpenFileDialogOutcome::Unavailable);
        return;
    };
    let hwnd = SendableHwnd(hwnd);
    std::thread::spawn(move || {
        let hwnd = hwnd;
        on_complete(show_open_dialog(&hwnd, &options));
    });
}

pub(crate) fn spawn_save_dialog(
    window: &Window,
    options: SaveFileDialogOptions,
    on_complete: impl Fn(SaveFileDialogOutcome) + Send + 'static,
) {
    let Some(hwnd) = raw_hwnd(window) else {
        on_complete(SaveFileDialogOutcome::Unavailable);
        return;
    };
    let hwnd = SendableHwnd(hwnd);
    std::thread::spawn(move || {
        let hwnd = hwnd;
        on_complete(show_save_dialog(&hwnd, &options));
    });
}

/// Owns the wide-string buffers `COMDLG_FILTERSPEC` entries point into,
/// so they stay valid for as long as the dialog might read them (up
/// through `Show`, which can redraw the file-type dropdown at any time,
/// not only during the `SetFileTypes` call itself).
struct FilterBuffers {
    _labels: Vec<Vec<u16>>,
    _patterns: Vec<Vec<u16>>,
    specs: Vec<COMDLG_FILTERSPEC>,
}

fn build_filter_buffers(filters: &[FileDialogFilter]) -> FilterBuffers {
    let mut labels = Vec::with_capacity(filters.len());
    let mut patterns = Vec::with_capacity(filters.len());
    for (label, pattern) in filter_patterns(filters) {
        labels.push(wide(&label));
        patterns.push(wide(&pattern));
    }
    let specs = labels
        .iter()
        .zip(patterns.iter())
        .map(|(label, pattern)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(label.as_ptr()),
            pszSpec: PCWSTR(pattern.as_ptr()),
        })
        .collect();
    FilterBuffers {
        _labels: labels,
        _patterns: patterns,
        specs,
    }
}

/// Resolves `path` to a real `IShellItem` for `IFileDialog::SetFolder`.
/// `None` (logged, non-fatal -- a starting folder is a convenience, not a
/// requirement) if the path can't be resolved; the dialog falls back to
/// its own last-used-folder default.
fn shell_item_for_path(path: &std::path::Path) -> Option<IShellItem> {
    let wide_path = wide(&path.to_string_lossy());
    // Safety: wide_path is a valid null-terminated wide string, live for
    // the duration of this call.
    let item =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None::<&IBindCtx>) };
    match item {
        Ok(item) => Some(item),
        Err(error) => {
            eprintln!(
                "florui-platform: could not resolve starting folder {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn show_open_dialog(hwnd: &SendableHwnd, options: &OpenFileDialogOptions) -> OpenFileDialogOutcome {
    // Safety: COINIT_APARTMENTTHREADED is required for shell dialogs;
    // CoUninitialize below is called before this function returns, on
    // this same (background) thread.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() {
        return OpenFileDialogOutcome::Unavailable;
    }
    let outcome = show_open_dialog_inner(hwnd, options);
    // Safety: matches the CoInitializeEx call above, same thread.
    unsafe { CoUninitialize() };
    outcome
}

fn show_open_dialog_inner(
    hwnd: &SendableHwnd,
    options: &OpenFileDialogOptions,
) -> OpenFileDialogOutcome {
    // Safety: FileOpenDialog is the documented CLSID for this coclass;
    // CLSCTX_INPROC_SERVER is the standard context for shell dialogs.
    let dialog: windows::core::Result<IFileOpenDialog> =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) };
    let dialog = match dialog {
        Ok(dialog) => dialog,
        Err(_) => return OpenFileDialogOutcome::Unavailable,
    };

    let filter_buffers = build_filter_buffers(&options.filters);
    if !filter_buffers.specs.is_empty() {
        // Safety: filter_buffers (and the wide-string buffers its specs
        // point into) outlives this call and the Show call below.
        let _ = unsafe { dialog.SetFileTypes(&filter_buffers.specs) };
    }
    if let Some(title) = &options.title {
        let _ = unsafe { dialog.SetTitle(&HSTRING::from(title.as_str())) };
    }
    if let Some(directory) = &options.starting_directory
        && let Some(item) = shell_item_for_path(directory)
    {
        let _ = unsafe { dialog.SetFolder(&item) };
    }
    if options.allow_multiple
        && let Ok(existing) = unsafe { dialog.GetOptions() }
    {
        let _ = unsafe { dialog.SetOptions(existing | FOS_ALLOWMULTISELECT) };
    }

    // Safety: hwnd.0 is a real HWND value extracted from the owning
    // window before this thread was spawned.
    match unsafe { dialog.Show(Some(hwnd.0)) } {
        Ok(()) => {
            // Safety: Show succeeded, so results are available.
            let Ok(items) = (unsafe { dialog.GetResults() }) else {
                return OpenFileDialogOutcome::Failed(
                    "GetResults failed after a successful Show".to_owned(),
                );
            };
            let count = unsafe { items.GetCount() }.unwrap_or(0);
            let mut paths = Vec::with_capacity(count as usize);
            for index in 0..count {
                let Ok(item) = (unsafe { items.GetItemAt(index) }) else {
                    continue;
                };
                let Ok(name) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else {
                    continue;
                };
                // Safety: name is a valid, null-terminated PWSTR from a
                // successful GetDisplayName call.
                if let Ok(path) = unsafe { name.to_string() } {
                    paths.push(std::path::PathBuf::from(path));
                }
            }
            if paths.is_empty() {
                OpenFileDialogOutcome::Failed(
                    "dialog reported success but no usable path was returned".to_owned(),
                )
            } else {
                OpenFileDialogOutcome::Selected(paths)
            }
        }
        Err(error) if error.code().0 == ERROR_CANCELLED_HRESULT => OpenFileDialogOutcome::Cancelled,
        Err(error) => OpenFileDialogOutcome::Failed(error.to_string()),
    }
}

fn show_save_dialog(hwnd: &SendableHwnd, options: &SaveFileDialogOptions) -> SaveFileDialogOutcome {
    // Safety: matches show_open_dialog's own contract.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() {
        return SaveFileDialogOutcome::Unavailable;
    }
    let outcome = show_save_dialog_inner(hwnd, options);
    // Safety: matches the CoInitializeEx call above, same thread.
    unsafe { CoUninitialize() };
    outcome
}

fn show_save_dialog_inner(
    hwnd: &SendableHwnd,
    options: &SaveFileDialogOptions,
) -> SaveFileDialogOutcome {
    // Safety: FileSaveDialog is the documented CLSID for this coclass.
    let dialog: windows::core::Result<IFileSaveDialog> =
        unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) };
    let dialog = match dialog {
        Ok(dialog) => dialog,
        Err(_) => return SaveFileDialogOutcome::Unavailable,
    };

    let filter_buffers = build_filter_buffers(&options.filters);
    if !filter_buffers.specs.is_empty() {
        // Safety: matches show_open_dialog_inner's own SetFileTypes call.
        let _ = unsafe { dialog.SetFileTypes(&filter_buffers.specs) };
    }
    if let Some(title) = &options.title {
        let _ = unsafe { dialog.SetTitle(&HSTRING::from(title.as_str())) };
    }
    if let Some(name) = &options.suggested_file_name {
        let _ = unsafe { dialog.SetFileName(&HSTRING::from(name.as_str())) };
    }
    if let Some(directory) = &options.starting_directory
        && let Some(item) = shell_item_for_path(directory)
    {
        let _ = unsafe { dialog.SetFolder(&item) };
    }

    // Safety: hwnd.0 is a real HWND value extracted from the owning
    // window before this thread was spawned. IFileSaveDialog shares
    // Show/GetResult with IFileDialog via COM interface inheritance.
    match unsafe { dialog.Show(Some(hwnd.0)) } {
        Ok(()) => {
            let Ok(item) = (unsafe { dialog.GetResult() }) else {
                return SaveFileDialogOutcome::Failed(
                    "GetResult failed after a successful Show".to_owned(),
                );
            };
            let Ok(name) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else {
                return SaveFileDialogOutcome::Failed(
                    "GetDisplayName failed after a successful Show".to_owned(),
                );
            };
            // Safety: name is a valid, null-terminated PWSTR from a
            // successful GetDisplayName call.
            match unsafe { name.to_string() } {
                Ok(path) => SaveFileDialogOutcome::Selected(std::path::PathBuf::from(path)),
                Err(error) => SaveFileDialogOutcome::Failed(error.to_string()),
            }
        }
        Err(error) if error.code().0 == ERROR_CANCELLED_HRESULT => SaveFileDialogOutcome::Cancelled,
        Err(error) => SaveFileDialogOutcome::Failed(error.to_string()),
    }
}

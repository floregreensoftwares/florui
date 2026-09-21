//! [`run`]: pairs [`UiRuntime`] with a real `winit` window, a `softbuffer`
//! surface, and an event loop, so a caller doesn't have to write its own
//! desktop event loop just to see a component tree running. [`run_windows`]
//! is the same thing generalized to any number of windows at once, each
//! independently titled/styled/iconed — [`run`]/[`run_with_options`]/
//! [`run_with_css_reload`]/[`run_with_css_reload_and_options`] are thin
//! wrappers over it for the common one-window case.
//!
//! HiDPI-aware: layout runs against the window's *logical* size (via
//! [`crate::dpi`]), matching `florui_style`'s CSS-style `width`/`height`;
//! the committed boxes are then scaled back up by the window's own scale
//! factor before painting, so the canvas renders at full physical device
//! resolution rather than blurrily upscaling a logical-resolution one.
//! Cursor positions (physical, from `winit`) are converted the other way
//! before hit-testing against that same logical layout.

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use florui::Element;
use florui_layout::BoxLayout;
use florui_reactive::provide_context;
use florui_style::{NodeId, Rgba, StyleError};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use taffy::prelude::*;
use winit::application::ApplicationHandler;
#[cfg(test)]
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::UiRuntime;
use crate::activation::{ActivationEvent, ActivationEvents, ActivationQueue, SingleInstance};
use crate::appearance::DecorationMode;
use crate::dpi::{self, ViewportScale};
use crate::drag_drop::{self, DragDropRegistration};
use crate::file_dialog::{OpenFileDialogOutcome, SaveFileDialogOutcome};
use crate::gpu::{self, GpuPresenter};
use crate::single_instance::{self, HandoffOutcome, InstanceRole};
use crate::window_controls::{InputMode, ScreenRect, WindowControls};
use crate::window_state::{self, WindowPersistence};

#[derive(Debug)]
pub enum RunError {
    EventLoop(winit::error::EventLoopError),
    Stylesheet(StyleError),
    WindowCreation(winit::error::OsError),
    SurfaceCreation(softbuffer::SoftBufferError),
    CssFile(std::io::Error),
    CssWatch(notify::Error),
    /// The named mutex or named pipe itself could not be set up (e.g. the
    /// security descriptor failed to build) — distinct from a normal
    /// hand-off failure, which is reported through [`RunOutcome`] instead
    /// since it isn't fatal to this process.
    SingleInstance(std::io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::EventLoop(err) => write!(f, "event loop failed: {err}"),
            RunError::Stylesheet(err) => write!(f, "stylesheet failed to parse: {err}"),
            RunError::WindowCreation(err) => write!(f, "window could not be created: {err}"),
            RunError::SurfaceCreation(err) => {
                write!(f, "render surface could not be created: {err}")
            }
            RunError::CssFile(err) => write!(f, "could not read stylesheet file: {err}"),
            RunError::CssWatch(err) => write!(f, "could not watch stylesheet file: {err}"),
            RunError::SingleInstance(err) => {
                write!(f, "single-instance setup failed: {err}")
            }
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::EventLoop(err) => Some(err),
            RunError::Stylesheet(err) => Some(err),
            RunError::WindowCreation(err) => Some(err),
            RunError::SurfaceCreation(err) => Some(err),
            RunError::CssFile(err) => Some(err),
            RunError::CssWatch(err) => Some(err),
            RunError::SingleInstance(err) => Some(err),
        }
    }
}

/// Every variant now carries the `WindowId` it's about -- with more than
/// one window live, each of these needs to say which one, whereas a
/// single-window host had nothing to disambiguate.
enum UserEvent {
    /// Either a [`florui_reactive::Signal`] changed somewhere under this
    /// window's root, or a [`florui_reactive::use_resource`] fetch became
    /// newly pollable — see [`UiRuntime::on_needs_update`]. Both call for
    /// the same reaction: re-render and repaint.
    Dirty(WindowId),
    /// The watched CSS file (see [`run_with_css_reload`]) changed on disk.
    CssChanged(WindowId),
    /// A [`crate::WindowControls::close`] call from inside the component
    /// tree, routed back through the event loop so it takes the exact
    /// same close path a real `WindowEvent::CloseRequested` (the OS's own
    /// close button, still live even under [`DecorationMode::Custom`] via,
    /// e.g., Alt+F4) — one real shutdown path, not two that could drift
    /// apart.
    RequestClose(WindowId),
    /// An [`ActivationEvent`] arrived from another launch of this same
    /// application — see [`run_single_instance`]. Not per-window, unlike
    /// every other variant here: activation isn't scoped to one window.
    Activation(ActivationEvent),
    /// A [`crate::WindowControls::open_file_dialog`]/`save_file_dialog`
    /// call's background thread finished — routed back through the event
    /// loop so the app's own `on_result` callback runs on the UI thread,
    /// where touching `Signal`s is safe.
    OpenFileDialogResult(WindowId, OpenFileDialogOutcome),
    SaveFileDialogResult(WindowId, SaveFileDialogOutcome),
}

/// What [`run_with_options`]/[`run_with_css_reload_and_options`] ask for
/// about the real window's own chrome — its own struct (not bare
/// parameters) so a later addition doesn't need a new `run_with_*_and_*`
/// function of its own. [`run`]/[`run_with_css_reload`] are thin wrappers
/// over these two with [`WindowOptions::default`] (system decorations, no
/// explicit size, opaque), so every existing caller keeps working
/// unchanged.
///
/// `size`/`min_size` are logical units (see this module's own HiDPI note),
/// `None` meaning "let the platform choose" exactly as today's behavior
/// with no explicit size request. `transparent` is carried here for
/// fidelity with `florui-config`'s own `window.transparent` field, but has
/// no effect on real window creation yet — see [`gpu::transparent_capable_attributes`]'s
/// own doc: a transparent-capable surface is already requested
/// unconditionally for every window, regardless of this option. `icon` is
/// the creation-time half of per-window icons — see
/// [`crate::WindowControls::set_icon`] for updating an already-open
/// window's icon instead. `respect_reduced_motion` (default `true`) is
/// the opt-out for this crate's own automatic transition/`@keyframes`
/// suppression when the real OS prefers reduced motion — `false` restores
/// plain, unsuppressed CSS animation regardless of that OS preference; the
/// `@media (prefers-reduced-motion: ...)` query itself always reflects OS
/// truth either way, see [`florui_style::AnimationTimeline`]'s own doc.
/// `theme` (default [`ThemePreference::System`]) is the same "follow the
/// OS, or force it" shape as `decorations` — see [`crate::theme`]'s own
/// module doc for why, unlike reduced motion, an explicit override here
/// *does* replace what `@media (prefers-color-scheme: ...)` itself
/// reports, rather than leaving a separately-preserved OS truth: `winit`
/// itself doesn't keep one once an override is set.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowOptions {
    pub decorations: DecorationMode,
    pub size: Option<(f64, f64)>,
    pub min_size: Option<(f64, f64)>,
    pub transparent: bool,
    pub icon: Option<florui_icon::RawIcon>,
    pub respect_reduced_motion: bool,
    pub theme: crate::theme::ThemePreference,
    /// `None` (the default) persists nothing -- see
    /// [`crate::WindowPersistence`]'s own doc for the app-side resolution
    /// pattern.
    pub persistence: Option<WindowPersistence>,
}

/// Not `#[derive(Default)]`: every field but `respect_reduced_motion`
/// matches what the derive would have given (unset/off/system-decorated),
/// but that one field's required default (`true`) is not the same as
/// `bool`'s own derived default (`false`) — a derive here would silently
/// invert it for every existing caller of [`WindowOptions::default`]
/// ([`run`], [`run_with_css_reload`]). `theme`'s own derived default
/// ([`ThemePreference::System`]) would have been fine, but it stays here
/// too, alongside the field it now can't be separated from without
/// re-deriving `Default` and reintroducing exactly the risk above.
impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            decorations: DecorationMode::default(),
            size: None,
            min_size: None,
            transparent: false,
            icon: None,
            respect_reduced_motion: true,
            theme: crate::theme::ThemePreference::default(),
            persistence: None,
        }
    }
}

/// Opens a window titled `title` and keeps it live over `root` — called
/// fresh on every render, the way a `#[component]` function normally is.
/// `css` is parsed once; it does not get watched for changes — for a dev
/// loop that reloads edited CSS without losing component state, use
/// [`run_with_css_reload`] instead. System-decorated — for an
/// application-drawn title bar instead, use [`run_with_options`].
///
/// Blocks the calling thread until the window closes.
pub fn run(
    title: &str,
    css: &str,
    canvas_color: Rgba,
    root: impl Fn() -> Element + 'static,
) -> Result<(), RunError> {
    run_with_options(title, css, canvas_color, WindowOptions::default(), root)
}

/// Same as [`run`], but with explicit control over the real window's own
/// chrome — see [`WindowOptions`]'s own doc. Under
/// [`DecorationMode::Custom`], `root`'s own component tree reaches
/// [`crate::use_window_controls`] to actually drive the window it no
/// longer has OS-drawn chrome to drive it for free.
///
/// Blocks the calling thread until the window closes.
pub fn run_with_options(
    title: &str,
    css: &str,
    canvas_color: Rgba,
    options: WindowOptions,
    root: impl Fn() -> Element + 'static,
) -> Result<(), RunError> {
    let spec = WindowSpec::new(title, css, canvas_color, options, root)?;
    run_windows(vec![spec])
}

/// Same as [`run`], but reads `css_path` from disk and watches it for
/// changes instead of taking a fixed string: saving an edit re-parses the
/// stylesheet and repaints through [`UiRuntime::set_rules`], which never
/// touches component state — every `Signal` keeps its value across the
/// reload, unlike a Rust source change, which needs an actual process
/// restart (and does lose it; see `florui dev`'s own reporting of that).
///
/// A reload that fails to parse is reported to stderr and the previous,
/// still-valid stylesheet keeps rendering — only the *initial* read at
/// startup must succeed. System-decorated — for an application-drawn
/// title bar instead, use [`run_with_css_reload_and_options`]. Blocks the
/// calling thread until the window closes.
pub fn run_with_css_reload(
    title: &str,
    css_path: impl AsRef<Path>,
    canvas_color: Rgba,
    root: impl Fn() -> Element + 'static,
) -> Result<(), RunError> {
    run_with_css_reload_and_options(
        title,
        css_path,
        canvas_color,
        WindowOptions::default(),
        root,
    )
}

/// Same as [`run_with_css_reload`], but with explicit control over the
/// real window's own chrome — see [`WindowOptions`]'s own doc and
/// [`run_with_options`]'s own note on [`DecorationMode::Custom`]. Blocks
/// the calling thread until the window closes.
pub fn run_with_css_reload_and_options(
    title: &str,
    css_path: impl AsRef<Path>,
    canvas_color: Rgba,
    options: WindowOptions,
    root: impl Fn() -> Element + 'static,
) -> Result<(), RunError> {
    let spec = WindowSpec::with_css_reload(title, css_path, canvas_color, options, root)?;
    run_windows(vec![spec])
}

/// Opens every window in `initial` on one shared event loop and blocks the
/// calling thread until the last one closes. If any window in `initial`
/// fails to create, the whole batch is treated as fatal (matching
/// [`run_with_options`]'s own single-window failure behavior) — this is
/// only reachable at startup, not once windows are already running.
pub fn run_windows(initial: Vec<WindowSpec>) -> Result<(), RunError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(RunError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut host = DesktopHost {
        windows: HashMap::new(),
        pending: initial,
        proxy: event_loop.create_proxy(),
        fatal_error: None,
        activation_queue: None,
        primary_window_id: None,
    };
    event_loop.run_app(&mut host).map_err(RunError::EventLoop)?;
    match host.fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// What [`run_single_instance`] actually did — distinct from [`RunError`],
/// since a failed hand-off is not fatal to this process; it's a normal
/// outcome the caller decides how to react to (e.g. become primary itself
/// instead).
#[derive(Debug)]
pub enum RunOutcome {
    /// This process was (or became) primary and its event loop ran to
    /// completion — the same as a plain [`run_windows`] call returning
    /// `Ok`.
    Ran,
    /// `launch_event` was delivered to, and acknowledged by, the
    /// already-running primary instance. No window was ever created —
    /// the caller should exit.
    HandedOff,
    /// Another instance owns the single-instance mutex, but the hand-off
    /// itself did not complete (the owner was unreachable, died mid-transfer,
    /// or never acknowledged within the configured timeout). The
    /// activation data is handed back rather than silently discarded —
    /// this must never be reported as [`RunOutcome::HandedOff`], and the
    /// caller decides what to do next (e.g. call [`run_windows`] itself,
    /// deliberately becoming a fresh primary).
    HandoffFailed(ActivationEvent),
}

/// Same as [`run_windows`], but first enforces `config`'s opt-in
/// single-instance behavior: exactly one process at a time owns
/// `config.app_identifier`'s OS-backed mutex (see
/// `crate::os::windows::single_instance`'s own doc — Windows-only for
/// now; every other platform always becomes primary, see
/// `crate::single_instance`'s stub). `launch_event` is this process's own
/// activation payload — sent to the existing primary if this process
/// turns out to be secondary, or the first event a fresh primary's own
/// [`crate::use_activation_events`] sees, whichever applies.
///
/// A secondary instance never creates a window or touches an `EventLoop`
/// at all — it performs the hand-off synchronously and returns.
pub fn run_single_instance(
    config: SingleInstance,
    launch_event: ActivationEvent,
    initial: Vec<WindowSpec>,
) -> Result<RunOutcome, RunError> {
    match single_instance::acquire(&config.app_identifier).map_err(RunError::SingleInstance)? {
        InstanceRole::Secondary => {
            match single_instance::handoff(
                &config.app_identifier,
                &launch_event,
                config.handoff_timeout,
            ) {
                HandoffOutcome::Delivered => Ok(RunOutcome::HandedOff),
                HandoffOutcome::Failed => Ok(RunOutcome::HandoffFailed(launch_event)),
            }
        }
        InstanceRole::Primary(mutex_ownership) => {
            let event_loop = EventLoop::<UserEvent>::with_user_event()
                .build()
                .map_err(RunError::EventLoop)?;
            event_loop.set_control_flow(ControlFlow::Wait);

            let activation_queue = Rc::new(RefCell::new(ActivationQueue::default()));
            let proxy = event_loop.create_proxy();
            single_instance::spawn_activation_listener(&config.app_identifier, move |event| {
                let _ = proxy.send_event(UserEvent::Activation(event));
            })
            .map_err(RunError::SingleInstance)?;
            // This process's own launch event goes straight into the same
            // queue the primary window's first render will read — no pipe
            // round trip needed for the process that already owns the
            // mutex.
            activation_queue.borrow_mut().push(launch_event);

            // Keeps the mutex held for as long as this process is primary
            // -- dropped (releasing it) only once `run_app` below returns,
            // i.e. when every window has closed and this process is about
            // to exit.
            let _mutex_ownership = mutex_ownership;

            let mut host = DesktopHost {
                windows: HashMap::new(),
                pending: initial,
                proxy: event_loop.create_proxy(),
                fatal_error: None,
                activation_queue: Some(activation_queue),
                primary_window_id: None,
            };
            event_loop.run_app(&mut host).map_err(RunError::EventLoop)?;
            match host.fatal_error {
                Some(error) => Err(error),
                None => Ok(RunOutcome::Ran),
            }
        }
    }
}

/// Everything needed to open one window, not yet created — consumed
/// exactly once by [`DesktopHost::resumed`], which drains a `Vec` of
/// these. Build one with [`WindowSpec::new`] or
/// [`WindowSpec::with_css_reload`].
pub struct WindowSpec {
    title: String,
    canvas_color: Rgba,
    rules: Vec<florui_style::Rule>,
    options: WindowOptions,
    root: Box<dyn Fn() -> Element>,
    /// `Some` => this window watches and live-reloads this file's CSS
    /// (see [`run_with_css_reload`]'s own doc); `None` => static CSS,
    /// parsed once.
    css_path: Option<PathBuf>,
}

impl WindowSpec {
    /// `css` is parsed immediately, so a syntax error is reported before
    /// any window opens rather than after.
    pub fn new(
        title: impl Into<String>,
        css: &str,
        canvas_color: Rgba,
        options: WindowOptions,
        root: impl Fn() -> Element + 'static,
    ) -> Result<Self, RunError> {
        let rules = florui_style::parse_stylesheet(css).map_err(RunError::Stylesheet)?;
        Ok(Self {
            title: title.into(),
            canvas_color,
            rules,
            options,
            root: Box::new(root),
            css_path: None,
        })
    }

    /// Same as [`Self::new`], but reads `css_path` from disk and watches
    /// it for changes once this window is live — see
    /// [`run_with_css_reload`]'s own doc for the reload contract.
    pub fn with_css_reload(
        title: impl Into<String>,
        css_path: impl AsRef<Path>,
        canvas_color: Rgba,
        options: WindowOptions,
        root: impl Fn() -> Element + 'static,
    ) -> Result<Self, RunError> {
        let css_path = css_path.as_ref().to_owned();
        let css = std::fs::read_to_string(&css_path).map_err(RunError::CssFile)?;
        let rules = florui_style::parse_stylesheet(&css).map_err(RunError::Stylesheet)?;
        Ok(Self {
            title: title.into(),
            canvas_color,
            rules,
            options,
            root: Box::new(root),
            css_path: Some(css_path),
        })
    }

    /// Like [`Self::new`], but for already-parsed rules instead of one raw
    /// CSS string — infallible, since parsing already happened. The route
    /// for multiple `StylesheetSource`s (e.g. one or more
    /// `stylesheet_scoped!` declarations plus any plain `stylesheet!`
    /// ones): compile each through `florui_style::compile_sources` first,
    /// which applies each scoped source's own class-selector rewrite
    /// before parsing, something a single concatenated CSS string
    /// couldn't represent (a scope boundary needs to be a property of one
    /// particular source, not the whole merged text). No CSS hot reload —
    /// same tradeoff as [`Self::new`].
    pub fn with_rules(
        title: impl Into<String>,
        rules: Vec<florui_style::Rule>,
        canvas_color: Rgba,
        options: WindowOptions,
        root: impl Fn() -> Element + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            canvas_color,
            rules,
            options,
            root: Box::new(root),
            css_path: None,
        }
    }
}

/// Watches `css_path`'s parent directory (not the file itself, so editors
/// that save via rename/replace are still observed) and wakes the event
/// loop only on a change to `css_path` exactly, tagged with `window_id` so
/// the right [`WindowState`] reloads — mirrors
/// `florui-devtools::preview::watch_fixture`.
fn watch_css_file(
    css_path: &Path,
    proxy: EventLoopProxy<UserEvent>,
    window_id: WindowId,
) -> notify::Result<RecommendedWatcher> {
    let target = css_path
        .canonicalize()
        .unwrap_or_else(|_| css_path.to_owned());
    let parent = css_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        let touches_target = event
            .paths
            .iter()
            .any(|p| p.canonicalize().map(|c| c == target).unwrap_or(false));
        if touches_target {
            // The event loop may already be gone; nothing to do if so.
            let _ = proxy.send_event(UserEvent::CssChanged(window_id));
        }
    })?;
    watcher.watch(parent, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

fn layout_viewport(scale: ViewportScale) -> Size<AvailableSpace> {
    Size {
        width: AvailableSpace::Definite(scale.logical.width),
        height: AvailableSpace::Definite(scale.logical.height),
    }
}

/// Scales every committed box from the logical pixels layout ran against
/// up to physical pixels, so painting can rasterize at full device
/// resolution instead of the canvas's own (unscaled) unit.
fn scale_layouts(layouts: &HashMap<NodeId, BoxLayout>, factor: f32) -> HashMap<NodeId, BoxLayout> {
    layouts
        .iter()
        .map(|(&id, layout)| {
            (
                id,
                BoxLayout {
                    x: layout.x * factor,
                    y: layout.y * factor,
                    width: layout.width * factor,
                    height: layout.height * factor,
                },
            )
        })
        .collect()
}

/// Collects every [`crate::WINDOW_INPUT_REGION_CLASS`] element's real
/// screen rectangle from this frame's own committed layout and hands
/// them to [`WindowControls::sync_input_regions`] — only called under
/// [`InputMode::Selective`], so an app that never uses it pays nothing.
fn sync_input_regions(
    controls: &WindowControls,
    window: &Window,
    arena: &florui_style::Arena,
    physical_layouts: &HashMap<NodeId, BoxLayout>,
) {
    let Ok(origin) = window.inner_position() else {
        return;
    };
    let regions: Vec<ScreenRect> = physical_layouts
        .iter()
        .filter(|&(&id, _)| {
            arena
                .classes(id)
                .iter()
                .any(|class| class == crate::WINDOW_INPUT_REGION_CLASS)
        })
        .map(|(_, layout)| ScreenRect {
            left: origin.x + layout.x.round() as i32,
            top: origin.y + layout.y.round() as i32,
            right: origin.x + (layout.x + layout.width).round() as i32,
            bottom: origin.y + (layout.y + layout.height).round() as i32,
        })
        .collect();
    controls.sync_input_regions(&regions);
}

/// Whichever presentation path [`DesktopHost::resumed`] actually got —
/// see [`crate::gpu`]'s own doc for the fallback contract between them.
/// `Cpu` is still `softbuffer`, unchanged from before this existed.
enum Presenter {
    Gpu(Box<GpuPresenter>),
    Cpu {
        surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
        _context: softbuffer::Context<Arc<Window>>,
    },
}

/// One real, live window: its actual `winit::window::Window`, real
/// presenter (GPU-preferred, `softbuffer` fallback — see [`Presenter`]),
/// and the [`UiRuntime`] rendering it — every rendering, hit-testing, and
/// dispatch decision for this one window delegates to that runtime. Only
/// ever exists post-creation (built once in [`DesktopHost::resumed`] from
/// a drained [`WindowSpec`]), so nothing here needs `Option` wrapping the
/// way [`WindowSpec`]'s own fields don't need it either.
struct WindowState {
    canvas_color: Rgba,
    runtime: UiRuntime,
    /// The node hit-tested at the last left-button press, if any — a
    /// click only dispatches on release over this same node.
    pressed: Option<NodeId>,
    last_cursor: (f64, f64),
    window: Arc<Window>,
    /// Also reachable from the component tree via
    /// [`crate::use_window_controls`] — kept here too so
    /// [`Self::handle_press`] can recognize a press on
    /// [`crate::WINDOW_DRAG_REGION_ID`] and start a real window drag
    /// itself, without a component needing to wire that up by hand.
    controls: Rc<WindowControls>,
    presenter: Presenter,
    /// When a `transition`/`@keyframes` animation still needs sampling —
    /// `None` once nothing is animating. Read and rescheduled only in
    /// [`ApplicationHandler::about_to_wait`], which runs after every loop
    /// iteration regardless of what triggered it (input, a `Signal::set`,
    /// or a previous animation wake), so it always sees whatever the most
    /// recent [`Self::redraw`] left here — no separate cross-thread
    /// signal needed to drive continuous repaints.
    next_animation_wake: Option<std::time::Instant>,
    /// Only set for a window built via [`WindowSpec::with_css_reload`] —
    /// [`Self::reload_css`] is a no-op without it.
    css_path: Option<PathBuf>,
    /// Kept alive only to keep watching; dropping it stops delivery.
    _css_watcher: Option<RecommendedWatcher>,
    /// Kept alive only to keep this window's drop target registered;
    /// dropping it revokes it. `None` when registration itself failed
    /// (see `crate::drag_drop::register`'s own doc) — drag-and-drop is
    /// then simply unavailable for this window, not a fatal error.
    _drag_drop: Option<DragDropRegistration>,
    /// What this window asked for — an explicit `Light`/`Dark` override
    /// makes it correctly immune to a live `WindowEvent::ThemeChanged`
    /// (the window is already immune on the `winit` side too once an
    /// override is set; see [`crate::theme`]'s own module doc), checked in
    /// [`Self::handle_theme_changed`].
    theme_preference: crate::theme::ThemePreference,
    /// `Some` when this window opted into bounds persistence — read at
    /// close time (see [`DesktopHost::close_if_confirmed`]) to flush a
    /// final save.
    persistence: Option<WindowPersistence>,
    /// `Some` once this window's bounds changed since the last successful
    /// save; cleared on save. Reset (not just refreshed) on every further
    /// change, so a continuous resize drag keeps pushing the save deadline
    /// out — a true debounce, not a fixed-interval throttle — instead of
    /// saving mid-drag on every qualifying tick.
    pending_geometry_save: Option<std::time::Instant>,
    /// Updated only by a live `WindowEvent::ModifiersChanged` — a real
    /// `KeyEvent` carries no modifier state of its own, so Shift+Tab needs
    /// this to distinguish itself from a plain Tab.
    modifiers: ModifiersState,
}

/// Long enough that a drag-resize (many `Resized`/`Moved` events per
/// second) collapses into one save after the user stops; short enough
/// that a crash/kill within a second or two of the last move doesn't lose
/// much. Not spec-mandated to an exact number, only "bounded/debounced."
const WINDOW_STATE_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

/// Logical pixels one wheel "line" (`MouseScrollDelta::LineDelta`'s own
/// unit) scrolls — real mouse wheels report in lines, not pixels, so this
/// is the conversion factor into the logical pixels a scroll offset is
/// measured in. Not spec-mandated to an exact number, the same as
/// [`WINDOW_STATE_SAVE_DEBOUNCE`]: browsers commonly use a value in this
/// range for the same conversion.
const WHEEL_LINE_HEIGHT: f32 = 40.0;

impl WindowState {
    fn viewport_scale(&self) -> ViewportScale {
        dpi::viewport_scale(self.window.inner_size(), self.window.scale_factor())
    }

    /// Converts a physical-pixel cursor position (as `winit` reports it)
    /// to the logical pixels layout runs against.
    fn to_logical_cursor(&self, x: f64, y: f64) -> (f32, f32) {
        let factor = self.viewport_scale().scale_factor;
        ((x / factor) as f32, (y / factor) as f32)
    }

    /// A real wheel/trackpad event's delta, converted to the logical
    /// pixels a scroll *offset* moves by — a real mouse wheel reports
    /// whole "lines" ([`WHEEL_LINE_HEIGHT`] logical pixels each), a
    /// trackpad reports already-fine-grained physical pixels needing only
    /// the same physical-to-logical scale [`Self::to_logical_cursor`]
    /// already applies to cursor positions.
    ///
    /// Negated on both axes: `winit`'s own `MouseScrollDelta` doc defines a
    /// positive value as "content...should move right and down (revealing
    /// more content left and up)" — the opposite of this offset's own
    /// scroll-position convention (also the DOM's `wheel` event
    /// convention), where a positive value reveals more content
    /// right/down by *increasing* the offset, not moving the content
    /// itself right/down. Scrolling down (revealing lower content) must
    /// increase `offset.1`, not decrease it.
    fn to_logical_scroll_delta(&self, delta: MouseScrollDelta) -> (f32, f32) {
        match delta {
            MouseScrollDelta::LineDelta(x, y) => (-x * WHEEL_LINE_HEIGHT, -y * WHEEL_LINE_HEIGHT),
            MouseScrollDelta::PixelDelta(position) => {
                let factor = self.viewport_scale().scale_factor;
                ((-position.x / factor) as f32, (-position.y / factor) as f32)
            }
        }
    }

    /// The nearest ancestor of `node` (`node` itself included) that is
    /// real CSS's `overflow: scroll`/`auto` on the axis `(dx, dy)` actually
    /// moves along, has more real content than its own viewport on that
    /// axis, and carries an `id` attribute with a live
    /// [`crate::use_scroll_offset`] registration for it — real CSS lets an
    /// `overflow: auto` element with nothing to scroll pass a wheel event
    /// through to a further ancestor, and an ancestor this scroll registry
    /// has never heard of (no matching `use_scroll_offset` call, not just
    /// no `id`) has no offset to move in the first place. `None` when no
    /// ancestor qualifies — the event is simply not a scroll anywhere.
    fn scrollable_ancestor_id(&self, node: NodeId, dx: f32, dy: f32) -> Option<String> {
        let (arena, styles, ..) = self.runtime.geometry();
        let registry = self.runtime.scroll_registry();
        let mut current = Some(node);
        while let Some(id) = current {
            if let Some(style) = styles.get(&id)
                && let Some(attr_id) = arena.id_attr(id)
            {
                let wants_x = dx != 0.0 && style.overflow_scrolls_x;
                let wants_y = dy != 0.0 && style.overflow_scrolls_y;
                let (viewport_w, viewport_h) = registry.viewport_size(attr_id);
                let (content_w, content_h) = registry.content_size(attr_id);
                let scrolls_x = wants_x && content_w > viewport_w;
                let scrolls_y = wants_y && content_h > viewport_h;
                if scrolls_x || scrolls_y {
                    return Some(attr_id.to_string());
                }
            }
            current = arena.parent(id);
        }
        None
    }

    /// Real mouse-wheel/trackpad input, hit-tested and routed to whichever
    /// scrollable ancestor actually owns it — see
    /// [`Self::scrollable_ancestor_id`]. Keyboard-driven scrolling (arrow
    /// keys, Page Up/Down, Home/End) is a real, documented gap: no focus
    /// model exists anywhere in this crate yet for a keyboard event to
    /// resolve *which* element it should even move.
    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let (dx, dy) = self.to_logical_scroll_delta(delta);
        let (x, y) = self.to_logical_cursor(self.last_cursor.0, self.last_cursor.1);
        let Some(hit) = self.runtime.hit_test(x, y) else {
            return;
        };
        let Some(id) = self.scrollable_ancestor_id(hit, dx, dy) else {
            return;
        };
        if self.runtime.scroll_registry().scroll_by(&id, dx, dy) {
            let viewport = layout_viewport(self.viewport_scale());
            self.runtime.update(viewport);
            self.window.request_redraw();
            self.refresh_animation_schedule();
        }
    }

    fn redraw(&mut self) {
        let scale_factor = self.viewport_scale().scale_factor;
        let window = self.window.clone();
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };

        let scroll_registry = self.runtime.scroll_registry();
        let (arena, styles, layouts, font) = self.runtime.geometry_and_font_mut();
        let scroll_offsets = scroll_registry.offsets_by_node(arena);
        let scrolled_layouts = florui_layout::apply_scroll_offsets(arena, layouts, &scroll_offsets);
        let physical_layouts = scale_layouts(&scrolled_layouts, scale_factor as f32);
        if self.controls.input_mode() == InputMode::Selective {
            sync_input_regions(&self.controls, &window, arena, &physical_layouts);
        }
        let canvas = florui_paint::paint_to_buffer(
            font,
            size.width,
            size.height,
            self.canvas_color,
            arena,
            styles,
            &physical_layouts,
            scale_factor as f32,
        );

        match &mut self.presenter {
            Presenter::Gpu(presenter) => {
                // tiny-skia's own pixel format is RGBA byte order,
                // premultiplied — matches `GpuPresenter`'s own upload
                // texture format exactly, so the painted bytes go straight
                // across with no channel swizzle.
                presenter.resize(width.get(), height.get());
                presenter.present(canvas.data());
            }
            Presenter::Cpu { surface, .. } => {
                if let Err(error) = surface.resize(width, height) {
                    eprintln!("florui-platform: could not resize the render surface: {error}");
                    return;
                }
                let mut buffer = match surface.buffer_mut() {
                    Ok(buffer) => buffer,
                    Err(error) => {
                        eprintln!("florui-platform: render surface buffer unavailable: {error}");
                        return;
                    }
                };
                // `softbuffer`'s own pixel format has no alpha channel at
                // all (see `crate::appearance`'s own doc) — the canvas is
                // always painted fully opaque today regardless, so
                // dropping alpha here is a no-op, not a lossy conversion.
                let pixels: Vec<u32> = canvas
                    .pixels()
                    .iter()
                    .map(|p| u32::from_be_bytes([0, p.red(), p.green(), p.blue()]))
                    .collect();
                buffer.copy_from_slice(&pixels);
                if let Err(error) = buffer.present() {
                    eprintln!("florui-platform: could not present the frame: {error}");
                }
            }
        }
    }

    /// The real handler for `WindowEvent::RedrawRequested`: if the reason
    /// this frame was requested is [`Self::next_animation_wake`] having
    /// come due, advances the animation timeline first so the paint below
    /// reflects the current instant — otherwise this is a plain repaint
    /// (a resize, an exposed region) and [`Self::redraw`] alone is
    /// correct, matching the design's own separation of "update" from
    /// "paint" everywhere else.
    fn redraw_for_frame(&mut self) {
        let deadline_due = self
            .next_animation_wake
            .is_some_and(|deadline| std::time::Instant::now() >= deadline);
        if deadline_due {
            let viewport = layout_viewport(self.viewport_scale());
            self.runtime
                .set_os_prefers_reduced_motion(crate::accessibility::prefers_reduced_motion());
            self.runtime.update(viewport);
            self.refresh_animation_schedule();
        }
        self.redraw();
    }

    /// Re-renders against the current viewport and requests a repaint —
    /// used both after a resize and after a [`UserEvent::Dirty`], so any
    /// `Signal::set` anywhere under the root reaches the screen without
    /// the host having to know which specific interaction caused it.
    fn update_and_request_redraw(&mut self) {
        let viewport = layout_viewport(self.viewport_scale());
        self.runtime.clear_dirty();
        self.runtime
            .set_os_prefers_reduced_motion(crate::accessibility::prefers_reduced_motion());
        self.runtime.update(viewport);
        self.window.request_redraw();
        self.refresh_animation_schedule();
    }

    /// Recomputes [`Self::next_animation_wake`] from
    /// [`UiRuntime::is_animating`] — called after every `runtime.update`
    /// site, so whichever one most recently ran always leaves an accurate
    /// deadline for [`ApplicationHandler::about_to_wait`] to act on. A
    /// stale deadline is intentional between calls: a real transition/
    /// animation samples at whatever instant it actually gets painted at
    /// (`Transition::calculate_value` is time-based, not tied to landing
    /// exactly on a 16ms boundary), so nothing here needs to be exact —
    /// only to keep asking for another frame while it's still true.
    fn refresh_animation_schedule(&mut self) {
        self.next_animation_wake = self
            .runtime
            .is_animating()
            .then(|| std::time::Instant::now() + std::time::Duration::from_millis(16));
    }

    /// Re-derives `InputMode::Selective`'s screen-space regions from the
    /// runtime's already-computed layout — no re-render, just the new
    /// window offset applied to geometry that hasn't otherwise changed.
    fn resync_input_regions(&self) {
        if self.controls.input_mode() != InputMode::Selective {
            return;
        }
        let scale_factor = self.viewport_scale().scale_factor;
        let (arena, _, layouts) = self.runtime.geometry();
        let physical_layouts = scale_layouts(layouts, scale_factor as f32);
        sync_input_regions(&self.controls, &self.window, arena, &physical_layouts);
    }

    /// Marks this window's bounds as changed since the last save, resetting
    /// (not just refreshing) the debounce deadline — see
    /// [`WINDOW_STATE_SAVE_DEBOUNCE`]'s own doc. A no-op window without
    /// persistence enabled.
    fn mark_geometry_dirty(&mut self) {
        if self.persistence.is_some() {
            self.pending_geometry_save = Some(std::time::Instant::now());
        }
    }

    /// Writes this window's current geometry if a change is pending and its
    /// debounce deadline has passed, clearing the pending flag either way
    /// (an unreachable-monitor/disabled edge case never reaches this with
    /// `persistence: None`, since [`Self::mark_geometry_dirty`] never sets
    /// it then). Called from [`ApplicationHandler::about_to_wait`] once per
    /// loop iteration, and unconditionally (regardless of any pending
    /// deadline) from [`DesktopHost::close_if_confirmed`] before this
    /// window's state is dropped.
    fn flush_geometry_save_if_due(&mut self, now: std::time::Instant) {
        let Some(persistence) = &self.persistence else {
            return;
        };
        let Some(changed_at) = self.pending_geometry_save else {
            return;
        };
        if now < changed_at + WINDOW_STATE_SAVE_DEBOUNCE {
            return;
        }
        window_state::capture_and_save(&self.window, persistence);
        self.pending_geometry_save = None;
    }

    /// Updates `:hover` against the runtime's cached geometry — no
    /// rebuild just to know what's under the cursor.
    fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        self.last_cursor = (x, y);
        let (x, y) = self.to_logical_cursor(x, y);
        let hit = self
            .runtime
            .hit_test(x, y)
            .filter(|&node| !self.is_disabled(node));
        self.set_hovered_and_redraw(hit);
    }

    /// The cursor leaving the window cancels any in-progress press (there
    /// is nowhere left to release onto) and clears `:hover` — otherwise
    /// whichever element was last under the cursor would stay visually
    /// `:hover`ed even after the mouse has left the window entirely.
    fn handle_cursor_left(&mut self) {
        self.pressed = None;
        self.set_hovered_and_redraw(None);
    }

    /// Applies a hover change and, only when it actually changed anything
    /// (`:hover` can affect computed style), re-renders and requests a
    /// repaint to pick that up.
    fn set_hovered_and_redraw(&mut self, hit: Option<NodeId>) {
        let viewport = layout_viewport(self.viewport_scale());
        if !self.runtime.set_hovered(hit) {
            return;
        }
        self.runtime
            .set_os_prefers_reduced_motion(crate::accessibility::prefers_reduced_motion());
        self.runtime.update(viewport);
        self.window.request_redraw();
        self.refresh_animation_schedule();
    }

    /// Reacts to a real, live OS theme change — only when this window
    /// asked to follow it ([`crate::theme::ThemePreference::System`]); an
    /// explicit override already ignores this on the `winit` side (see
    /// [`crate::theme`]'s own module doc), so this check just keeps
    /// florui's own signal consistent with what the window is actually
    /// doing. Unlike reduced motion, no per-frame polling is needed
    /// anywhere in this file: `WindowEvent::ThemeChanged` tells this host
    /// exactly when the value actually changes.
    fn handle_theme_changed(&mut self, theme: winit::window::Theme) {
        if self.theme_preference != crate::theme::ThemePreference::System {
            return;
        }
        let viewport = layout_viewport(self.viewport_scale());
        self.runtime
            .set_prefers_dark_color_scheme(crate::theme::ColorScheme::from(theme).is_dark());
        self.runtime.update(viewport);
        self.window.request_redraw();
        self.refresh_animation_schedule();
    }

    /// Remembers whichever node is under the cursor at press time — the
    /// click itself only fires on release, and only if that release lands
    /// back on this same node (so dragging off a button and releasing
    /// elsewhere cancels it). A press that lands exactly on
    /// [`crate::WINDOW_DRAG_REGION_ID`] is a different gesture entirely —
    /// see that constant's own doc — and starts a real window drag
    /// instead of ever becoming a click candidate; `winit`'s own
    /// `drag_window` takes over the mouse for the rest of this gesture,
    /// so there is no matching press to remember here.
    fn handle_press(&mut self) {
        let (x, y) = self.to_logical_cursor(self.last_cursor.0, self.last_cursor.1);
        let hit = self.runtime.hit_test(x, y);

        if hit.is_some_and(|node| self.is_drag_region(node)) {
            self.controls.drag();
            return;
        }
        // A disabled button must not become `pressed`: `handle_release`
        // sets focus purely from `pressed` being `Some`, before it ever
        // calls `dispatch_click` -- excluding it here is what stops a
        // mouse click from focusing a disabled button, which gating
        // `dispatch_click` alone can't (that only stops the click's own
        // handler from firing).
        self.pressed = hit.filter(|&node| !self.is_disabled(node));
    }

    fn is_drag_region(&self, node: NodeId) -> bool {
        let (arena, ..) = self.runtime.geometry();
        arena.id_attr(node) == Some(crate::WINDOW_DRAG_REGION_ID)
    }

    /// Tag-gated the same as `florui_platform::focus::is_focusable` and
    /// `UiRuntime::dispatch_click`: `disabled` has no wired behavior
    /// outside `<button>` in v1. Used to keep a disabled button out of
    /// `pressed` (see `handle_press`) and out of `:hover` (see
    /// `handle_cursor_moved`) -- real browsers don't deliver pointer
    /// events to a disabled control either.
    fn is_disabled(&self, node: NodeId) -> bool {
        let (arena, ..) = self.runtime.geometry();
        arena.tag(node) == "button" && arena.is_disabled(node)
    }

    fn should_close(&self) -> bool {
        self.controls.confirm_close()
    }

    /// Re-reads and re-parses the watched CSS file (see
    /// [`WindowSpec::with_css_reload`]), swaps it into the running
    /// [`UiRuntime`] via [`UiRuntime::set_rules`] — never rebuilding the
    /// tree, so every `Signal` keeps its value — and repaints. A failure
    /// (bad syntax, a save-in-progress truncated read) is reported and the
    /// last good stylesheet keeps rendering, the same recovery contract
    /// the native inspector's own fixture preview already established.
    fn reload_css(&mut self) {
        let Some(path) = self.css_path.clone() else {
            return;
        };
        let loaded = std::fs::read_to_string(&path)
            .map_err(RunError::CssFile)
            .and_then(|css| florui_style::parse_stylesheet(&css).map_err(RunError::Stylesheet));
        match loaded {
            Ok(rules) => {
                self.runtime.set_rules(rules);
                println!(
                    "florui-platform: stylesheet reloaded from {}",
                    path.display()
                );
                self.update_and_request_redraw();
            }
            Err(error) => {
                eprintln!(
                    "florui-platform: stylesheet reload failed, keeping last good version: {error}"
                );
            }
        }
    }

    fn handle_release(&mut self) {
        let (x, y) = self.to_logical_cursor(self.last_cursor.0, self.last_cursor.1);
        let pressed = self.pressed.take();
        let released_over = self.runtime.hit_test(x, y);
        if let (Some(pressed), Some(released_over)) = (pressed, released_over)
            && pressed == released_over
        {
            // Matches real HTML: a click sets keyboard focus to its
            // target too, just not :focus-visible (via_keyboard: false).
            let focus_changed = self.runtime.set_focused(Some(pressed), false);
            self.runtime.dispatch_click(pressed);
            if focus_changed {
                self.update_and_request_redraw();
            }
        }
    }

    /// The real handler for `WindowEvent::KeyboardInput`. Only a fresh
    /// key-down does anything: a held key's own repeat must not re-fire
    /// activation, and a key-up carries no action of its own here. Tab/
    /// Shift+Tab move focus; Enter/Space activate whatever is currently
    /// focused through the same [`UiRuntime::dispatch_click`] a real
    /// mouse click already uses — no second event name invented.
    fn handle_keyboard_input(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        match event.logical_key {
            Key::Named(NamedKey::Tab) => {
                let moved = if self.modifiers.shift_key() {
                    self.runtime.focus_previous()
                } else {
                    self.runtime.focus_next()
                };
                if moved {
                    self.update_and_request_redraw();
                }
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                if let Some(focused) = self.runtime.focused() {
                    self.runtime.dispatch_click(focused);
                }
            }
            _ => {}
        }
    }
}

/// Owns every currently-open window and whichever [`WindowSpec`]s haven't
/// been created yet. `windows` is the single source of truth for "is a
/// window still open" — closing one removes its entry, and the process
/// exits once the map is empty (the simplest correct "last window closes
/// the app" default; a fully customizable close/focus/last-window
/// contract across many windows is separate, larger work this does not
/// attempt).
struct DesktopHost {
    windows: HashMap<WindowId, WindowState>,
    pending: Vec<WindowSpec>,
    proxy: EventLoopProxy<UserEvent>,
    fatal_error: Option<RunError>,
    /// `Some` only under [`run_single_instance`] — shared with the
    /// pipe-listener thread's `UserEvent::Activation` sends and with the
    /// primary window's own [`ActivationEvents`] context (see `resumed`).
    activation_queue: Option<Rc<RefCell<ActivationQueue>>>,
    /// The first window `resumed` ever creates — the sole window
    /// [`ActivationEvents`] context is provided to and the sole target
    /// `UserEvent::Activation` redraws. Multi-window fan-out is
    /// deliberately out of scope: a queue shared by every window would
    /// make `take_pending` a race over which window's render drains it
    /// first.
    primary_window_id: Option<WindowId>,
}

impl DesktopHost {
    /// Stops the event loop after logging `error`, and keeps it so
    /// [`run_windows`] can return it once `run_app` unwinds — an
    /// `ApplicationHandler` method has no return value of its own to
    /// report failure through.
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: RunError) {
        eprintln!("florui-platform: {error}");
        self.fatal_error = Some(error);
        event_loop.exit();
    }

    /// Shared by the OS's own `CloseRequested` and
    /// [`UserEvent::RequestClose`] — one real close path for `id`, both
    /// askable to veto via that window's own
    /// [`WindowState::should_close`]. Removes just that window's entry;
    /// only exits the whole process once none are left.
    fn close_if_confirmed(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let Some(state) = self.windows.get(&id) else {
            return;
        };
        if !state.should_close() {
            return;
        }
        if let Some(persistence) = &state.persistence {
            window_state::capture_and_save(&state.window, persistence);
        }
        self.windows.remove(&id);
        if self.windows.is_empty() {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<UserEvent> for DesktopHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `mem::take` (not `self.pending.drain(..)`) so the loop owns its
        // own `Vec` independently of `self` -- `self.fail` below needs a
        // fresh `&mut self`, which a live borrow from `drain` would
        // conflict with. A `spec` not yet reached when the function
        // returns early (a failure partway through the batch) is dropped
        // along with the rest of this now-local `Vec`'s `IntoIter`,
        // matching "the whole batch fails together."
        for spec in std::mem::take(&mut self.pending) {
            // Always requested — harmless for whichever presenter actually
            // ends up used (see `gpu::transparent_capable_attributes`'s own
            // doc), and `crate::gpu::GpuPresenter::try_new` needs the window
            // to have already been created with these attributes to have any
            // chance at real `TransparentSurface` compositing.
            let mut attrs = Window::default_attributes()
                .with_title(spec.title.clone())
                .with_decorations(matches!(spec.options.decorations, DecorationMode::System))
                .with_theme(crate::theme::requested_winit_theme(spec.options.theme));
            attrs = drag_drop::disable_builtin_drag_and_drop(attrs);
            if let Some((width, height)) = spec.options.size {
                attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(width, height));
            }
            if let Some((width, height)) = spec.options.min_size {
                attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(width, height));
            }
            // A malformed icon doesn't take the window down with it --
            // logged and skipped, matching the CSS-reload-failure
            // precedent ("report it, keep what already works") rather
            // than window/surface-creation's own fatal-batch precedent.
            if let Some(icon) = &spec.options.icon {
                match crate::window_controls::to_winit_icon(icon) {
                    Ok(winit_icon) => attrs = attrs.with_window_icon(Some(winit_icon)),
                    Err(error) => {
                        eprintln!("florui-platform: window icon could not be applied: {error}");
                    }
                }
            }
            // Restores persisted bounds, revalidated against currently
            // connected monitors -- a saved position that no longer
            // overlaps any of them is a full miss, not partially honored,
            // so a title/drag region is never left unreachable off-screen.
            // A saved size with no saved position (never recorded, e.g. no
            // `Moved` event ever fired) still applies -- losing position
            // alone is a smaller compromise than losing everything.
            let mut should_restore_maximized = false;
            if let Some(persistence) = &spec.options.persistence {
                let saved =
                    window_state::window_state_path(&persistence.app_identifier, &persistence.key)
                        .as_deref()
                        .and_then(window_state::load_or_default);
                if let Some(saved) = saved {
                    let monitors: Vec<window_state::MonitorRect> = event_loop
                        .available_monitors()
                        .map(|monitor| window_state::MonitorRect {
                            position: (monitor.position().x, monitor.position().y),
                            physical_size: (monitor.size().width, monitor.size().height),
                            scale_factor: monitor.scale_factor(),
                        })
                        .collect();
                    let restorable_position = saved.position.filter(|&position| {
                        window_state::overlapping_monitor(
                            saved.logical_size,
                            position,
                            saved.monitor.scale_factor,
                            &monitors,
                        )
                        .is_some()
                    });
                    if saved.position.is_none() || restorable_position.is_some() {
                        let clamped =
                            window_state::clamp_to_min(saved.logical_size, spec.options.min_size);
                        attrs = attrs
                            .with_inner_size(winit::dpi::LogicalSize::new(clamped.0, clamped.1));
                        if let Some(position) = restorable_position {
                            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(
                                position.0, position.1,
                            ));
                        }
                        should_restore_maximized = saved.maximized;
                    }
                }
            }
            let attrs = gpu::transparent_capable_attributes(attrs);
            let window = match event_loop.create_window(attrs) {
                Ok(window) => Arc::new(window),
                Err(error) => return self.fail(event_loop, RunError::WindowCreation(error)),
            };
            let window_id = window.id();
            // The very first window this host ever creates, across its
            // whole lifetime -- `pending` is only ever drained once (see
            // this function's own doc), so this is unambiguous.
            let is_primary_window = self.primary_window_id.is_none();
            if is_primary_window {
                self.primary_window_id = Some(window_id);
            }
            if should_restore_maximized {
                window.set_maximized(true);
            }

            // GPU-preferred, `softbuffer` fallback — see `crate::gpu`'s own
            // doc for the two-tier (three-way, counting this CPU path)
            // capability contract this implements.
            let presenter = match GpuPresenter::try_new(window.clone()) {
                Some(gpu_presenter) => Presenter::Gpu(Box::new(gpu_presenter)),
                None => {
                    let context = match softbuffer::Context::new(window.clone()) {
                        Ok(context) => context,
                        Err(error) => {
                            return self.fail(event_loop, RunError::SurfaceCreation(error));
                        }
                    };
                    let surface = match softbuffer::Surface::new(&context, window.clone()) {
                        Ok(surface) => surface,
                        Err(error) => {
                            return self.fail(event_loop, RunError::SurfaceCreation(error));
                        }
                    };
                    Presenter::Cpu {
                        surface,
                        _context: context,
                    }
                }
            };

            let viewport = layout_viewport(dpi::viewport_scale(
                window.inner_size(),
                window.scale_factor(),
            ));

            // Reachable from the component tree via `crate::use_window_controls`
            // from this runtime's very first render onward — see
            // `UiRuntime::with_rules_and_context`'s own doc for why that needs
            // to be a constructor argument rather than registered afterward.
            let controls_window = window.clone();
            let close_proxy = self.proxy.clone();
            let open_dialog_proxy = self.proxy.clone();
            let save_dialog_proxy = self.proxy.clone();
            let controls = Rc::new(WindowControls::new(
                controls_window,
                move || {
                    let _ = close_proxy.send_event(UserEvent::RequestClose(window_id));
                },
                move |outcome| {
                    let _ = open_dialog_proxy
                        .send_event(UserEvent::OpenFileDialogResult(window_id, outcome));
                },
                move |outcome| {
                    let _ = save_dialog_proxy
                        .send_event(UserEvent::SaveFileDialogResult(window_id, outcome));
                },
            ));
            let drag_drop_registration = drag_drop::register(&window, Rc::clone(&controls));
            let mut context_providers: Vec<Box<dyn Fn()>> = {
                let controls = Rc::clone(&controls);
                vec![Box::new(move || {
                    provide_context(Rc::clone(&controls));
                })]
            };
            // Only the primary window ever gets this context -- see
            // `DesktopHost::primary_window_id`'s own doc for why.
            if is_primary_window && let Some(queue) = &self.activation_queue {
                let events = ActivationEvents(Rc::clone(queue));
                context_providers.push(Box::new(move || {
                    provide_context(events.clone());
                }));
            }

            let respect_reduced_motion = spec.options.respect_reduced_motion;
            let theme_preference = spec.options.theme;
            let initial_color_scheme =
                crate::theme::effective_color_scheme(theme_preference, &window);
            let mut runtime = UiRuntime::with_rules_and_context(
                spec.rules,
                spec.root,
                viewport,
                context_providers,
                respect_reduced_motion,
                crate::accessibility::prefers_reduced_motion(),
                initial_color_scheme.is_dark(),
            );
            let proxy = self.proxy.clone();
            runtime.on_needs_update(move || {
                let _ = proxy.send_event(UserEvent::Dirty(window_id));
            });
            // The listener above can only be registered after the runtime (and
            // the first render its constructor already ran) exists — so an
            // initial mount effect that itself calls `Signal::set` marks the
            // flag with nothing listening yet, and that mark would otherwise
            // be lost: nothing else re-checks it before the first paint.
            if runtime.is_dirty() {
                runtime.clear_dirty();
                runtime.update(viewport);
            }

            let css_watcher = match &spec.css_path {
                Some(path) => match watch_css_file(path, self.proxy.clone(), window_id) {
                    Ok(watcher) => Some(watcher),
                    Err(error) => return self.fail(event_loop, RunError::CssWatch(error)),
                },
                None => None,
            };

            let mut state = WindowState {
                canvas_color: spec.canvas_color,
                runtime,
                pressed: None,
                last_cursor: (0.0, 0.0),
                window,
                controls,
                presenter,
                next_animation_wake: None,
                css_path: spec.css_path,
                _css_watcher: css_watcher,
                _drag_drop: drag_drop_registration,
                theme_preference,
                persistence: spec.options.persistence.clone(),
                pending_geometry_save: None,
                modifiers: ModifiersState::empty(),
            };
            state.redraw();
            // A `@keyframes` animation already running on mount (no `:hover`
            // or other interaction needed to start it) needs a deadline set
            // here — every other call site only does this after an `update()`
            // this constructor's own first render already ran.
            state.refresh_animation_schedule();
            self.windows.insert(window_id, state);
        }
    }

    /// Runs after every loop iteration, whatever triggered it (input, a
    /// `UserEvent`, a previous animation wake) — the one place this host
    /// decides the control flow for the *next* iteration. Redraws are
    /// requested per window independently (each may be animating on its
    /// own schedule); the next `WaitUntil` deadline is the *minimum* across
    /// every window still waiting on one, so no window's animation lags
    /// behind another's.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = std::time::Instant::now();
        let mut next_wake: Option<std::time::Instant> = None;
        for state in self.windows.values_mut() {
            match state.next_animation_wake {
                Some(deadline) if now >= deadline => state.window.request_redraw(),
                Some(deadline) => {
                    next_wake = Some(next_wake.map_or(deadline, |current| current.min(deadline)));
                }
                None => {}
            }
            state.flush_geometry_save_if_due(now);
            if let Some(changed_at) = state.pending_geometry_save {
                let save_deadline = changed_at + WINDOW_STATE_SAVE_DEBOUNCE;
                next_wake =
                    Some(next_wake.map_or(save_deadline, |current| current.min(save_deadline)));
            }
        }
        match next_wake {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Handled before looking up `windows` mutably -- closing calls
        // `self.close_if_confirmed`, a `&mut self` method, which a live
        // `&mut WindowState` borrow (below) would conflict with.
        if matches!(event, WindowEvent::CloseRequested) {
            self.close_if_confirmed(event_loop, window_id);
            return;
        }
        let Some(state) = self.windows.get_mut(&window_id) else {
            return;
        };
        match event {
            WindowEvent::Resized(_) => {
                state.update_and_request_redraw();
                state.mark_geometry_dirty();
            }
            // Fires on its own — not bundled into `Resized` — when the
            // window moves to a display with a different scale factor, or
            // the OS scale setting changes live; `viewport_scale` re-reads
            // `window.scale_factor()` fresh every call, so re-rendering is
            // all this needs.
            WindowEvent::ScaleFactorChanged { .. } => state.update_and_request_redraw(),
            WindowEvent::ThemeChanged(theme) => state.handle_theme_changed(theme),
            // A pure move (dragging the window, snapping it) changes
            // nothing about its content, only where `InputMode::Selective`'s
            // own screen-space regions sit -- resyncing them here, from
            // the already-computed layout, avoids paying for a full
            // re-render on every step of a drag.
            WindowEvent::Moved(_) => {
                state.resync_input_regions();
                state.mark_geometry_dirty();
            }
            WindowEvent::RedrawRequested => state.redraw_for_frame(),
            WindowEvent::CursorMoved { position, .. } => {
                state.handle_cursor_moved(position.x, position.y);
            }
            WindowEvent::CursorLeft { .. } => state.handle_cursor_left(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => state.handle_press(),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => state.handle_release(),
            WindowEvent::MouseWheel { delta, .. } => state.handle_mouse_wheel(delta),
            WindowEvent::ModifiersChanged(modifiers) => state.modifiers = modifiers.state(),
            // `is_synthetic: true` is winit re-synthesizing "this key was
            // already held" on focus gain/loss — must not trigger
            // activation, only a genuine key-down the user just pressed.
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => state.handle_keyboard_input(event),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Dirty(id) => {
                if let Some(state) = self.windows.get_mut(&id) {
                    state.update_and_request_redraw();
                }
            }
            UserEvent::CssChanged(id) => {
                if let Some(state) = self.windows.get_mut(&id) {
                    state.reload_css();
                }
            }
            UserEvent::RequestClose(id) => self.close_if_confirmed(event_loop, id),
            UserEvent::Activation(activation_event) => {
                let Some(queue) = &self.activation_queue else {
                    return;
                };
                queue.borrow_mut().push(activation_event);
                // If the primary window hasn't mounted yet, there's
                // nothing to redraw -- its own first `update()` (already
                // run inside `UiRuntime::with_rules_and_context`) will see
                // this queue's contents, since the same `Rc` is what its
                // context provider reads.
                if let Some(id) = self.primary_window_id
                    && let Some(state) = self.windows.get_mut(&id)
                {
                    state.update_and_request_redraw();
                }
            }
            UserEvent::OpenFileDialogResult(id, outcome) => {
                if let Some(state) = self.windows.get_mut(&id) {
                    state.controls.deliver_open_dialog_result(outcome);
                    state.update_and_request_redraw();
                }
            }
            UserEvent::SaveFileDialogResult(id, outcome) => {
                if let Some(state) = self.windows.get_mut(&id) {
                    state.controls.deliver_save_dialog_result(outcome);
                    state.update_and_request_redraw();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_options_default_respects_reduced_motion() {
        assert!(
            WindowOptions::default().respect_reduced_motion,
            "the manual Default impl must not silently invert this field's required default, \
             the way adding it to a derive would have"
        );
    }

    #[test]
    fn layout_viewport_uses_the_logical_size_not_the_physical_one() {
        let scale = dpi::viewport_scale(PhysicalSize::new(1600, 1200), 2.0);
        let viewport = layout_viewport(scale);
        assert_eq!(viewport.width, AvailableSpace::Definite(800.0));
        assert_eq!(viewport.height, AvailableSpace::Definite(600.0));
    }

    #[test]
    fn scale_layouts_at_1x_is_the_identity() {
        let mut layouts = HashMap::new();
        layouts.insert(
            0,
            BoxLayout {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            },
        );
        let scaled = scale_layouts(&layouts, 1.0);
        assert_eq!(scaled[&0], layouts[&0]);
    }

    #[test]
    fn scale_layouts_at_2x_doubles_every_field() {
        let mut layouts = HashMap::new();
        layouts.insert(
            0,
            BoxLayout {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            },
        );
        let scaled = scale_layouts(&layouts, 2.0);
        assert_eq!(
            scaled[&0],
            BoxLayout {
                x: 20.0,
                y: 40.0,
                width: 200.0,
                height: 100.0,
            }
        );
    }

    #[test]
    fn scale_layouts_preserves_every_node_id_and_only_those() {
        let mut layouts = HashMap::new();
        layouts.insert(
            1,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        );
        layouts.insert(
            2,
            BoxLayout {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        );
        let scaled = scale_layouts(&layouts, 1.5);
        let mut ids: Vec<NodeId> = scaled.keys().copied().collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
    }
}

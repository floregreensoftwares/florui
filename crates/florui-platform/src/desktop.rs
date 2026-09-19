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
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::UiRuntime;
use crate::appearance::DecorationMode;
use crate::dpi::{self, ViewportScale};
use crate::gpu::{self, GpuPresenter};
use crate::window_controls::{InputMode, ScreenRect, WindowControls};

#[derive(Debug)]
pub enum RunError {
    EventLoop(winit::error::EventLoopError),
    Stylesheet(StyleError),
    WindowCreation(winit::error::OsError),
    SurfaceCreation(softbuffer::SoftBufferError),
    CssFile(std::io::Error),
    CssWatch(notify::Error),
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
/// window's icon instead.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WindowOptions {
    pub decorations: DecorationMode,
    pub size: Option<(f64, f64)>,
    pub min_size: Option<(f64, f64)>,
    pub transparent: bool,
    pub icon: Option<florui_icon::RawIcon>,
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
    };
    event_loop.run_app(&mut host).map_err(RunError::EventLoop)?;
    match host.fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
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
}

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

    fn redraw(&mut self) {
        let scale_factor = self.viewport_scale().scale_factor;
        let window = self.window.clone();
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };

        let (arena, styles, layouts, font) = self.runtime.geometry_and_font_mut();
        let physical_layouts = scale_layouts(layouts, scale_factor as f32);
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

    /// Updates `:hover` against the runtime's cached geometry — no
    /// rebuild just to know what's under the cursor.
    fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        self.last_cursor = (x, y);
        let (x, y) = self.to_logical_cursor(x, y);
        let hit = self.runtime.hit_test(x, y);
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
        self.pressed = hit;
    }

    fn is_drag_region(&self, node: NodeId) -> bool {
        let (arena, ..) = self.runtime.geometry();
        arena.id_attr(node) == Some(crate::WINDOW_DRAG_REGION_ID)
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
            self.runtime.dispatch_click(pressed);
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
        if self.windows.get(&id).is_some_and(WindowState::should_close) {
            self.windows.remove(&id);
            if self.windows.is_empty() {
                event_loop.exit();
            }
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
                .with_decorations(matches!(spec.options.decorations, DecorationMode::System));
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
            let attrs = gpu::transparent_capable_attributes(attrs);
            let window = match event_loop.create_window(attrs) {
                Ok(window) => Arc::new(window),
                Err(error) => return self.fail(event_loop, RunError::WindowCreation(error)),
            };
            let window_id = window.id();

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
            let controls = Rc::new(WindowControls::new(controls_window, move || {
                let _ = close_proxy.send_event(UserEvent::RequestClose(window_id));
            }));
            let context_providers: Vec<Box<dyn Fn()>> = {
                let controls = Rc::clone(&controls);
                vec![Box::new(move || {
                    provide_context(Rc::clone(&controls));
                })]
            };

            let mut runtime = UiRuntime::with_rules_and_context(
                spec.rules,
                spec.root,
                viewport,
                context_providers,
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
            WindowEvent::Resized(_) => state.update_and_request_redraw(),
            // Fires on its own — not bundled into `Resized` — when the
            // window moves to a display with a different scale factor, or
            // the OS scale setting changes live; `viewport_scale` re-reads
            // `window.scale_factor()` fresh every call, so re-rendering is
            // all this needs.
            WindowEvent::ScaleFactorChanged { .. } => state.update_and_request_redraw(),
            // A pure move (dragging the window, snapping it) changes
            // nothing about its content, only where `InputMode::Selective`'s
            // own screen-space regions sit -- resyncing them here, from
            // the already-computed layout, avoids paying for a full
            // re-render on every step of a drag.
            WindowEvent::Moved(_) => state.resync_input_regions(),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

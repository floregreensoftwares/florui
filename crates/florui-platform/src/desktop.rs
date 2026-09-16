//! [`run`]: pairs [`UiRuntime`] with a real `winit` window, a `softbuffer`
//! surface, and an event loop, so a caller doesn't have to write its own
//! desktop event loop just to see a component tree running.
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
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::UiRuntime;
use crate::appearance::DecorationMode;
use crate::dpi::{self, ViewportScale};
use crate::gpu::{self, GpuPresenter};
use crate::window_controls::WindowControls;

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

enum UserEvent {
    /// Either a [`florui_reactive::Signal`] changed somewhere under the
    /// root, or a [`florui_reactive::use_resource`] fetch became newly
    /// pollable — see [`UiRuntime::on_needs_update`]. Both call for the
    /// same reaction: re-render and repaint.
    Dirty,
    /// The watched CSS file (see [`run_with_css_reload`]) changed on disk.
    CssChanged,
    /// A [`crate::WindowControls::close`] call from inside the component
    /// tree, routed back through the event loop so it takes the exact
    /// same [`ActiveEventLoop::exit`] path a real
    /// `WindowEvent::CloseRequested` (the OS's own close button, still
    /// live even under [`DecorationMode::Custom`] via, e.g., Alt+F4) —
    /// one real shutdown path, not two that could drift apart.
    RequestClose,
}

/// What [`run_with_options`]/[`run_with_css_reload_and_options`] ask for
/// about the real window's own chrome — currently just `decorations`, but
/// its own struct (not a bare [`DecorationMode`] parameter) so a later
/// addition doesn't need a new `run_with_*_and_*` function of its own.
/// [`run`]/[`run_with_css_reload`] are thin wrappers over these two with
/// [`WindowOptions::default`] (system decorations), so every existing
/// caller keeps working unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowOptions {
    pub decorations: DecorationMode,
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
    let rules = florui_style::parse_stylesheet(css).map_err(RunError::Stylesheet)?;
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(RunError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut host = DesktopHost::new(
        title.to_string(),
        canvas_color,
        rules,
        Box::new(root),
        event_loop.create_proxy(),
        options,
    );
    event_loop.run_app(&mut host).map_err(RunError::EventLoop)?;
    match host.fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
    let css_path = css_path.as_ref().to_owned();
    let css = std::fs::read_to_string(&css_path).map_err(RunError::CssFile)?;
    let rules = florui_style::parse_stylesheet(&css).map_err(RunError::Stylesheet)?;

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(RunError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let watcher = watch_css_file(&css_path, proxy.clone()).map_err(RunError::CssWatch)?;

    let mut host = DesktopHost::new(
        title.to_string(),
        canvas_color,
        rules,
        Box::new(root),
        proxy,
        options,
    );
    host.css_path = Some(css_path);
    host._css_watcher = Some(watcher);

    event_loop.run_app(&mut host).map_err(RunError::EventLoop)?;
    match host.fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Watches `css_path`'s parent directory (not the file itself, so editors
/// that save via rename/replace are still observed) and wakes the event
/// loop only on a change to `css_path` exactly — mirrors
/// `florui-devtools::preview::watch_fixture`.
fn watch_css_file(
    css_path: &Path,
    proxy: EventLoopProxy<UserEvent>,
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
            let _ = proxy.send_event(UserEvent::CssChanged);
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

/// Owns the window, the real presenter (GPU-preferred, `softbuffer`
/// fallback — see [`Presenter`]), and the event loop; delegates every
/// rendering, hit-testing, and dispatch decision to a [`UiRuntime`].
struct DesktopHost {
    title: String,
    canvas_color: Rgba,
    rules: Vec<florui_style::Rule>,
    root: Option<Box<dyn Fn() -> Element>>,
    options: WindowOptions,
    proxy: EventLoopProxy<UserEvent>,
    runtime: Option<UiRuntime>,
    /// The node hit-tested at the last left-button press, if any — a
    /// click only dispatches on release over this same node.
    pressed: Option<NodeId>,
    last_cursor: (f64, f64),
    window: Option<Arc<Window>>,
    /// Also reachable from the component tree via
    /// [`crate::use_window_controls`] — kept here too so
    /// [`Self::handle_press`] can recognize a press on
    /// [`crate::WINDOW_DRAG_REGION_ID`] and start a real window drag
    /// itself, without a component needing to wire that up by hand.
    controls: Option<Rc<WindowControls>>,
    presenter: Option<Presenter>,
    fatal_error: Option<RunError>,
    /// Only set by [`run_with_css_reload`] — [`run`] leaves this `None`,
    /// and [`Self::reload_css`] is a no-op without it.
    css_path: Option<PathBuf>,
    /// Kept alive only to keep watching; dropping it stops delivery.
    _css_watcher: Option<RecommendedWatcher>,
}

impl DesktopHost {
    fn new(
        title: String,
        canvas_color: Rgba,
        rules: Vec<florui_style::Rule>,
        root: Box<dyn Fn() -> Element>,
        proxy: EventLoopProxy<UserEvent>,
        options: WindowOptions,
    ) -> Self {
        Self {
            title,
            canvas_color,
            rules,
            root: Some(root),
            options,
            proxy,
            runtime: None,
            pressed: None,
            last_cursor: (0.0, 0.0),
            window: None,
            controls: None,
            presenter: None,
            fatal_error: None,
            css_path: None,
            _css_watcher: None,
        }
    }

    fn viewport_scale(&self) -> ViewportScale {
        let (size, factor) = self
            .window
            .as_ref()
            .map(|window| (window.inner_size(), window.scale_factor()))
            .unwrap_or((PhysicalSize::default(), 1.0));
        dpi::viewport_scale(size, factor)
    }

    /// Converts a physical-pixel cursor position (as `winit` reports it)
    /// to the logical pixels layout runs against.
    fn to_logical_cursor(&self, x: f64, y: f64) -> (f32, f32) {
        let factor = self.viewport_scale().scale_factor;
        ((x / factor) as f32, (y / factor) as f32)
    }

    /// Stops the event loop after logging `error`, and keeps it so [`run`]
    /// can return it once `run_app` unwinds — an `ApplicationHandler`
    /// method has no return value of its own to report failure through.
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: RunError) {
        eprintln!("florui-platform: {error}");
        self.fatal_error = Some(error);
        event_loop.exit();
    }

    fn redraw(&mut self) {
        // Read before borrowing `self.runtime` mutably below — `viewport_scale`
        // needs `&self` as a whole (it reads `self.window`), which a live
        // `&mut self.runtime` borrow would conflict with.
        let scale_factor = self.viewport_scale().scale_factor;
        let (Some(window), Some(runtime)) = (self.window.clone(), self.runtime.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };

        let (arena, styles, layouts, font) = runtime.geometry_and_font_mut();
        let physical_layouts = scale_layouts(layouts, scale_factor as f32);
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
            Some(Presenter::Gpu(presenter)) => {
                // tiny-skia's own pixel format is RGBA byte order,
                // premultiplied — matches `GpuPresenter`'s own upload
                // texture format exactly, so the painted bytes go straight
                // across with no channel swizzle.
                presenter.resize(width.get(), height.get());
                presenter.present(canvas.data());
            }
            Some(Presenter::Cpu { surface, .. }) => {
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
            None => {}
        }
    }

    /// Re-renders against the current viewport and requests a repaint —
    /// used both after a resize and after a [`UserEvent::Dirty`], so any
    /// `Signal::set` anywhere under the root reaches the screen without
    /// the host having to know which specific interaction caused it.
    fn update_and_request_redraw(&mut self) {
        let viewport = layout_viewport(self.viewport_scale());
        if let Some(runtime) = &mut self.runtime {
            runtime.clear_dirty();
            runtime.update(viewport);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Updates `:hover` against the runtime's cached geometry — no
    /// rebuild just to know what's under the cursor.
    fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        self.last_cursor = (x, y);
        let (x, y) = self.to_logical_cursor(x, y);
        let hit = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.hit_test(x, y));
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
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        if !runtime.set_hovered(hit) {
            return;
        }
        runtime.update(viewport);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
        let hit = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.hit_test(x, y));

        if hit.is_some_and(|node| self.is_drag_region(node)) {
            if let Some(controls) = &self.controls {
                controls.drag();
            }
            return;
        }
        self.pressed = hit;
    }

    fn is_drag_region(&self, node: NodeId) -> bool {
        self.runtime.as_ref().is_some_and(|runtime| {
            let (arena, ..) = runtime.geometry();
            arena.id_attr(node) == Some(crate::WINDOW_DRAG_REGION_ID)
        })
    }

    /// Re-reads and re-parses the watched CSS file (see
    /// [`run_with_css_reload`]), swaps it into the running [`UiRuntime`]
    /// via [`UiRuntime::set_rules`] — never rebuilding the tree, so every
    /// `Signal` keeps its value — and repaints. A failure (bad syntax, a
    /// save-in-progress truncated read) is reported and the last good
    /// stylesheet keeps rendering, the same recovery contract the
    /// native inspector's own fixture preview already established.
    fn reload_css(&mut self) {
        let Some(path) = self.css_path.clone() else {
            return;
        };
        let loaded = std::fs::read_to_string(&path)
            .map_err(RunError::CssFile)
            .and_then(|css| florui_style::parse_stylesheet(&css).map_err(RunError::Stylesheet));
        match loaded {
            Ok(rules) => {
                if let Some(runtime) = &mut self.runtime {
                    runtime.set_rules(rules);
                }
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
        let Some(runtime) = &self.runtime else {
            return;
        };
        let released_over = runtime.hit_test(x, y);
        if let (Some(pressed), Some(released_over)) = (pressed, released_over)
            && pressed == released_over
        {
            runtime.dispatch_click(pressed);
        }
    }
}

impl ApplicationHandler<UserEvent> for DesktopHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Always requested — harmless for whichever presenter actually
        // ends up used (see `gpu::transparent_capable_attributes`'s own
        // doc), and `crate::gpu::GpuPresenter::try_new` needs the window
        // to have already been created with these attributes to have any
        // chance at real `TransparentSurface` compositing.
        let attrs = gpu::transparent_capable_attributes(
            Window::default_attributes()
                .with_title(self.title.clone())
                .with_decorations(matches!(self.options.decorations, DecorationMode::System)),
        );
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(error) => return self.fail(event_loop, RunError::WindowCreation(error)),
        };

        // GPU-preferred, `softbuffer` fallback — see `crate::gpu`'s own
        // doc for the two-tier (three-way, counting this CPU path)
        // capability contract this implements.
        let presenter = match GpuPresenter::try_new(window.clone()) {
            Some(gpu_presenter) => Presenter::Gpu(Box::new(gpu_presenter)),
            None => {
                let context = match softbuffer::Context::new(window.clone()) {
                    Ok(context) => context,
                    Err(error) => return self.fail(event_loop, RunError::SurfaceCreation(error)),
                };
                let surface = match softbuffer::Surface::new(&context, window.clone()) {
                    Ok(surface) => surface,
                    Err(error) => return self.fail(event_loop, RunError::SurfaceCreation(error)),
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
        let root = self
            .root
            .take()
            .expect("resumed only builds the runtime once, guarded by self.window");

        // Reachable from the component tree via `crate::use_window_controls`
        // from this runtime's very first render onward — see
        // `UiRuntime::with_rules_and_context`'s own doc for why that needs
        // to be a constructor argument rather than registered afterward.
        let controls_window = window.clone();
        let close_proxy = self.proxy.clone();
        let controls = Rc::new(WindowControls::new(controls_window, move || {
            let _ = close_proxy.send_event(UserEvent::RequestClose);
        }));
        self.controls = Some(Rc::clone(&controls));
        let context_providers: Vec<Box<dyn Fn()>> = vec![Box::new(move || {
            provide_context(Rc::clone(&controls));
        })];

        let mut runtime = UiRuntime::with_rules_and_context(
            self.rules.clone(),
            root,
            viewport,
            context_providers,
        );
        let proxy = self.proxy.clone();
        runtime.on_needs_update(move || {
            let _ = proxy.send_event(UserEvent::Dirty);
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

        self.runtime = Some(runtime);
        self.window = Some(window);
        self.presenter = Some(presenter);
        self.redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|w| w.id()) != Some(window_id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => self.update_and_request_redraw(),
            // Fires on its own — not bundled into `Resized` — when the
            // window moves to a display with a different scale factor, or
            // the OS scale setting changes live; `viewport_scale` re-reads
            // `window.scale_factor()` fresh every call, so re-rendering is
            // all this needs.
            WindowEvent::ScaleFactorChanged { .. } => self.update_and_request_redraw(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x, position.y);
            }
            WindowEvent::CursorLeft { .. } => self.handle_cursor_left(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.handle_press(),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => self.handle_release(),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Dirty => self.update_and_request_redraw(),
            UserEvent::CssChanged => self.reload_css(),
            UserEvent::RequestClose => event_loop.exit(),
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

//! [`run`]: pairs [`UiRuntime`] with a real `winit` window, a `softbuffer`
//! surface, and an event loop, so a caller doesn't have to write its own
//! desktop event loop just to see a component tree running.
//!
//! This host passes `winit`'s physical-pixel window size straight through
//! as the layout viewport, with no device-pixel-ratio scaling —
//! `florui_style`'s declared `width`/`height` are CSS-style logical
//! pixels, so on a HiDPI display this host lays out and paints as if the
//! window were physically larger than it visually is. Fixing this needs a
//! real logical/physical split through layout and painting, not just
//! here; it is a known, not yet addressed gap, not an oversight to route
//! around.

use std::num::NonZeroU32;
use std::rc::Rc;

use florui::Element;
use florui_style::{NodeId, Rgba, StyleError};
use taffy::prelude::*;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::UiRuntime;

#[derive(Debug)]
pub enum RunError {
    EventLoop(winit::error::EventLoopError),
    Stylesheet(StyleError),
    WindowCreation(winit::error::OsError),
    SurfaceCreation(softbuffer::SoftBufferError),
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
        }
    }
}

enum UserEvent {
    /// Either a [`florui_reactive::Signal`] changed somewhere under the
    /// root, or a [`florui_reactive::use_resource`] fetch became newly
    /// pollable — see [`UiRuntime::on_needs_update`]. Both call for the
    /// same reaction: re-render and repaint.
    Dirty,
}

/// Opens a window titled `title` and keeps it live over `root` — called
/// fresh on every render, the way a `#[component]` function normally is.
/// `css` is parsed once; it does not get watched for changes.
///
/// Blocks the calling thread until the window closes.
pub fn run(
    title: &str,
    css: &str,
    canvas_color: Rgba,
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
    );
    event_loop.run_app(&mut host).map_err(RunError::EventLoop)?;
    match host.fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Owns the window, the `softbuffer` surface, and the event loop; delegates
/// every rendering, hit-testing, and dispatch decision to a [`UiRuntime`].
struct DesktopHost {
    title: String,
    canvas_color: Rgba,
    rules: Vec<florui_style::Rule>,
    root: Option<Box<dyn Fn() -> Element>>,
    proxy: EventLoopProxy<UserEvent>,
    runtime: Option<UiRuntime>,
    /// The node hit-tested at the last left-button press, if any — a
    /// click only dispatches on release over this same node.
    pressed: Option<NodeId>,
    last_cursor: (f64, f64),
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
    fatal_error: Option<RunError>,
}

impl DesktopHost {
    fn new(
        title: String,
        canvas_color: Rgba,
        rules: Vec<florui_style::Rule>,
        root: Box<dyn Fn() -> Element>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        Self {
            title,
            canvas_color,
            rules,
            root: Some(root),
            proxy,
            runtime: None,
            pressed: None,
            last_cursor: (0.0, 0.0),
            window: None,
            surface: None,
            _context: None,
            fatal_error: None,
        }
    }

    fn viewport_size(&self) -> Size<AvailableSpace> {
        let size = self
            .window
            .as_ref()
            .map(|window| window.inner_size())
            .unwrap_or_default();
        Size {
            width: AvailableSpace::Definite(size.width as f32),
            height: AvailableSpace::Definite(size.height as f32),
        }
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
        let (Some(window), Some(runtime)) = (self.window.clone(), &self.runtime) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };

        let (arena, styles, layouts) = runtime.geometry();
        let canvas = florui_paint::paint_to_buffer(
            size.width,
            size.height,
            self.canvas_color,
            arena,
            styles,
            layouts,
        );

        let Some(surface) = &mut self.surface else {
            return;
        };
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
        // Canvas is always opaque, so premultiplied-by-255 is a no-op.
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

    /// Re-renders against the current viewport and requests a repaint —
    /// used both after a resize and after a [`UserEvent::Dirty`], so any
    /// `Signal::set` anywhere under the root reaches the screen without
    /// the host having to know which specific interaction caused it.
    fn update_and_request_redraw(&mut self) {
        let viewport = self.viewport_size();
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
        let hit = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.hit_test(x as f32, y as f32));
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
        let viewport = self.viewport_size();
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
    /// elsewhere cancels it).
    fn handle_press(&mut self) {
        let (x, y) = self.last_cursor;
        self.pressed = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.hit_test(x as f32, y as f32));
    }

    fn handle_release(&mut self) {
        let (x, y) = self.last_cursor;
        let pressed = self.pressed.take();
        let Some(runtime) = &self.runtime else {
            return;
        };
        let released_over = runtime.hit_test(x as f32, y as f32);
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
        let attrs = Window::default_attributes().with_title(self.title.clone());
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Rc::new(window),
            Err(error) => return self.fail(event_loop, RunError::WindowCreation(error)),
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => return self.fail(event_loop, RunError::SurfaceCreation(error)),
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => return self.fail(event_loop, RunError::SurfaceCreation(error)),
        };

        let size = window.inner_size();
        let viewport = Size {
            width: AvailableSpace::Definite(size.width as f32),
            height: AvailableSpace::Definite(size.height as f32),
        };
        let root = self
            .root
            .take()
            .expect("resumed only builds the runtime once, guarded by self.window");
        let mut runtime = UiRuntime::with_rules(self.rules.clone(), root, viewport);
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
        self._context = Some(context);
        self.surface = Some(surface);
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

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Dirty => self.update_and_request_redraw(),
        }
    }
}

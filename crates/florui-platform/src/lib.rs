//! A minimal desktop runner: opens a real window and keeps repainting a
//! component tree through the actual style+layout+paint pipeline, so a
//! caller doesn't have to write its own `winit`/`softbuffer` event loop
//! just to see one running.
//!
//! # Scope
//!
//! One window, one root component, one CSS string parsed once at
//! startup — no hot reload, no multiple windows, no resizable layout
//! beyond whatever the tree's own explicit sizes already produce. Real
//! mouse position drives `:hover`; a real click dispatches whichever
//! `onclick` handler the clicked node declared; a [`florui_reactive::Signal`]
//! set anywhere under the root triggers a real repaint. No keyboard, no
//! focus, no text input.

use std::cell::Cell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;

use florui::Element;
use florui_layout::BoxLayout;
use florui_reactive::Scope;
use florui_style::{Arena, ComputedStyle, InteractionState, NodeId, Rgba, StyleError};
use taffy::prelude::*;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

#[derive(Debug)]
pub enum RunError {
    EventLoop(winit::error::EventLoopError),
    Stylesheet(StyleError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::EventLoop(err) => write!(f, "event loop failed: {err}"),
            RunError::Stylesheet(err) => write!(f, "stylesheet failed to parse: {err}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::EventLoop(err) => Some(err),
            RunError::Stylesheet(err) => Some(err),
        }
    }
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
    let event_loop = EventLoop::new().map_err(RunError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new(title.to_string(), canvas_color, rules, Box::new(root));
    event_loop.run_app(&mut app).map_err(RunError::EventLoop)
}

struct App {
    title: String,
    canvas_color: Rgba,
    rules: Vec<florui_style::Rule>,
    root: Box<dyn Fn() -> Element>,
    scope: Scope,
    dirty: Rc<Cell<bool>>,
    interaction: InteractionState,
    hovered: Option<NodeId>,
    last_cursor: (f64, f64),
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
}

impl App {
    fn new(
        title: String,
        canvas_color: Rgba,
        rules: Vec<florui_style::Rule>,
        root: Box<dyn Fn() -> Element>,
    ) -> Self {
        let (scope, dirty) = Scope::new();
        Self {
            title,
            canvas_color,
            rules,
            root,
            scope,
            dirty,
            interaction: InteractionState::new(),
            hovered: None,
            last_cursor: (0.0, 0.0),
            window: None,
            surface: None,
            _context: None,
        }
    }

    /// Builds a fresh tree and computes its style and layout — no
    /// diffing, a full rebuild every render.
    fn render(
        &mut self,
    ) -> (
        Arena,
        HashMap<NodeId, ComputedStyle>,
        HashMap<NodeId, BoxLayout>,
    ) {
        let tree = self.scope.render(|| (self.root)());
        let arena = Arena::build(&tree);
        let styles = florui_style::compute(&arena, &self.rules, &self.interaction);
        let layouts = florui_layout::compute_layout(&arena, &styles, Size::MAX_CONTENT)
            .expect("this tree's explicit sizes never produce a layout failure");
        (arena, styles, layouts)
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };

        let (arena, styles, layouts) = self.render();
        let canvas = florui_paint::paint_to_buffer(
            size.width,
            size.height,
            self.canvas_color,
            &arena,
            &styles,
            &layouts,
        );

        let Some(surface) = &mut self.surface else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        // Canvas is always opaque, so premultiplied-by-255 is a no-op.
        let pixels: Vec<u32> = canvas
            .pixels()
            .iter()
            .map(|p| u32::from_be_bytes([0, p.red(), p.green(), p.blue()]))
            .collect();
        buffer.copy_from_slice(&pixels);
        let _ = buffer.present();
    }

    fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        self.last_cursor = (x, y);
        let (arena, _styles, layouts) = self.render();
        let hit = florui_layout::hit_test(&arena, &layouts, x as f32, y as f32);
        if hit == self.hovered {
            return;
        }
        self.hovered = hit;
        self.interaction = match hit {
            Some(id) => InteractionState::new().with_hovered(id),
            None => InteractionState::new(),
        };
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Renders once purely to get real geometry to hit-test against,
    /// finds whatever node the cursor is over, and calls its `click`
    /// handler if it has one. Whether that changed anything is the
    /// `Scope`'s own dirty flag's call, not this function's.
    fn handle_click(&mut self) {
        let (arena, _styles, layouts) = self.render();
        let (x, y) = self.last_cursor;
        if let Some(node) = florui_layout::hit_test(&arena, &layouts, x as f32, y as f32)
            && let Some(handler) = arena.handler(node, "click")
        {
            handler.call();
        }

        if self.dirty.get() {
            self.dirty.set(false);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(self.title.clone());
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("window should be creatable on a real desktop session"),
        );
        let context = softbuffer::Context::new(window.clone())
            .expect("softbuffer context should be creatable");
        let surface = softbuffer::Surface::new(&context, window.clone())
            .expect("softbuffer surface should be creatable");

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
            WindowEvent::Resized(_) => self.redraw(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.handle_click(),
            _ => {}
        }
    }
}

//! A clickable counter, live: `+1` is a real `onclick` handler on the
//! button, calling `Signal::set` directly — the host's only job is
//! translating a real mouse click into "which node was that" via
//! `florui_layout::hit_test`, then calling whatever handler it finds. The
//! `Scope`'s dirty flag (not an external "was a click pending" flag)
//! decides whether to repaint.
//!
//! `cargo run --example counter -p florui-example-app`

use std::cell::Cell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;

use florui_example_app::components::counter::{Counter, CounterProps};
use florui_layout::BoxLayout;
use florui_reactive::Scope;
use florui_style::{Arena, ComputedStyle, InteractionState, NodeId, Rgba};
use taffy::prelude::*;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const CANVAS_COLOR: Rgba = Rgba::opaque(0x10, 0x10, 0x14);
const COUNTER_CSS: &str = include_str!("../src/components/counter.css");

fn main() {
    let event_loop =
        EventLoop::new().expect("winit event loop should build on a real desktop session");
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new();
    event_loop
        .run_app(&mut app)
        .expect("event loop should not fail on a real desktop session");
}

struct App {
    scope: Scope,
    dirty: Rc<Cell<bool>>,
    last_cursor: (f64, f64),
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
}

impl App {
    fn new() -> Self {
        let (scope, dirty) = Scope::new();
        Self {
            scope,
            dirty,
            last_cursor: (0.0, 0.0),
            window: None,
            surface: None,
            _context: None,
        }
    }

    /// Builds a fresh tree and computes its style and layout — no diffing,
    /// a full rebuild every render.
    fn render(
        &mut self,
    ) -> (
        Arena,
        HashMap<NodeId, ComputedStyle>,
        HashMap<NodeId, BoxLayout>,
    ) {
        let tree = self.scope.render(|| Counter(CounterProps {}));
        let arena = Arena::build(&tree);
        let rules = florui_style::parse_stylesheet(COUNTER_CSS)
            .expect("counter.css should parse under florui-style's supported subset");
        let styles = florui_style::compute(&arena, &rules, &InteractionState::new());
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
            CANVAS_COLOR,
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

    /// Renders once purely to get real geometry to hit-test against, finds
    /// whatever node the cursor is over, and calls its `click` handler if
    /// it has one. Whether that actually changed anything is the `Scope`'s
    /// own dirty flag's call, not this function's.
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
        let attrs = Window::default_attributes().with_title("Florui counter");
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
                self.last_cursor = (position.x, position.y);
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

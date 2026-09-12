//! A minimal live window for a single, fixed [`element_scene::Scene`].
//!
//! Kept separate from [`crate::preview`]'s CSS-fixture loop rather than
//! extending it: there is no separate style file to watch here, since the
//! tree comes straight from Rust source via `view!`/`#[component]`, and
//! Rust changes need a rebuild/restart regardless — this window shows one
//! scene until closed, nothing more.
//!
//! [`element_scene::Scene`]: crate::element_scene::Scene

use std::fmt;
use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::element_scene::Scene;
use crate::scene::render_0rgb;

#[derive(Debug)]
pub enum ElementPreviewError {
    EventLoop(winit::error::EventLoopError),
}

impl fmt::Display for ElementPreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ElementPreviewError::EventLoop(err) => {
                write!(f, "element preview event loop failed: {err}")
            }
        }
    }
}

impl std::error::Error for ElementPreviewError {}

/// Opens a window showing `scene` until closed. Blocks the calling thread;
/// only meaningful on a real desktop session.
pub fn run(scene: Scene) -> Result<(), ElementPreviewError> {
    let event_loop = EventLoop::new().map_err(ElementPreviewError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App {
        scene,
        window: None,
        surface: None,
        _context: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(ElementPreviewError::EventLoop)
}

struct App {
    scene: Scene,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
}

impl App {
    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };

        let element_color = self.scene.element.unwrap_or(self.scene.canvas);
        let pixels = render_0rgb(size.width, size.height, self.scene.canvas, element_color);
        buffer.copy_from_slice(&pixels);
        let _ = buffer.present();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Florui element preview");
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Rc::new(window),
            Err(_) => {
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(_) => {
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(_) => {
                event_loop.exit();
                return;
            }
        };

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
            WindowEvent::Resized(_) | WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

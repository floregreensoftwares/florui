//! Runs a real, minimal probe of `florui_platform::appearance`'s own
//! capability contract against a real window: requests custom
//! decorations and a transparent surface, prints the resulting
//! `AppearanceReport` once the window exists, then paints a real opaque
//! red frame through `softbuffer` (this crate's own current desktop
//! presentation backend) every redraw.
//!
//! `cargo run --example appearance_probe -p florui-platform`
//!
//! Run this for real, don't just read the printed report: an unpainted
//! window's own default fill would prove nothing about whether
//! transparency actually works, only whether a real frame does — and a
//! real frame is exactly what this paints. Expect a plain opaque red
//! rectangle with no desktop content showing through: see
//! `florui_platform::appearance`'s own module doc for why that's the
//! verified, expected result, not a bug in this example.

use std::num::NonZeroU32;
use std::rc::Rc;

use florui_platform::appearance::{
    AppearanceRequest, DecorationMode, probe_appearance, probe_window_attributes,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    request: AppearanceRequest,
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
        // Solid red — not a dimmed or "50% alpha" value, because there
        // is nothing to dim it *with*: `softbuffer`'s own documented
        // pixel format has no alpha channel at all (see
        // `florui_platform::appearance`'s own module doc), so there is
        // no value this call could fill the buffer with that would make
        // the desktop show through even partially. This window should
        // paint as flat, fully opaque red, full stop.
        let opaque_red = u32::from_be_bytes([0, 0xff, 0, 0]);
        buffer.fill(opaque_red);
        let _ = buffer.present();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = probe_window_attributes(self.request).with_title(
            "florui-platform appearance probe — look at this window, don't just read stdout",
        );
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("window creation should not fail on a real desktop session"),
        );
        let report = probe_appearance(&window, self.request);
        println!("requested vs. effective, per capability:\n{report:#?}");

        let context = softbuffer::Context::new(window.clone())
            .expect("softbuffer context should not fail on a real desktop session");
        let surface = softbuffer::Surface::new(&context, window.clone())
            .expect("softbuffer surface should not fail on a real desktop session");

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

fn main() {
    let event_loop = EventLoop::new().expect("event loop should build on a real desktop session");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        request: AppearanceRequest {
            decorations: DecorationMode::Custom,
            transparent: true,
        },
        window: None,
        surface: None,
        _context: None,
    };
    event_loop
        .run_app(&mut app)
        .expect("event loop should not fail on a real desktop session");
}

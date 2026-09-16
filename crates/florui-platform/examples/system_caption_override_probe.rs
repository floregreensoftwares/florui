//! A different capability entirely from `appearance_probe`'s own
//! `DecorationMode::Custom` (replacing the OS's own title bar outright
//! with application-drawn content): can this crate *override the
//! appearance of the real system decorations* instead — recolor the
//! title bar's own background and text/button-glyph color while the OS
//! still draws and owns them? Runs the real, production
//! `florui_platform::caption::override_caption_colors` live (see its own
//! doc for the exact `DwmSetWindowAttribute` recipe) rather than
//! duplicating that logic here, and reports what it actually returned.
//!
//! `cargo run --example system_caption_override_probe -p florui-platform`
//!
//! Look at the real title bar, don't just read stdout: a `true` result
//! means both `DwmSetWindowAttribute` calls returned `S_OK`, which this
//! probe still can't fully equate with "looks right" without a human
//! actually checking the color landed on the real title bar and its
//! buttons.

use florui_platform::caption::override_caption_colors;
use florui_style::Rgba;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window =
            event_loop
                .create_window(Window::default_attributes().with_title(
                    "system_caption_override_probe — the title bar itself should turn red",
                ))
                .expect("window creation should not fail on a real desktop session");

        let succeeded = override_caption_colors(
            &window,
            Rgba::opaque(0xd9, 0x2b, 0x2b), // a saturated red, hard to mistake for default
            Rgba::opaque(0xff, 0xff, 0xff),
        );
        if succeeded {
            println!(
                "override_caption_colors returned true — the real title bar should now be red \
                 with white text/buttons, still fully OS-drawn and OS-interactive (drag, snap, \
                 system menu, close/minimize/maximize all still real system chrome, just \
                 recolored)"
            );
        } else {
            println!(
                "override_caption_colors returned false — either this isn't Windows, or it's \
                 not Windows 11 build 22000+ (where DWMWA_CAPTION_COLOR/DWMWA_TEXT_COLOR don't \
                 exist yet). The title bar should look unchanged."
            );
        }

        self.window = Some(window);
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
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop should build on a real desktop session");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { window: None };
    event_loop
        .run_app(&mut app)
        .expect("event loop should not fail on a real desktop session");
}

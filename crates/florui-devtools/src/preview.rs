//! The native preview host: one window, one element, CSS hot reload.
//!
//! This intentionally does not use the eventual layout/paint pipeline — it
//! exists to validate the edit-CSS-see-a-change loop before that pipeline
//! exists. `winit` handles the window and event loop; `softbuffer` blits CPU
//! pixels directly, deferring any GPU backend decision to later.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::color::Rgba;
use crate::diagnostics::{DevEvent, ElementId, log_event, print_banner};
use crate::fixture::{Fixture, load_fixture};
use crate::scene::render_0rgb;

/// Canvas color; visually distinct from the element box so the inset
/// rectangle from [`crate::scene`] is unambiguous.
const CANVAS_COLOR: Rgba = Rgba::opaque(30, 30, 34);
/// Element color shown when the fixture has never loaded successfully.
const FALLBACK_ELEMENT_COLOR: Rgba = Rgba::opaque(180, 40, 40);

#[derive(Debug)]
pub enum DevError {
    EventLoop(winit::error::EventLoopError),
    Watch(notify::Error),
}

impl std::fmt::Display for DevError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DevError::EventLoop(err) => write!(f, "preview event loop failed: {err}"),
            DevError::Watch(err) => write!(f, "could not watch fixture directory: {err}"),
        }
    }
}

impl std::error::Error for DevError {}

enum UserEvent {
    FixtureChanged,
}

/// Runs the preview host until the window is closed. Blocks the calling
/// thread; only meaningful on a real desktop session, not headless CI.
pub fn run(fixture_path: PathBuf) -> Result<(), DevError> {
    print_banner(&fixture_path);

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(DevError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let _watcher = watch_fixture(&fixture_path, proxy).map_err(DevError::Watch)?;

    let mut app = App::new(fixture_path);
    event_loop.run_app(&mut app).map_err(DevError::EventLoop)
}

/// Watches the fixture's parent directory (not the file itself, so editors
/// that save via rename/replace are still observed) and wakes the event
/// loop on any change to the exact fixture path.
fn watch_fixture(
    fixture_path: &Path,
    proxy: EventLoopProxy<UserEvent>,
) -> notify::Result<RecommendedWatcher> {
    let target = fixture_path
        .canonicalize()
        .unwrap_or_else(|_| fixture_path.to_owned());
    let parent = fixture_path
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
            let _ = proxy.send_event(UserEvent::FixtureChanged);
        }
    })?;
    watcher.watch(parent, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

struct App {
    fixture_path: PathBuf,
    epoch: Instant,
    element: ElementId,
    revision: u64,
    current: Fixture,
    stale: bool,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
}

impl App {
    fn new(fixture_path: PathBuf) -> Self {
        Self {
            fixture_path,
            epoch: Instant::now(),
            element: ElementId::next(),
            revision: 0,
            current: Fixture {
                background: FALLBACK_ELEMENT_COLOR,
            },
            stale: true,
            window: None,
            surface: None,
            _context: None,
        }
    }

    fn reload(&mut self) {
        match load_fixture(&self.fixture_path) {
            Ok(fixture) => {
                self.revision += 1;
                self.current = fixture;
                self.stale = false;
                log_event(
                    self.epoch,
                    &DevEvent::FixtureLoaded {
                        path: &self.fixture_path,
                        revision: self.revision,
                    },
                );
            }
            Err(error) => {
                self.stale = true;
                log_event(
                    self.epoch,
                    &DevEvent::FixtureReloadFailed {
                        path: &self.fixture_path,
                        error: &error,
                    },
                );
            }
        }
        self.set_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn set_title(&self) {
        if let Some(window) = &self.window {
            let status = if self.stale {
                "STALE — reload failed, showing last good revision"
            } else {
                "ok"
            };
            window.set_title(&format!(
                "Florui preview — element {} — revision {} — {status}",
                self.element, self.revision
            ));
        }
    }

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

        let start = Instant::now();
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let pixels = render_0rgb(
            size.width,
            size.height,
            CANVAS_COLOR,
            self.current.background,
        );
        buffer.copy_from_slice(&pixels);
        let _ = buffer.present();

        log_event(
            self.epoch,
            &DevEvent::FrameRendered {
                element: self.element,
                revision: self.revision,
                render_time: start.elapsed(),
            },
        );
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Florui preview");
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
        self.reload();
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
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::FixtureChanged => self.reload(),
        }
    }
}

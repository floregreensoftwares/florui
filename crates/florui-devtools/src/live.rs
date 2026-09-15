//! A real [`UiRuntime`]-backed preview window paired with the inspector —
//! the counterpart to [`crate::preview`]'s bootstrap, CSS-only fixture
//! loop, for an app that already has a real component tree the way
//! [`florui_platform::run`] does.
//!
//! `wgpu`/`egui` stay devtools-only dependencies, so this re-implements
//! `florui_platform::desktop`'s private window/surface/event-loop shape
//! rather than exporting it from there.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;

use florui::Element;
use florui_layout::{BoxLayout, SizeCause, absolute_position, compute_size_causes};
use florui_platform::UiRuntime;
use florui_style::{Arena, ComputedStyle, Display, Edges, NodeId, Rgba, StyleError};
use taffy::prelude::*;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::inspector::{ContentBox, Inspector, InspectorAction, InspectorModel, InspectorNode};
use crate::scene::{ElementBox, outline_rect};

const SELECTION_HIGHLIGHT: crate::color::Rgba = crate::color::Rgba::opaque(250, 204, 21);
/// Outline around whatever's under the cursor while "Pick element" is
/// armed, distinct from [`SELECTION_HIGHLIGHT`].
const PICK_HOVER_HIGHLIGHT: crate::color::Rgba = crate::color::Rgba::opaque(56, 189, 248);

#[derive(Debug)]
pub enum LiveError {
    EventLoop(winit::error::EventLoopError),
    Stylesheet(StyleError),
    WindowCreation(winit::error::OsError),
    SurfaceCreation(softbuffer::SoftBufferError),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveError::EventLoop(err) => write!(f, "event loop failed: {err}"),
            LiveError::Stylesheet(err) => write!(f, "stylesheet failed to parse: {err}"),
            LiveError::WindowCreation(err) => write!(f, "window could not be created: {err}"),
            LiveError::SurfaceCreation(err) => {
                write!(f, "render surface could not be created: {err}")
            }
        }
    }
}

impl std::error::Error for LiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LiveError::EventLoop(err) => Some(err),
            LiveError::Stylesheet(err) => Some(err),
            LiveError::WindowCreation(err) => Some(err),
            LiveError::SurfaceCreation(err) => Some(err),
        }
    }
}

enum UserEvent {
    Dirty,
}

/// Opens a preview window titled `title` over `root`, paired with a second
/// inspector window sharing the same event loop and [`UiRuntime`]. `css` is
/// parsed once; it does not get watched for changes.
///
/// Blocks the calling thread until the preview window closes.
pub fn run(
    title: &str,
    css: &str,
    canvas_color: Rgba,
    root: impl Fn() -> Element + 'static,
) -> Result<(), LiveError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(LiveError::EventLoop)?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut host = LiveHost::new(
        title.to_string(),
        canvas_color,
        css.to_string(),
        Box::new(root),
        event_loop.create_proxy(),
    );
    event_loop
        .run_app(&mut host)
        .map_err(LiveError::EventLoop)?;
    match host.fatal_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn display_name(display: Display) -> &'static str {
    match display {
        Display::Block => "block",
        Display::Flex => "flex",
        Display::Inline => "inline",
        Display::InlineBlock => "inline-block",
        Display::Grid => "grid",
    }
}

/// Flattens `arena` pre-order into real [`InspectorNode`]s.
fn build_inspector_model(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    selected: Option<NodeId>,
    picking: bool,
) -> InspectorModel {
    // Diagnostic-only, opted into here rather than in the hot preview
    // paint path — see `compute_size_causes`'s own doc.
    let causes = compute_size_causes(arena, styles, layouts);
    let mut nodes = Vec::new();
    let mut stack: Vec<(NodeId, usize)> =
        arena.roots().iter().rev().map(|&root| (root, 0)).collect();
    while let Some((id, depth)) = stack.pop() {
        push_node(arena, styles, layouts, &causes, id, depth, &mut nodes);
        stack.extend(
            arena
                .children(id)
                .iter()
                .rev()
                .map(|&child| (child, depth + 1)),
        );
    }
    InspectorModel {
        nodes,
        selected,
        picking,
        stale: false,
    }
}

fn format_size_cause(cause: &SizeCause) -> String {
    match cause {
        SizeCause::MinContentClamped { intrinsic_width } => format!(
            "width held to this element's own content — flex-shrink wanted it narrower than \
             its natural {intrinsic_width:.0}px"
        ),
    }
}

fn push_node(
    arena: &Arena,
    styles: &HashMap<NodeId, ComputedStyle>,
    layouts: &HashMap<NodeId, BoxLayout>,
    causes: &HashMap<NodeId, SizeCause>,
    id: NodeId,
    depth: usize,
    out: &mut Vec<InspectorNode>,
) {
    let style = styles
        .get(&id)
        .expect("compute() resolves a ComputedStyle for every arena node");
    let content = layouts.get(&id).map(|layout| {
        let (x, y) = absolute_position(arena, layouts, id);
        ContentBox {
            x,
            y,
            width: layout.width,
            height: layout.height,
        }
    });
    let border = Edges {
        top: style.border.top.width,
        right: style.border.right.width,
        bottom: style.border.bottom.width,
        left: style.border.left.width,
    };

    out.push(InspectorNode {
        id,
        depth,
        tag: arena.tag(id).to_string(),
        display: display_name(style.display).to_string(),
        background: style.background_color,
        content,
        padding: style.padding,
        border,
        margin: style.margin,
        size_cause: causes.get(&id).map(format_size_cause),
    });
}

/// A node's border box (content expanded by its own padding and border) in
/// the same physical-pixel space the preview paints in.
fn border_box_rect(node: &InspectorNode) -> Option<ElementBox> {
    let content = node.content?;
    let x = (content.x - node.padding.left - node.border.left).max(0.0);
    let y = (content.y - node.padding.top - node.border.top).max(0.0);
    let width = content.width
        + node.padding.left
        + node.padding.right
        + node.border.left
        + node.border.right;
    let height = content.height
        + node.padding.top
        + node.padding.bottom
        + node.border.top
        + node.border.bottom;
    Some(ElementBox {
        x: x.round() as u32,
        y: y.round() as u32,
        width: width.max(0.0).round() as u32,
        height: height.max(0.0).round() as u32,
    })
}

/// Owns both windows, the `softbuffer` surface, and the event loop;
/// delegates every rendering, hit-testing, and dispatch decision to one
/// shared [`UiRuntime`].
struct LiveHost {
    title: String,
    canvas_color: Rgba,
    css: String,
    root: Option<Box<dyn Fn() -> Element>>,
    proxy: EventLoopProxy<UserEvent>,
    runtime: Option<UiRuntime>,
    pressed: Option<NodeId>,
    selected: Option<NodeId>,
    picking: bool,
    /// Tracked separately from `UiRuntime`'s own `:hover` state, purely to
    /// draw [`PICK_HOVER_HIGHLIGHT`] while picking.
    hovered: Option<NodeId>,
    last_cursor: (f64, f64),
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
    inspector: Option<Inspector>,
    fatal_error: Option<LiveError>,
}

impl LiveHost {
    fn new(
        title: String,
        canvas_color: Rgba,
        css: String,
        root: Box<dyn Fn() -> Element>,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        Self {
            title,
            canvas_color,
            css,
            root: Some(root),
            proxy,
            runtime: None,
            pressed: None,
            selected: None,
            picking: false,
            hovered: None,
            last_cursor: (0.0, 0.0),
            window: None,
            surface: None,
            _context: None,
            inspector: None,
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

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: LiveError) {
        eprintln!("florui-devtools: {error}");
        self.fatal_error = Some(error);
        event_loop.exit();
    }

    fn inspector_model(&self) -> Option<InspectorModel> {
        let runtime = self.runtime.as_ref()?;
        let (arena, styles, layouts) = runtime.geometry();
        Some(build_inspector_model(
            arena,
            styles,
            layouts,
            self.selected,
            self.picking,
        ))
    }

    fn redraw(&mut self) {
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
        let canvas = florui_paint::paint_to_buffer(
            font,
            size.width,
            size.height,
            self.canvas_color,
            arena,
            styles,
            layouts,
        );
        let mut pixels: Vec<u32> = canvas
            .pixels()
            .iter()
            .map(|p| u32::from_be_bytes([0, p.red(), p.green(), p.blue()]))
            .collect();

        if let Some(model) = self.inspector_model() {
            if let Some(selected) = self.selected
                && let Some(node) = model.nodes.iter().find(|node| node.id == selected)
                && let Some(rect) = border_box_rect(node)
            {
                outline_rect(
                    &mut pixels,
                    size.width,
                    size.height,
                    rect,
                    SELECTION_HIGHLIGHT,
                    2,
                );
            }
            if self.picking
                && let Some(hovered) = self.hovered
                && let Some(node) = model.nodes.iter().find(|node| node.id == hovered)
                && let Some(rect) = border_box_rect(node)
            {
                outline_rect(
                    &mut pixels,
                    size.width,
                    size.height,
                    rect,
                    PICK_HOVER_HIGHLIGHT,
                    2,
                );
            }
        }

        let Some(surface) = &mut self.surface else {
            return;
        };
        if let Err(error) = surface.resize(width, height) {
            eprintln!("florui-devtools: could not resize the render surface: {error}");
            return;
        }
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("florui-devtools: render surface buffer unavailable: {error}");
                return;
            }
        };
        buffer.copy_from_slice(&pixels);
        if let Err(error) = buffer.present() {
            eprintln!("florui-devtools: could not present the frame: {error}");
        }
    }

    fn redraw_inspector(&mut self) {
        let Some(model) = self.inspector_model() else {
            return;
        };
        let Some(inspector) = &mut self.inspector else {
            return;
        };
        match inspector.redraw(&model) {
            Some(InspectorAction::SelectNode(id)) => {
                self.selected = Some(id);
                self.request_redraws();
            }
            Some(InspectorAction::TogglePicking) => {
                self.picking = !self.picking;
                self.request_redraws();
            }
            None => {}
        }
    }

    fn request_redraws(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(inspector) = &self.inspector {
            inspector.request_redraw();
        }
    }

    fn update_and_request_redraw(&mut self) {
        let viewport = self.viewport_size();
        if let Some(runtime) = &mut self.runtime {
            runtime.clear_dirty();
            runtime.update(viewport);
        }
        self.request_redraws();
    }

    fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        self.last_cursor = (x, y);
        let hit = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.hit_test(x as f32, y as f32));
        if self.picking && self.hovered != hit {
            self.hovered = hit;
            self.request_redraws();
        }
        self.set_hovered_and_redraw(hit);
    }

    fn handle_cursor_left(&mut self) {
        self.pressed = None;
        if self.picking && self.hovered.take().is_some() {
            self.request_redraws();
        }
        self.set_hovered_and_redraw(None);
    }

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

    fn handle_press(&mut self) {
        let (x, y) = self.last_cursor;
        self.pressed = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.hit_test(x as f32, y as f32));
    }

    /// While picking, a release selects whatever it landed on (or
    /// deselects, over empty canvas) and disarms picking, without
    /// dispatching a `click` — the same one-shot pick a browser's own
    /// devtools uses. Otherwise, activation stays strict (press and
    /// release on the same node) and selection is left alone.
    fn handle_release(&mut self) {
        let (x, y) = self.last_cursor;
        let pressed = self.pressed.take();
        let Some(runtime) = &self.runtime else {
            return;
        };
        let released_over = runtime.hit_test(x as f32, y as f32);
        if self.picking {
            self.selected = released_over;
            self.picking = false;
        } else if let (Some(pressed), Some(released_over)) = (pressed, released_over)
            && pressed == released_over
        {
            runtime.dispatch_click(pressed);
        }
        self.request_redraws();
    }
}

impl ApplicationHandler<UserEvent> for LiveHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(self.title.clone());
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Rc::new(window),
            Err(error) => return self.fail(event_loop, LiveError::WindowCreation(error)),
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => return self.fail(event_loop, LiveError::SurfaceCreation(error)),
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => return self.fail(event_loop, LiveError::SurfaceCreation(error)),
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
        let mut runtime = match UiRuntime::new(&self.css, root, viewport) {
            Ok(runtime) => runtime,
            Err(error) => return self.fail(event_loop, LiveError::Stylesheet(error)),
        };
        let proxy = self.proxy.clone();
        runtime.on_needs_update(move || {
            let _ = proxy.send_event(UserEvent::Dirty);
        });
        // A mount effect that itself calls `Signal::set` marks the flag
        // before the listener above exists to see it.
        if runtime.is_dirty() {
            runtime.clear_dirty();
            runtime.update(viewport);
        }

        self.runtime = Some(runtime);
        self.window = Some(window);
        self._context = Some(context);
        self.surface = Some(surface);

        match Inspector::new(event_loop) {
            Ok(inspector) => self.inspector = Some(inspector),
            Err(err) => {
                // Losing the inspector should not take down the preview.
                eprintln!("florui-devtools: {err}");
            }
        }

        self.redraw();
        self.redraw_inspector();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.inspector.as_ref().map(Inspector::window_id) == Some(window_id) {
            if let Some(inspector) = &mut self.inspector {
                let _ = inspector.handle_window_event(&event);
            }
            match event {
                WindowEvent::CloseRequested => self.inspector = None,
                WindowEvent::RedrawRequested => self.redraw_inspector(),
                _ => {}
            }
            return;
        }

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

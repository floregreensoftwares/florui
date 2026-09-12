//! A second native window: a minimal tree/style/box inspector for the
//! preview's single selectable element.
//!
//! Uses `egui` (via `egui-winit` + `egui-wgpu`) sharing the preview's own
//! `winit` event loop — not `eframe`, which would want to own the loop
//! itself. `wgpu`/`egui*` are devtools-only dependencies; the preview window
//! itself stays on `softbuffer` and never touches a GPU.

use std::fmt;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use egui::ViewportId;
use egui_wgpu::WgpuError;
use egui_wgpu::winit::Painter;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::color::Rgba;
use crate::diagnostics::ElementId;
use crate::fixture::SourceLocation;
use crate::scene::ElementBox;

/// Everything the inspector needs to render one frame. Rebuilt by the
/// caller whenever the fixture reloads or the selection changes; the
/// inspector itself holds no opinion about *when* it goes stale.
pub struct InspectorModel {
    pub element: ElementId,
    pub selected: bool,
    pub stale: bool,
    pub background: Rgba,
    pub background_hex: String,
    pub source_path: PathBuf,
    pub source_location: SourceLocation,
    /// `None` when the canvas is too small to fit the configured insets —
    /// shown honestly rather than as a fabricated zero-sized box.
    pub element_box: Option<ElementBox>,
}

#[derive(Debug)]
pub enum InspectorError {
    CreateWindow(winit::error::OsError),
    CreateSurface(WgpuError),
}

impl fmt::Display for InspectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InspectorError::CreateWindow(err) => {
                write!(f, "could not create inspector window: {err}")
            }
            InspectorError::CreateSurface(err) => {
                write!(f, "could not initialize inspector rendering: {err}")
            }
        }
    }
}

impl std::error::Error for InspectorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InspectorError::CreateWindow(err) => Some(err),
            InspectorError::CreateSurface(err) => Some(err),
        }
    }
}

pub struct Inspector {
    window: Arc<Window>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    painter: Painter,
}

impl Inspector {
    /// Creates the inspector's own native window on `event_loop` — the same
    /// event loop the preview window uses, since a process may only drive
    /// one `winit` event loop.
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, InspectorError> {
        let attrs = Window::default_attributes().with_title("Florui inspector");
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(InspectorError::CreateWindow)?,
        );

        let egui_ctx = egui::Context::default();

        let mut painter = pollster::block_on(Painter::new(
            egui_ctx.clone(),
            egui_wgpu::WgpuConfiguration::default(),
            false,
            egui_wgpu::RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .map_err(InspectorError::CreateSurface)?;

        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            painter.max_texture_side(),
        );

        Ok(Self {
            window,
            egui_ctx,
            egui_state,
            painter,
        })
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Forwards a window event to egui. Returns whether egui consumed it
    /// (the caller should not also act on a consumed event).
    pub fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::Resized(size) = event {
            self.resize(*size);
        }
        self.egui_state
            .on_window_event(&self.window, event)
            .consumed
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        {
            self.painter
                .on_window_resized(ViewportId::ROOT, width, height);
        }
    }

    pub fn redraw(&mut self, model: &InspectorModel) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let full_output = self.egui_ctx.run_ui(raw_input, |ui| draw_ui(ui, model));
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let clipped_primitives = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            full_output.pixels_per_point,
            [0.08, 0.08, 0.09, 1.0],
            &clipped_primitives,
            &full_output.textures_delta,
            Vec::new(),
            &self.window,
        );
    }
}

fn draw_ui(ui: &mut egui::Ui, model: &InspectorModel) {
    egui::Panel::left("florui-inspector-tree").show(ui, |ui| {
        ui.heading("Tree");
        let _ = ui.selectable_label(model.selected, format!("body {}", model.element));
    });

    egui::CentralPanel::default().show(ui, |ui| {
        if model.stale {
            ui.colored_label(egui::Color32::RED, "STALE — showing last good revision");
            ui.separator();
        }

        ui.heading("Styles");
        ui.monospace(format!("background-color: {}", model.background_hex));
        ui.label(format!(
            "{}:{}",
            model.source_path.display(),
            model.source_location
        ));

        ui.separator();
        ui.heading("Box model");
        match model.element_box {
            Some(b) => {
                ui.monospace(format!(
                    "content: {}, {} — {}x{} (physical px)",
                    b.x, b.y, b.width, b.height
                ));
            }
            None => {
                ui.label("no box: canvas is too small for the configured insets");
            }
        }
        // Not fabricated as zero: this bootstrap engine has no box model
        // beyond the content rectangle above.
        ui.label("padding: not implemented yet");
        ui.label("border: not implemented yet");
        ui.label("margin: not implemented yet");
    });
}

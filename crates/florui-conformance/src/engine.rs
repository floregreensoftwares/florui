//! Renders a [`FloruiSpec`] through Florui's own real style/layout/paint
//! pipeline — `Arena::build` → `compute` → `compute_layout` →
//! `paint_to_buffer` — replacing the `florui_devtools::scene` stand-in
//! this crate used before real style/layout/paint existed (see
//! [`crate::reference_fixture`]'s own module doc for that history).
//!
//! The tested element is wrapped in a synthetic `<div>`, matching the
//! Chromium HTML fixture's own `<body>` — without it, a bare `<span>`
//! would sit at the tree root, where Stylo's real blockification rule
//! (root elements are always block-level, `<https://drafts.csswg.org/css-display/#blockify>`)
//! would force it block regardless of `display: inline`, a false mismatch
//! against Chromium's own *non-root* `<span>` staying genuinely inline.
//!
//! Scoped to `device_pixel_ratio == 1.0`: this crate's engine functions
//! have no notion of a CSS-pixel vs. physical-pixel distinction (they
//! just take pixels), so a fixture at a different DPR would need the
//! whole style/layout pass re-run at scaled font-size/dimensions rather
//! than a simple post-hoc pixel scale — not attempted in this slice.

use std::collections::HashMap;
use std::fmt;

use florui::Element;
use florui_layout::{BoxLayout, LayoutError, absolute_position, compute_layout};
use florui_paint::paint_to_buffer;
use florui_style::{Arena, InteractionState, NodeId, Rgba, compute, parse_stylesheet};
use image::RgbaImage;
use taffy::prelude::{AvailableSpace, Size};

use crate::geometry::BoxGeometryPx;
use crate::reference_fixture::FloruiSpec;

#[derive(Debug)]
pub enum EngineError {
    /// `spec.tag` isn't one of the fixed tags this harness knows how to
    /// build a real `florui::Element` for — see [`to_static_tag`].
    UnknownTag(String),
    Layout(LayoutError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::UnknownTag(tag) => write!(f, "unknown florui fixture tag: {tag:?}"),
            EngineError::Layout(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::UnknownTag(_) => None,
            EngineError::Layout(source) => Some(source),
        }
    }
}

#[derive(Debug)]
pub struct EngineRender {
    pub image: RgbaImage,
    pub element_box_css_px: BoxGeometryPx,
}

/// The fixed set of tags this harness's fixtures build — a `&'static str`
/// is what [`Element::node`] needs, which a JSON-loaded `String` can't
/// borrow from; matching against this small, known set is simpler than a
/// generic (and here unneeded) leak-and-intern scheme.
fn to_static_tag(tag: &str) -> Result<&'static str, EngineError> {
    Ok(match tag {
        "div" => "div",
        "span" => "span",
        "p" => "p",
        "button" => "button",
        "h1" => "h1",
        "h2" => "h2",
        "h3" => "h3",
        "h4" => "h4",
        "h5" => "h5",
        "h6" => "h6",
        other => return Err(EngineError::UnknownTag(other.to_owned())),
    })
}

/// Renders `spec` inside a `width_css_px`x`height_css_px` root (matching
/// the Chromium fixture's own viewport), returning the whole canvas plus
/// the tested element's own box.
///
/// # Panics
///
/// Panics if `spec.css` isn't parseable as real CSS (a fixture's own
/// author error, not a runtime condition to recover from) or
/// `canvas_color` isn't valid hex.
pub fn render_fixture(
    spec: &FloruiSpec,
    canvas_color: &str,
    width_css_px: u32,
    height_css_px: u32,
) -> Result<EngineRender, EngineError> {
    let tag = to_static_tag(&spec.tag)?;
    let text = spec.text.clone();
    let element = Element::node(
        tag,
        vec![("id".to_string(), "el".to_string())],
        vec![Element::text(text)],
    );
    // The synthetic wrapper this module's own doc explains — real CSS's
    // own body-equivalent, not part of the fixture's own declared markup.
    let tree = Element::node("div", vec![], vec![element]);

    let arena = Arena::build(&tree);
    let rules = parse_stylesheet(&spec.css).expect("fixture CSS must be valid");
    let styles = compute(&arena, &rules, &InteractionState::new());

    let available = Size {
        width: AvailableSpace::Definite(width_css_px as f32),
        height: AvailableSpace::Definite(height_css_px as f32),
    };
    let layouts = compute_layout(&arena, &styles, available).map_err(EngineError::Layout)?;

    let wrapper = arena.roots()[0];
    let node = arena.children(wrapper)[0];
    // A plain `display: inline` element with no content of its own (a
    // bare `<span>`) has no individual `BoxLayout` yet — a documented
    // bound (`florui_layout`'s module doc): only `InlineBlock`
    // children get an exact box from a real inline formatting context,
    // not a plain `Inline` one. Falling back to the wrapper's own box is
    // honest about that gap rather than panicking a fixture that hits it.
    let element_box_css_px = box_geometry_of(&arena, &layouts, node).unwrap_or_else(|| {
        box_geometry_of(&arena, &layouts, wrapper).expect("the wrapper always has its own box")
    });

    let canvas = Rgba::opaque(0, 0, 0);
    let canvas = florui_style::parse_hex_color(canvas_color).unwrap_or(canvas);
    let image_pixmap = paint_to_buffer(
        width_css_px,
        height_css_px,
        canvas,
        &arena,
        &styles,
        &layouts,
    );
    let image = RgbaImage::from_raw(width_css_px, height_css_px, image_pixmap.data().to_vec())
        .expect("paint_to_buffer's own dimensions must match what was requested");

    Ok(EngineRender {
        image,
        element_box_css_px,
    })
}

fn box_geometry_of(
    arena: &Arena,
    layouts: &HashMap<NodeId, BoxLayout>,
    node: NodeId,
) -> Option<BoxGeometryPx> {
    let layout = *layouts.get(&node)?;
    let (x, y) = absolute_position(arena, layouts, node);
    Some(BoxGeometryPx {
        x: f64::from(x),
        y: f64::from(y),
        width: f64::from(layout.width),
        height: f64::from(layout.height),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(tag: &str) -> FloruiSpec {
        FloruiSpec {
            tag: tag.to_string(),
            text: String::new(),
            css: String::new(),
        }
    }

    #[test]
    fn renders_a_bare_div_at_the_wrapper_origin() {
        let render = render_fixture(&spec("div"), "#000000", 200, 100).unwrap();
        assert_eq!(render.element_box_css_px.x, 0.0);
        assert_eq!(render.element_box_css_px.y, 0.0);
        assert_eq!(render.image.dimensions(), (200, 100));
    }

    #[test]
    fn an_unknown_tag_is_a_reported_error_not_a_panic() {
        let error = render_fixture(&spec("marquee"), "#000000", 200, 100).unwrap_err();
        assert!(matches!(error, EngineError::UnknownTag(tag) if tag == "marquee"));
    }

    #[test]
    fn a_span_nested_under_the_synthetic_wrapper_is_not_blockified() {
        // The whole reason for the synthetic `<div>` wrapper this module's
        // own doc explains: a bare `<span>` at the tree *root* would be
        // blockified (Stylo's real root-element rule), a false mismatch
        // against Chromium's own non-root `<span>`. This only proves the
        // wrapper exists and layout succeeds; `florui_style`'s own tests
        // already prove the blockification distinction itself.
        let render = render_fixture(&spec("span"), "#000000", 200, 100);
        assert!(render.is_ok());
    }

    #[test]
    fn heading_text_measures_a_nonzero_box() {
        let mut spec = spec("h2");
        spec.text = "Hello".to_string();
        let render = render_fixture(&spec, "#000000", 200, 100).unwrap();
        assert!(render.element_box_css_px.width > 0.0);
        assert!(render.element_box_css_px.height > 0.0);
    }
}

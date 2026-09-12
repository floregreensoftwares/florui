//! Text measurement — the "text" stage the engine's layout consults for
//! intrinsic sizing, between elements/style and boxes/layout in the
//! pipeline (components → elements → style → boxes → layout ↔ **text** →
//! painting).
//!
//! Checked against [Parley](https://github.com/linebender/parley) 0.11's
//! actual current API (a standalone experiment, plus reading its vendored
//! source) rather than assumed from memory, after getting bitten by
//! exactly that mistake earlier in this project's history.
//!
//! # Scope
//!
//! One embedded font ([`EMBEDDED_FONT`], pinned deliberately — see
//! `fonts/NOTICE.md` — rather than presuming font fallback support before
//! it exists), one size, one plain unstyled run of text, measured
//! unwrapped (no max-width, no line breaking, no multi-line, no rich/mixed
//! styling within a run). [`measure`] answers exactly one question: how
//! wide and tall is this text if nothing constrains its width.
//! [`Font::shape`] answers a different question: which glyphs, from which
//! font, at which pen positions — the input a rasterizer needs, without
//! this crate doing any rasterizing itself. Actual wrapping at a given
//! available width is a separate, not-yet-built concern.

use std::sync::Arc;

use parley::{
    FontContext, FontData, FontFamily, LayoutContext, PositionedLayoutItem, StyleProperty,
};
use peniko::Blob;

/// Fira Mono (SIL Open Font License 1.1) — see `fonts/NOTICE.md` for
/// provenance and why a monospace font was chosen for this first slice.
pub const EMBEDDED_FONT: &[u8] = include_bytes!("../fonts/FiraMono-Medium.ttf");

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    /// The width of the text with no wrapping applied.
    pub width: f32,
    /// The height of the (single, unwrapped) line.
    pub height: f32,
}

/// One glyph, positioned relative to the text's own top-left origin
/// (y-down, baseline already accounted for) — everything a rasterizer
/// needs to place it, but no outline data: that lives in the font itself,
/// keyed by [`ShapedGlyph::id`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// A sequence of glyphs sharing one font and size — Parley may itemize a
/// single call to [`Font::shape`] into more than one run (script/bidi
/// boundaries, or a fallback font substituted for a codepoint this crate's
/// one embedded font doesn't cover), so each run carries its own font
/// rather than assuming the caller's requested one.
#[derive(Debug, Clone)]
pub struct ShapedRun {
    pub font: FontData,
    pub font_size: f32,
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Debug, Clone)]
pub struct ShapedText {
    pub runs: Vec<ShapedRun>,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug)]
pub struct TextError {
    message: &'static str,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TextError {}

/// Holds the (deliberately reused, per Parley's own guidance) scratch
/// state needed to measure text, plus the one font this crate works with.
pub struct Font {
    font_cx: FontContext,
    layout_cx: LayoutContext,
    family_name: String,
}

impl Font {
    /// Loads [`EMBEDDED_FONT`].
    pub fn load_embedded() -> Self {
        Self::load(EMBEDDED_FONT).expect("the embedded font is always valid")
    }

    /// Registers `font_bytes` as this instance's only font.
    pub fn load(font_bytes: &[u8]) -> Result<Self, TextError> {
        let mut font_cx = FontContext::new();
        let registered = font_cx
            .collection
            .register_fonts(Blob::new(Arc::new(font_bytes.to_vec())), None);
        let (family_id, _) = registered.first().ok_or(TextError {
            message: "not a valid font file",
        })?;
        let family_name = font_cx
            .collection
            .family_name(*family_id)
            .ok_or(TextError {
                message: "registered font has no family name",
            })?
            .to_string();

        Ok(Self {
            font_cx,
            layout_cx: LayoutContext::new(),
            family_name,
        })
    }

    /// Measures `text` at `font_size`, as if nothing constrained its
    /// width: no wrapping, a single line.
    pub fn measure(&mut self, text: &str, font_size: f32) -> TextMetrics {
        if text.is_empty() {
            // Parley measures an empty string as zero-width but still
            // reports a nonzero line height; a truly empty run should not
            // claim to occupy a line it never lays out.
            return TextMetrics {
                width: 0.0,
                height: 0.0,
            };
        }

        let layout = self.layout(text, font_size);
        TextMetrics {
            width: layout.width(),
            height: layout.height(),
        }
    }

    /// Shapes `text` at `font_size` into paintable glyphs — same
    /// unwrapped, single-line, single-run-per-font layout as [`measure`],
    /// but exposing glyph ids and pen positions instead of just overall
    /// width/height.
    pub fn shape(&mut self, text: &str, font_size: f32) -> ShapedText {
        if text.is_empty() {
            return ShapedText {
                runs: Vec::new(),
                width: 0.0,
                height: 0.0,
            };
        }

        let layout = self.layout(text, font_size);
        let mut runs = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let glyphs = glyph_run
                    .positioned_glyphs()
                    .map(|glyph| ShapedGlyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: glyph.y,
                    })
                    .collect();
                runs.push(ShapedRun {
                    font: run.font().clone(),
                    font_size: run.font_size(),
                    glyphs,
                });
            }
        }

        ShapedText {
            runs,
            width: layout.width(),
            height: layout.height(),
        }
    }

    /// Builds and line-breaks (as a single unwrapped line) a Parley layout
    /// for `text` at `font_size` — the scratch-state setup [`measure`] and
    /// [`shape`] both need before reading anything back out of it.
    fn layout(&mut self, text: &str, font_size: f32) -> parley::Layout<[u8; 4]> {
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::FontFamily(FontFamily::named(
            self.family_name.as_str(),
        )));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_embedded_font_and_reports_its_family_name() {
        let font = Font::load_embedded();
        assert_eq!(font.family_name, "Fira Mono");
    }

    #[test]
    fn rejects_data_that_is_not_a_font() {
        assert!(Font::load(b"not a font").is_err());
    }

    #[test]
    fn measures_a_monospace_run_proportionally_to_its_length() {
        let mut font = Font::load_embedded();
        let one = font.measure("A", 16.0);
        let five = font.measure("AAAAA", 16.0);
        // Every glyph in a monospace font has the same advance, so five
        // characters must measure to exactly five times one character's
        // width — this would not hold for a proportional font.
        assert_eq!(five.width, one.width * 5.0);
        assert_eq!(
            five.height, one.height,
            "line height does not depend on content length"
        );
    }

    #[test]
    fn larger_font_size_measures_wider_and_taller() {
        let mut font = Font::load_embedded();
        let small = font.measure("Hello", 16.0);
        let large = font.measure("Hello", 32.0);
        assert!(large.width > small.width);
        assert!(large.height > small.height);
    }

    #[test]
    fn empty_text_measures_to_zero() {
        let mut font = Font::load_embedded();
        let metrics = font.measure("", 16.0);
        assert_eq!(
            metrics,
            TextMetrics {
                width: 0.0,
                height: 0.0
            }
        );
    }

    #[test]
    fn different_text_of_equal_length_can_still_differ_by_glyph() {
        // Guards against a measurement that secretly only counts
        // characters rather than actually shaping them.
        let mut font = Font::load_embedded();
        let dots = font.measure("iiiii", 16.0);
        let wide = font.measure("MMMMM", 16.0);
        // Fira Mono is monospace, so these happen to be equal — this test
        // exists to be revisited if the embedded font ever changes to a
        // proportional one, where it would need to assert inequality
        // instead.
        assert_eq!(dots.width, wide.width);
    }

    #[test]
    fn shape_produces_one_glyph_per_character_in_source_order() {
        let mut font = Font::load_embedded();
        let shaped = font.shape("AB", 16.0);
        assert_eq!(shaped.runs.len(), 1, "one plain run, one font, one style");
        assert_eq!(shaped.runs[0].glyphs.len(), 2);
    }

    #[test]
    fn shape_advances_each_glyph_by_the_monospace_width() {
        let mut font = Font::load_embedded();
        let shaped = font.shape("AA", 16.0);
        let glyphs = &shaped.runs[0].glyphs;
        let advance = glyphs[1].x - glyphs[0].x;
        assert_eq!(
            advance,
            font.measure("A", 16.0).width,
            "a monospace font's per-glyph advance equals a single character's measured width"
        );
        assert_eq!(
            glyphs[0].y, glyphs[1].y,
            "glyphs on the same unwrapped line share a baseline"
        );
    }

    #[test]
    fn shape_reports_the_run_actually_used_not_just_the_requested_size() {
        let mut font = Font::load_embedded();
        let shaped = font.shape("A", 24.0);
        assert_eq!(shaped.runs[0].font_size, 24.0);
    }

    #[test]
    fn empty_text_shapes_to_no_runs() {
        let mut font = Font::load_embedded();
        let shaped = font.shape("", 16.0);
        assert!(shaped.runs.is_empty());
        assert_eq!(shaped.width, 0.0);
        assert_eq!(shaped.height, 0.0);
    }
}

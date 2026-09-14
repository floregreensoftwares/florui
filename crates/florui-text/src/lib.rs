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
//! it exists), one size, one plain unstyled run of text. [`Font::measure`]/
//! [`Font::shape`] answer their questions (respectively: how wide/tall:,
//! and which glyphs at which pen positions) unwrapped, as a single line;
//! [`Font::measure_wrapped`]/[`Font::shape_wrapped`] answer the same two
//! questions wrapped at a given available width, real multi-line layout.
//! `ShapedGlyph`'s `x`/`y` are already absolute within the whole shaped
//! block regardless of which line a glyph landed on, so [`ShapedText`]
//! needs no separate per-line type to support either case.
//!
//! Not addressed here: min-content sizing (the width of the single
//! longest unbreakable word) — [`Font::measure_wrapped`] wraps at an
//! explicit width or not at all, so a caller with only a min-content
//! constraint (no definite width anywhere) still gets unwrapped
//! measurement, which overstates how narrow the text could actually go.

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
        Self::metrics_of(self.layout_unwrapped(text, font_size))
    }

    /// Measures `text` at `font_size`, wrapping at `max_width` — real
    /// multi-line layout. A single word wider than `max_width` still gets
    /// its own (overflowing) line rather than being broken mid-word or
    /// bleeding onto an adjacent line — Parley's own wrapping behavior,
    /// matching real CSS's default `overflow-wrap: normal`.
    pub fn measure_wrapped(&mut self, text: &str, font_size: f32, max_width: f32) -> TextMetrics {
        Self::metrics_of(self.layout_wrapped(text, font_size, max_width))
    }

    fn metrics_of(layout: Option<parley::Layout<[u8; 4]>>) -> TextMetrics {
        match layout {
            // Parley measures an empty string as zero-width but still
            // reports a nonzero line height; a truly empty run should not
            // claim to occupy a line it never lays out.
            None => TextMetrics {
                width: 0.0,
                height: 0.0,
            },
            Some(layout) => TextMetrics {
                width: layout.width(),
                height: layout.height(),
            },
        }
    }

    /// Shapes `text` at `font_size` into paintable glyphs — same
    /// unwrapped, single-line layout as [`Self::measure`], but exposing
    /// glyph ids and pen positions instead of just overall width/height.
    pub fn shape(&mut self, text: &str, font_size: f32) -> ShapedText {
        Self::shaped_text_of(self.layout_unwrapped(text, font_size))
    }

    /// Shapes `text` at `font_size` into paintable glyphs, wrapped at
    /// `max_width` — same wrapping behavior as [`Self::measure_wrapped`].
    /// Every glyph's `x`/`y` stays absolute within the whole shaped block
    /// (not line-relative), so a caller paints this exactly like an
    /// unwrapped [`Self::shape`] result — later lines simply carry larger
    /// `y` values, with no separate per-line structure to unpack.
    pub fn shape_wrapped(&mut self, text: &str, font_size: f32, max_width: f32) -> ShapedText {
        Self::shaped_text_of(self.layout_wrapped(text, font_size, max_width))
    }

    fn shaped_text_of(layout: Option<parley::Layout<[u8; 4]>>) -> ShapedText {
        let Some(layout) = layout else {
            return ShapedText {
                runs: Vec::new(),
                width: 0.0,
                height: 0.0,
            };
        };

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

    /// [`Self::layout_wrapped`] with no width constraint — a single
    /// unwrapped line.
    fn layout_unwrapped(&mut self, text: &str, font_size: f32) -> Option<parley::Layout<[u8; 4]>> {
        self.build_layout(text, font_size, None)
    }

    /// Builds and line-breaks a Parley layout for `text` at `font_size`,
    /// wrapped at `max_width` — the scratch-state setup every measure/shape
    /// method needs before reading anything back out of it. `None` for
    /// empty `text`, which Parley itself would otherwise still lay out as
    /// one zero-width line with a nonzero line height.
    fn layout_wrapped(
        &mut self,
        text: &str,
        font_size: f32,
        max_width: f32,
    ) -> Option<parley::Layout<[u8; 4]>> {
        self.build_layout(text, font_size, Some(max_width))
    }

    fn build_layout(
        &mut self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
    ) -> Option<parley::Layout<[u8; 4]>> {
        if text.is_empty() {
            return None;
        }
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::FontFamily(FontFamily::named(
            self.family_name.as_str(),
        )));
        let mut layout = builder.build(text);
        layout.break_all_lines(max_width);
        Some(layout)
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

    #[test]
    fn wrapping_at_a_width_that_fits_everything_matches_unwrapped_measurement() {
        let mut font = Font::load_embedded();
        let unwrapped = font.measure("one two three", 16.0);
        let wrapped = font.measure_wrapped("one two three", 16.0, unwrapped.width + 1.0);
        assert_eq!(wrapped.width, unwrapped.width);
        assert_eq!(
            wrapped.height, unwrapped.height,
            "room for the whole line must not wrap it at all"
        );
    }

    #[test]
    fn wrapping_at_a_narrower_width_grows_the_height_and_shrinks_the_width() {
        let mut font = Font::load_embedded();
        let one_word = font.measure("aaaaa", 16.0);
        let unwrapped = font.measure("aaaaa bbbbb ccccc", 16.0);

        // Just wide enough for the widest single word, not the whole line —
        // must wrap onto three lines, one per word.
        let wrapped = font.measure_wrapped("aaaaa bbbbb ccccc", 16.0, one_word.width + 1.0);

        assert!(
            wrapped.width <= one_word.width + 1.0,
            "no wrapped line should exceed the width it wrapped at"
        );
        assert_eq!(
            wrapped.height,
            one_word.height * 3.0,
            "three words, one per line, at three times a single line's height"
        );
        assert!(wrapped.height > unwrapped.height);
    }

    #[test]
    fn wrapping_never_breaks_a_single_word_even_when_narrower_than_the_max_width() {
        // Real CSS's own default (`overflow-wrap: normal`): an unbreakable
        // word wider than the container overflows it rather than being
        // split mid-word.
        let mut font = Font::load_embedded();
        let word = font.measure("supercalifragilisticexpialidocious", 16.0);
        let wrapped =
            font.measure_wrapped("supercalifragilisticexpialidocious", 16.0, word.width / 2.0);
        assert_eq!(
            wrapped.width, word.width,
            "the single unbreakable word must overflow the requested width, not be cut"
        );
        assert_eq!(wrapped.height, word.height, "still exactly one line");
    }

    #[test]
    fn empty_text_wraps_to_zero() {
        let mut font = Font::load_embedded();
        let metrics = font.measure_wrapped("", 16.0, 100.0);
        assert_eq!(
            metrics,
            TextMetrics {
                width: 0.0,
                height: 0.0
            }
        );
    }

    #[test]
    fn shape_wrapped_keeps_every_glyph_and_spreads_them_across_lines() {
        let mut font = Font::load_embedded();
        let one_word = font.measure("aaaaa", 16.0);
        let shaped = font.shape_wrapped("aaaaa bbbbb", 16.0, one_word.width + 1.0);

        let total_glyphs: usize = shaped.runs.iter().map(|run| run.glyphs.len()).sum();
        assert_eq!(
            total_glyphs, 11,
            "wrapping must not drop or duplicate any of the 10 letters or the \
             space between the two words"
        );

        let first_y = shaped.runs[0].glyphs[0].y;
        let last_run = shaped.runs.last().unwrap();
        let last_y = last_run.glyphs.last().unwrap().y;
        assert!(
            last_y > first_y,
            "a glyph on the second wrapped line must sit lower than one on the first"
        );
    }

    #[test]
    fn shape_wrapped_glyphs_on_the_same_line_share_a_y() {
        let mut font = Font::load_embedded();
        let shaped = font.shape_wrapped("aaaaa", 16.0, 1000.0);
        let glyphs = &shaped.runs[0].glyphs;
        assert_eq!(glyphs[0].y, glyphs[4].y, "one line, one shared baseline");
    }
}

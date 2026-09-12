//! The declared value of a property before cascade/inheritance resolves
//! it to a used value, and the closed set of properties this crate
//! understands.

use crate::color::Rgba;

/// What kind of value a property's grammar accepts, so the parser can
/// reject a color where a length belongs and vice versa instead of
/// guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Color,
    /// A length, or the `auto` keyword (in addition to the universal
    /// `inherit`/`initial`).
    LengthOrAuto,
    /// A length only — real CSS padding does not accept `auto`.
    Length,
}

/// The only properties this crate computes. Anything else is a parse
/// error, not a silently ignored declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Property {
    /// Does not inherit; initial value is transparent.
    BackgroundColor,
    /// Inherits; initial value is black.
    Color,
    /// Does not inherit; initial value is `auto`.
    Width,
    /// Does not inherit; initial value is `auto`.
    Height,
    /// Does not inherit; initial value is `0`. `auto` enables the usual
    /// auto-margin centering behavior.
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    /// Does not inherit; initial value is `0`. No `auto` — real CSS
    /// padding does not accept it either.
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
}

impl Property {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "background-color" => Some(Property::BackgroundColor),
            "color" => Some(Property::Color),
            "width" => Some(Property::Width),
            "height" => Some(Property::Height),
            "margin-top" => Some(Property::MarginTop),
            "margin-right" => Some(Property::MarginRight),
            "margin-bottom" => Some(Property::MarginBottom),
            "margin-left" => Some(Property::MarginLeft),
            "padding-top" => Some(Property::PaddingTop),
            "padding-right" => Some(Property::PaddingRight),
            "padding-bottom" => Some(Property::PaddingBottom),
            "padding-left" => Some(Property::PaddingLeft),
            _ => None,
        }
    }

    pub fn value_kind(self) -> ValueKind {
        match self {
            Property::BackgroundColor | Property::Color => ValueKind::Color,
            Property::Width
            | Property::Height
            | Property::MarginTop
            | Property::MarginRight
            | Property::MarginBottom
            | Property::MarginLeft => ValueKind::LengthOrAuto,
            Property::PaddingTop
            | Property::PaddingRight
            | Property::PaddingBottom
            | Property::PaddingLeft => ValueKind::Length,
        }
    }

    pub fn inherits(self) -> bool {
        matches!(self, Property::Color)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Color(Rgba),
    /// A resolved length in CSS pixels.
    Length(f32),
    /// The `auto` keyword — only valid where [`ValueKind::LengthOrAuto`]
    /// allows it.
    Auto,
    /// The `inherit` keyword: forces inheritance even for a
    /// non-inheriting property.
    Inherit,
    /// The `initial` keyword: forces the property's initial value even
    /// where it would otherwise inherit.
    Initial,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_declared_property_name() {
        for (name, property) in [
            ("background-color", Property::BackgroundColor),
            ("color", Property::Color),
            ("width", Property::Width),
            ("height", Property::Height),
            ("margin-top", Property::MarginTop),
            ("padding-left", Property::PaddingLeft),
        ] {
            assert_eq!(Property::parse(name), Some(property));
        }
        assert_eq!(Property::parse("border"), None);
    }

    #[test]
    fn inheritance_matches_real_css() {
        assert!(!Property::BackgroundColor.inherits());
        assert!(Property::Color.inherits());
        assert!(!Property::Width.inherits());
        assert!(!Property::MarginTop.inherits());
        assert!(!Property::PaddingTop.inherits());
    }

    #[test]
    fn padding_does_not_accept_auto_but_margin_does() {
        assert_eq!(Property::PaddingTop.value_kind(), ValueKind::Length);
        assert_eq!(Property::MarginTop.value_kind(), ValueKind::LengthOrAuto);
        assert_eq!(Property::Width.value_kind(), ValueKind::LengthOrAuto);
    }
}

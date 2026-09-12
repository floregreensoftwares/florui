//! The declared value of a property before cascade/inheritance resolves
//! it to a used value, and the closed set of properties this crate
//! understands.

use crate::color::Rgba;

/// The only properties this crate computes. Anything else is a parse
/// error, not a silently ignored declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Property {
    /// Does not inherit; initial value is transparent.
    BackgroundColor,
    /// Inherits; initial value is black.
    Color,
}

impl Property {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "background-color" => Some(Property::BackgroundColor),
            "color" => Some(Property::Color),
            _ => None,
        }
    }

    pub fn inherits(self) -> bool {
        matches!(self, Property::Color)
    }

    pub fn initial(self) -> Rgba {
        match self {
            Property::BackgroundColor => Rgba::TRANSPARENT,
            Property::Color => Rgba::opaque(0, 0, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Color(Rgba),
    /// The `inherit` keyword: forces inheritance even for a
    /// non-inheriting property like `background-color`.
    Inherit,
    /// The `initial` keyword: forces the property's initial value even
    /// where it would otherwise inherit.
    Initial,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_declared_properties_parse() {
        assert_eq!(
            Property::parse("background-color"),
            Some(Property::BackgroundColor)
        );
        assert_eq!(Property::parse("color"), Some(Property::Color));
        assert_eq!(Property::parse("border"), None);
    }

    #[test]
    fn inheritance_matches_real_css_for_these_two_properties() {
        assert!(!Property::BackgroundColor.inherits());
        assert!(Property::Color.inherits());
    }
}

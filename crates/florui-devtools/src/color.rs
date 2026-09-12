//! Bootstrap hex-color parsing for the native preview host.
//!
//! This is not the CSS color grammar (no named colors, `rgb()`, alpha, or
//! CSS's own error-recovery rules). Replace it once real style computation
//! exists.

use std::fmt;

/// An opaque RGBA color in the sRGB color space, 8 bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorParseError {
    input: String,
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported color value {:?} (only #rgb and #rrggbb hex are accepted)",
            self.input
        )
    }
}

impl std::error::Error for ColorParseError {}

/// Parses `#rgb` or `#rrggbb` hex colors. Any other input, including valid
/// CSS colors outside this bootstrap grammar, is an error.
pub fn parse_hex_color(input: &str) -> Result<Rgba, ColorParseError> {
    let trimmed = input.trim();
    let hex = trimmed.strip_prefix('#').ok_or_else(|| ColorParseError {
        input: trimmed.to_owned(),
    })?;

    let expand = |c: u8| c * 16 + c;
    let digit = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };

    let bytes = hex.as_bytes();
    let err = || ColorParseError {
        input: trimmed.to_owned(),
    };

    match bytes.len() {
        3 => {
            let r = digit(bytes[0]).ok_or_else(err)?;
            let g = digit(bytes[1]).ok_or_else(err)?;
            let b = digit(bytes[2]).ok_or_else(err)?;
            Ok(Rgba::opaque(expand(r), expand(g), expand(b)))
        }
        6 => {
            let channel = |i: usize| -> Result<u8, ColorParseError> {
                let hi = digit(bytes[i]).ok_or_else(err)?;
                let lo = digit(bytes[i + 1]).ok_or_else(err)?;
                Ok(hi * 16 + lo)
            };
            Ok(Rgba::opaque(channel(0)?, channel(2)?, channel(4)?))
        }
        _ => Err(err()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex() {
        assert_eq!(
            parse_hex_color("#42734f").unwrap(),
            Rgba::opaque(0x42, 0x73, 0x4f)
        );
    }

    #[test]
    fn parses_three_digit_hex() {
        assert_eq!(
            parse_hex_color("#fff").unwrap(),
            Rgba::opaque(0xff, 0xff, 0xff)
        );
    }

    #[test]
    fn rejects_named_colors() {
        assert!(parse_hex_color("cornflowerblue").is_err());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(parse_hex_color("#1234").is_err());
    }
}

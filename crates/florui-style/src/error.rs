//! Errors from parsing the supported CSS subset. Every unsupported
//! construct (an unknown property, a pseudo-class we don't implement,
//! `!important`, an unclosed block) is a named, reported error — never a
//! silently accepted or silently dropped declaration.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleError {
    UnclosedBlock,
    EmptySelector,
    EmptyCompoundSelector,
    UnsupportedPseudoClass(String),
    UnsupportedProperty(String),
    InvalidValue { property: String, value: String },
    UnsupportedImportant { property: String },
    EmptyDeclaration,
}

impl fmt::Display for StyleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StyleError::UnclosedBlock => write!(f, "unclosed `{{ ... }}` block"),
            StyleError::EmptySelector => write!(f, "empty selector"),
            StyleError::EmptyCompoundSelector => {
                write!(f, "selector has an empty `.`, `#`, or `:` component")
            }
            StyleError::UnsupportedPseudoClass(name) => {
                write!(
                    f,
                    "unsupported pseudo-class `:{name}` (only :hover, :focus, :active are understood)"
                )
            }
            StyleError::UnsupportedProperty(name) => {
                write!(
                    f,
                    "unsupported property `{name}` (only background-color and color are understood)"
                )
            }
            StyleError::InvalidValue { property, value } => {
                write!(f, "invalid value `{value}` for `{property}`")
            }
            StyleError::UnsupportedImportant { property } => {
                write!(f, "`!important` is not supported (on `{property}`)")
            }
            StyleError::EmptyDeclaration => write!(f, "empty declaration (stray `;`?)"),
        }
    }
}

impl std::error::Error for StyleError {}

//! Reads a `florui::Element` tree down to the same two-color scene the
//! preview renderer already knows how to paint: the root's own background
//! and, if it has one, its first child's.
//!
//! There is no CSS cascade or selector matching anywhere yet, so this does
//! not read a class or a stylesheet — only a literal HTML `style` attribute
//! directly on the element, e.g. `style="background-color: #1e1e22;"`, the
//! same way a browser reads an inline style before any author stylesheet.
//! Layout does not exist either: children past the first, and nesting past
//! one level, are read but not shown, matching the single inset rectangle
//! [`crate::scene`] already draws — this bridges what `view!` builds to
//! what the renderer can currently display, not the other way around.

use florui::{Element, ElementNode};

use crate::color::{Rgba, parse_hex_color};

/// The subset of an element tree the current renderer can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scene {
    pub canvas: Rgba,
    pub element: Option<Rgba>,
}

/// Reads `root`'s own background and its first child's, falling back to
/// `fallback` for the canvas when the root declares no background.
pub fn from_element(root: &Element, fallback: Rgba) -> Scene {
    Scene {
        canvas: background_color(root).unwrap_or(fallback),
        element: first_child(root).and_then(background_color),
    }
}

fn first_child(element: &Element) -> Option<&Element> {
    match element {
        Element::Node(ElementNode { children, .. }) => children.first(),
        Element::Fragment(children) => children.first(),
        Element::Text(_) => None,
    }
}

fn background_color(element: &Element) -> Option<Rgba> {
    let Element::Node(node) = element else {
        return None;
    };
    let (_, style) = node.attrs.iter().find(|(name, _)| name == "style")?;
    let value = style_declaration(style, "background-color")?;
    parse_hex_color(value).ok()
}

/// A minimal `property: value;` reader for one inline style attribute —
/// not a CSS parser, just enough to find one declaration by name.
fn style_declaration<'a>(style: &'a str, property: &str) -> Option<&'a str> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        (name.trim() == property).then(|| value.trim())
    })
}

#[cfg(test)]
mod tests {
    use florui::prelude::*;

    use super::*;

    const FALLBACK: Rgba = Rgba::opaque(9, 9, 9);

    #[test]
    fn reads_the_root_background() {
        let tree: Element = view! { <div style="background-color: #1e1e22;" /> };
        let scene = from_element(&tree, FALLBACK);
        assert_eq!(scene.canvas, Rgba::opaque(0x1e, 0x1e, 0x22));
        assert_eq!(scene.element, None);
    }

    #[test]
    fn reads_the_first_childs_background() {
        let tree: Element = view! {
            <div style="background-color: #1e1e22;">
                <div style="background-color: #b42828;" />
            </div>
        };
        let scene = from_element(&tree, FALLBACK);
        assert_eq!(scene.canvas, Rgba::opaque(0x1e, 0x1e, 0x22));
        assert_eq!(scene.element, Some(Rgba::opaque(0xb4, 0x28, 0x28)));
    }

    #[test]
    fn falls_back_when_the_root_has_no_style() {
        let tree: Element = view! { <div /> };
        let scene = from_element(&tree, FALLBACK);
        assert_eq!(scene.canvas, FALLBACK);
        assert_eq!(scene.element, None);
    }

    #[test]
    fn ignores_children_past_the_first() {
        let tree: Element = view! {
            <div style="background-color: #1e1e22;">
                <div style="background-color: #b42828;" />
                <div style="background-color: #2a9d3f;" />
            </div>
        };
        let scene = from_element(&tree, FALLBACK);
        assert_eq!(scene.element, Some(Rgba::opaque(0xb4, 0x28, 0x28)));
    }
}

//! [`use_scroll_offset`]: real per-id scroll state for an `overflow:
//! scroll`/`auto` element — the [`crate::use_committed_size`] counterpart
//! for a scrollable container.
//!
//! The offset itself is a real [`Signal`], not a plain value behind a
//! bespoke notification path: [`florui_reactive::DirtyFlag::mark`] is
//! crate-private to `florui-reactive`, so this crate cannot mark one
//! directly the way a `Signal::set` does internally. Backing the offset
//! with an actual `Signal` means both the real wheel-driven path and a
//! future imperative `scroll_to` go through the exact same write, so a
//! component reading [`ScrollHandle::offset`] is correctly reactive to
//! either, with nothing here to keep in sync by hand.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use florui_layout::{BoxLayout, ContentExtent};
use florui_reactive::{Cleanup, Signal, use_attachment, use_context, use_signal};
use florui_style::{Arena, NodeId};

struct ScrollEntry {
    offset: Signal<(f32, f32)>,
    viewport_size: (f32, f32),
    content_size: (f32, f32),
    last_notified: Option<(f32, f32)>,
    on_scroll: Box<dyn FnMut(f32, f32)>,
}

/// Shared per-[`crate::UiRuntime`] registry of scrollable elements,
/// provided fresh through context every render the same way
/// [`crate::SizeObserverRegistry`] is.
#[derive(Default)]
pub struct ScrollRegistry {
    entries: RefCell<HashMap<String, ScrollEntry>>,
}

impl ScrollRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn set(&self, id: String, offset: Signal<(f32, f32)>, on_scroll: Box<dyn FnMut(f32, f32)>) {
        self.entries.borrow_mut().insert(
            id,
            ScrollEntry {
                offset,
                viewport_size: (0.0, 0.0),
                content_size: (0.0, 0.0),
                last_notified: None,
                on_scroll,
            },
        );
    }

    fn remove(&self, id: &str) {
        self.entries.borrow_mut().remove(id);
    }

    /// Refreshes every registered id's viewport/content size from this
    /// render's own real geometry, and re-clamps its persisted offset —
    /// content that shrank (e.g. a future virtualized list removing items)
    /// can't leave the offset pointing past its new end. Called once per
    /// [`crate::UiRuntime::update`], right after `size_observers.notify` —
    /// see that call's own doc for why after, not before.
    pub(crate) fn sync(
        &self,
        arena: &Arena,
        layouts: &HashMap<NodeId, BoxLayout>,
        content_extents: &HashMap<NodeId, ContentExtent>,
    ) {
        let ids: Vec<String> = self.entries.borrow().keys().cloned().collect();
        for id in ids {
            let Some(node) = arena.find(|a, candidate| a.id_attr(candidate) == Some(id.as_str()))
            else {
                continue;
            };
            let Some(layout) = layouts.get(&node) else {
                continue;
            };
            let viewport_size = (layout.width, layout.height);
            let content_size = content_extents
                .get(&node)
                .map_or(viewport_size, |extent| (extent.width, extent.height));

            // Removed and reinserted around the callback, rather than held
            // borrowed across it, so `on_scroll` touching this same
            // registry can't panic on a re-entrant borrow — the same
            // discipline `SizeObserverRegistry::notify` already follows.
            let Some(mut entry) = self.entries.borrow_mut().remove(&id) else {
                continue;
            };
            entry.viewport_size = viewport_size;
            entry.content_size = content_size;
            let current = entry.offset.get();
            let clamped = clamp_offset(current, viewport_size, content_size);
            if clamped != current {
                entry.offset.set(clamped);
            }
            if entry.last_notified != Some(clamped) {
                entry.last_notified = Some(clamped);
                (entry.on_scroll)(clamped.0, clamped.1);
            }
            self.entries.borrow_mut().insert(id, entry);
        }
    }

    /// Moves `id`'s offset by `(dx, dy)` from its current value, clamped to
    /// its last-known content/viewport size. Returns whether the offset
    /// actually changed — the real wheel handler only needs to repaint
    /// when it did.
    pub(crate) fn scroll_by(&self, id: &str, dx: f32, dy: f32) -> bool {
        let current = self.offset(id);
        self.scroll_to(id, current.0 + dx, current.1 + dy)
    }

    /// Sets `id`'s offset to `(x, y)`, clamped the same way. Returns
    /// whether the offset actually changed.
    pub(crate) fn scroll_to(&self, id: &str, x: f32, y: f32) -> bool {
        let Some((offset, viewport_size, content_size)) =
            self.entries.borrow().get(id).map(|entry| {
                (
                    entry.offset.clone(),
                    entry.viewport_size,
                    entry.content_size,
                )
            })
        else {
            return false;
        };
        let clamped = clamp_offset((x, y), viewport_size, content_size);
        if clamped == offset.get() {
            return false;
        }
        offset.set(clamped);
        true
    }

    fn offset(&self, id: &str) -> (f32, f32) {
        self.entries
            .borrow()
            .get(id)
            .map_or((0.0, 0.0), |entry| entry.offset.get())
    }

    fn viewport_size(&self, id: &str) -> (f32, f32) {
        self.entries
            .borrow()
            .get(id)
            .map_or((0.0, 0.0), |entry| entry.viewport_size)
    }

    fn content_size(&self, id: &str) -> (f32, f32) {
        self.entries
            .borrow()
            .get(id)
            .map_or((0.0, 0.0), |entry| entry.content_size)
    }
}

fn clamp_offset(
    offset: (f32, f32),
    viewport_size: (f32, f32),
    content_size: (f32, f32),
) -> (f32, f32) {
    let max_x = (content_size.0 - viewport_size.0).max(0.0);
    let max_y = (content_size.1 - viewport_size.1).max(0.0);
    (offset.0.clamp(0.0, max_x), offset.1.clamp(0.0, max_y))
}

/// A real, generic scrollable element's live offset and geometry —
/// returned by [`use_scroll_offset`]. Cheap to clone; every clone reads
/// and drives the same underlying registry entry.
#[derive(Clone)]
pub struct ScrollHandle {
    registry: Rc<ScrollRegistry>,
    id: String,
}

impl ScrollHandle {
    /// The current scroll offset. Reactive: reading this inside a
    /// component re-renders it when the offset changes, the same as
    /// reading any other [`Signal`] — this is backed by one.
    pub fn offset(&self) -> (f32, f32) {
        self.registry.offset(&self.id)
    }

    /// The scrollable element's own padding-box size, as of the most
    /// recent render — a plain query against last-known geometry, the
    /// same "answer against last computed frame" contract
    /// [`crate::use_committed_size`] already has; not itself reactive.
    pub fn viewport_size(&self) -> (f32, f32) {
        self.registry.viewport_size(&self.id)
    }

    /// The element's real scrollable content extent, as of the most
    /// recent render — same non-reactive contract as
    /// [`Self::viewport_size`].
    pub fn content_size(&self) -> (f32, f32) {
        self.registry.content_size(&self.id)
    }

    /// Scrolls immediately to `(x, y)`, clamped to the element's
    /// last-known content/viewport size — e.g. for a future virtualized
    /// list's scroll-to-item.
    pub fn scroll_to(&self, x: f32, y: f32) {
        self.registry.scroll_to(&self.id, x, y);
    }

    /// Scrolls by `(dx, dy)` from the current offset, clamped the same
    /// way.
    pub fn scroll_by(&self, dx: f32, dy: f32) {
        self.registry.scroll_by(&self.id, dx, dy);
    }
}

/// Subscribes to real scroll state for the element whose `id` attribute is
/// `id`: `on_scroll(x, y)` runs whenever this element's offset actually
/// changes (a real wheel event, an imperative [`ScrollHandle::scroll_to`],
/// or a clamp forced by shrinking content), including once for whatever
/// offset the very first sync produces. Requires a [`ScrollRegistry`] in
/// context, which [`crate::UiRuntime`] provides every render — calling
/// this outside one is a programming error, not a recoverable condition.
///
/// # Panics
///
/// Panics if no [`ScrollRegistry`] is in context.
pub fn use_scroll_offset(
    id: impl Into<String>,
    on_scroll: impl FnMut(f32, f32) + 'static,
) -> ScrollHandle {
    let id = id.into();
    let registry = use_context::<Rc<ScrollRegistry>>().expect(
        "use_scroll_offset needs a ScrollRegistry in context — only a UiRuntime-hosted render \
         provides one",
    );
    let offset = use_signal(|| (0.0f32, 0.0f32));
    let setup_id = id.clone();
    use_attachment(Rc::clone(&registry), id.clone(), move |registry| {
        registry.set(setup_id.clone(), offset.clone(), Box::new(on_scroll));
        let registry = Rc::clone(registry);
        let cleanup_id = setup_id;
        Some(Box::new(move || registry.remove(&cleanup_id)) as Cleanup)
    });
    ScrollHandle { registry, id }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use florui::prelude::*;
    use taffy::prelude::*;

    use super::*;
    use crate::UiRuntime;

    #[test]
    fn clamp_offset_bounds_to_zero_and_to_the_real_scrollable_range() {
        assert_eq!(
            clamp_offset((-5.0, -5.0), (50.0, 50.0), (200.0, 200.0)),
            (0.0, 0.0)
        );
        assert_eq!(
            clamp_offset((1000.0, 1000.0), (50.0, 50.0), (200.0, 200.0)),
            (150.0, 150.0)
        );
        assert_eq!(
            clamp_offset((10.0, 10.0), (50.0, 50.0), (30.0, 30.0)),
            (0.0, 0.0),
            "content smaller than the viewport has nothing to scroll, regardless of a stale non-zero offset"
        );
    }

    fn viewport() -> Size<AvailableSpace> {
        Size {
            width: AvailableSpace::Definite(200.0),
            height: AvailableSpace::Definite(200.0),
        }
    }

    #[test]
    fn use_scroll_offset_reports_real_geometry_and_clamps_scroll_to_the_real_content_extent() {
        let handle_slot: Rc<RefCell<Option<ScrollHandle>>> = Rc::new(RefCell::new(None));
        let on_scroll_log: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
        let root = {
            let handle_slot = Rc::clone(&handle_slot);
            let on_scroll_log = Rc::clone(&on_scroll_log);
            move || {
                let log = Rc::clone(&on_scroll_log);
                let handle = use_scroll_offset("box", move |x, y| log.borrow_mut().push((x, y)));
                *handle_slot.borrow_mut() = Some(handle);
                view! {
                    <div id="box" class="box">
                        <div class="content" />
                    </div>
                }
            }
        };
        let rules = florui_style::parse_stylesheet(
            ".box { width: 50px; height: 50px; } .content { width: 10px; height: 200px; }",
        )
        .unwrap();
        let mut runtime = UiRuntime::with_rules(rules, root, viewport());

        // `UiRuntime::with_rules` already ran one real `update` during
        // construction — the very first sync, right after that first real
        // layout, already reports real geometry and fires `on_scroll` once
        // for the initial `(0, 0)` offset, the same "including once for
        // whatever ... the very first ... produces" contract
        // `use_committed_size` already has.
        let handle = handle_slot.borrow().clone().unwrap();
        assert_eq!(handle.viewport_size(), (50.0, 50.0));
        assert_eq!(handle.content_size(), (10.0, 200.0));
        assert_eq!(on_scroll_log.borrow().as_slice(), &[(0.0, 0.0)]);

        handle.scroll_to(0.0, 1000.0);
        runtime.update(viewport());
        assert_eq!(
            handle.offset(),
            (0.0, 150.0),
            "scrolling past the real content extent must clamp to the real maximum \
             (content height 200 minus viewport height 50), not the requested value"
        );
        assert_eq!(
            on_scroll_log.borrow().as_slice(),
            &[(0.0, 0.0), (0.0, 150.0)],
            "on_scroll must fire again for the clamped value actually reached"
        );

        handle.scroll_to(0.0, 150.0);
        runtime.update(viewport());
        assert_eq!(
            on_scroll_log.borrow().len(),
            2,
            "on_scroll must not fire again when the offset does not actually change"
        );
    }
}

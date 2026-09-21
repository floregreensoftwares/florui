//! [`use_virtual_list`]: mounts only a bounded window of a real, ordered
//! dataset — real block-flow layout, [`KeyedExtents`]/[`ScrollAnchor`]
//! (`florui_reactive::virtualization`) unchanged, [`crate::use_scroll_offset`]/
//! [`crate::use_committed_size`] as the real scroll and real-measurement
//! sources.
//!
//! Fixed and variable height share one code path: [`ItemHeight::Fixed`]
//! seeds a [`KeyedExtents`] that is simply never measured (every key
//! permanently reports its estimate, which already behaves exactly like a
//! real fixed height), so a fixed-height list also skips per-row
//! [`crate::use_committed_size`] registration entirely — nothing to
//! measure.
//!
//! Mounting itself needs no new primitive: a plain loop over the visible
//! window calling [`florui_reactive::use_child_scope_keyed`] already gives
//! correct mount/reorder/unmount semantics. "Item identity is a data key,
//! never a recycled row index" is satisfied by that dispose-then-remount
//! alone — reusing a live `ComponentScope`'s own allocated hook slots
//! across different keys (skipping mount-effect cost during fast
//! scrolling) is a real, separate performance optimization, not attempted
//! here.

use std::ops::Range;
use std::rc::Rc;

use florui::Element;
use florui_reactive::{Key, KeyedExtents, use_child_scope_keyed, use_memo, use_ref, use_signal};

use crate::scroll::ScrollHandle;
use crate::size_observer::use_committed_size;
use crate::use_scroll_offset;

/// How an item's extent along the scroll axis is determined.
pub enum ItemHeight {
    /// Every item is exactly `height` logical pixels — no per-row
    /// measurement, no correction, ever.
    Fixed(f32),
    /// Items start at `estimate` and are refined in place via
    /// [`crate::use_committed_size`] once each visible row's real layout
    /// commits.
    Variable { estimate: f32 },
}

impl ItemHeight {
    fn default_estimate(&self) -> f32 {
        match self {
            ItemHeight::Fixed(height) => *height,
            ItemHeight::Variable { estimate } => *estimate,
        }
    }

    fn measures(&self) -> bool {
        matches!(self, ItemHeight::Variable { .. })
    }
}

/// Extra items mounted beyond the strictly visible range, so a fast
/// scroll doesn't show a blank frame while a new row's own layout/paint
/// catches up.
#[derive(Clone, Copy)]
pub enum Overscan {
    Items(usize),
    Pixels(f32),
}

impl Default for Overscan {
    fn default() -> Self {
        Overscan::Items(3)
    }
}

/// Cached per-`(item_count, dataset_version, extents_version)` layout:
/// every item's key, in order, plus its cumulative offset along the
/// scroll axis — `cumulative[i]` is the offset item `i` starts at, and
/// `cumulative[item_count]` is the dataset's total extent. Rebuilt only
/// when one of those three deps actually changes (see [`use_virtual_list`]'s
/// `use_memo` call) — not, critically, on every pure scroll-offset render.
struct ListLayout {
    keys: Vec<Key>,
    cumulative: Vec<f32>,
}

impl ListLayout {
    fn total(&self) -> f32 {
        *self.cumulative.last().unwrap_or(&0.0)
    }
}

fn build_layout(
    item_count: usize,
    key_for: &impl Fn(usize) -> Key,
    extents: &KeyedExtents,
) -> ListLayout {
    let mut keys = Vec::with_capacity(item_count);
    let mut cumulative = Vec::with_capacity(item_count + 1);
    let mut offset = 0.0;
    cumulative.push(0.0);
    for i in 0..item_count {
        let key = key_for(i);
        offset += extents.get(&key).value();
        cumulative.push(offset);
        keys.push(key);
    }
    ListLayout { keys, cumulative }
}

/// The visible window's `[start, end)` item indices, extended by
/// `overscan`, given `layout`'s cumulative offsets and the current
/// `(scroll_top, viewport_height)`.
fn visible_range(
    layout: &ListLayout,
    scroll_top: f32,
    viewport_height: f32,
    overscan: &Overscan,
) -> Range<usize> {
    let item_count = layout.keys.len();
    if item_count == 0 {
        return 0..0;
    }
    let (lo, hi) = match overscan {
        Overscan::Pixels(px) => (
            (scroll_top - px).max(0.0),
            scroll_top + viewport_height + px,
        ),
        Overscan::Items(_) => (scroll_top, scroll_top + viewport_height),
    };
    // The item whose own range straddles `lo` is one before the first
    // cumulative entry that already exceeds it.
    let start = layout.cumulative[..item_count]
        .partition_point(|&c| c <= lo)
        .saturating_sub(1);
    // Any item whose own range starts before `hi` needs to be mounted,
    // even if it extends past `hi` — this counts exactly those.
    let end = layout
        .cumulative
        .partition_point(|&c| c < hi)
        .min(item_count);
    match overscan {
        Overscan::Items(n) => (start.saturating_sub(*n))..(end + n).min(item_count),
        Overscan::Pixels(_) => start..end,
    }
}

/// A mounted virtualized list's live offset — returned by
/// [`use_virtual_list`] alongside the [`Element`] it actually renders.
#[derive(Clone)]
pub struct VirtualListHandle {
    scroll: ScrollHandle,
}

impl VirtualListHandle {
    /// The list's current scroll offset — reactive, the same as
    /// [`crate::scroll::ScrollHandle::offset`].
    pub fn offset(&self) -> (f32, f32) {
        self.scroll.offset()
    }
}

fn spacer(height: f32) -> Element {
    Element::node(
        "div",
        vec![("style".to_string(), format!("height: {height}px;"))],
        Vec::new(),
    )
}

/// Renders only the visible (plus `overscan`) window of an `item_count`-long
/// ordered dataset, keyed by `key_for` and rendered by `render_item` —
/// neither is ever called for an off-screen index. Returns the actual
/// content (two spacers plus the mounted rows) for the caller to place
/// inside their own scrollable element, whose `id` attribute must be this
/// same `id` (see [`crate::use_scroll_offset`]'s identical requirement)
/// and whose CSS must declare a real `overflow-y: scroll`/`auto` and a
/// definite height.
///
/// `dataset_version` is a caller-supplied token (any `PartialEq + Clone`,
/// e.g. a counter bumped whenever the caller's own backing collection is
/// reordered, filtered, or spliced) — the same "caller supplies deps,
/// nothing here infers them" contract [`florui_reactive::use_memo`]
/// already uses throughout this codebase.
pub fn use_virtual_list(
    id: impl Into<String>,
    item_count: usize,
    dataset_version: impl PartialEq + Clone + 'static,
    height: ItemHeight,
    overscan: Overscan,
    key_for: impl Fn(usize) -> Key,
    render_item: impl Fn(usize) -> Element,
) -> (Element, VirtualListHandle) {
    let id = id.into();
    let measures = height.measures();
    let extents = use_ref(|| KeyedExtents::new(height.default_estimate()));
    let extents_version = use_signal(|| 0u64);

    let layout: Rc<ListLayout> = {
        let extents = extents.clone();
        use_memo(
            (item_count, dataset_version, extents_version.get()),
            move |_| Rc::new(extents.with(|e| build_layout(item_count, &key_for, e))),
        )
    };

    let scroll = use_scroll_offset(id.clone(), |_, _| {});
    let (_, scroll_top) = scroll.offset();

    // `ScrollHandle::viewport_size` is a deliberately non-reactive query
    // against last-known geometry (same contract as `use_committed_size`
    // itself) -- reading it here would leave this list permanently empty
    // on its first real render, since nothing would ever schedule the
    // follow-up render that sees the real size `ScrollRegistry::sync`
    // only learns *after* this render's own layout commits. Mirroring it
    // into a real `Signal` via `use_committed_size` is what actually makes
    // "the container's real size just became known" cause a re-render.
    let viewport_height_signal = use_signal(|| 0.0f32);
    {
        let viewport_height_signal = viewport_height_signal.clone();
        use_committed_size(id.clone(), move |_w, h| viewport_height_signal.set(h));
    }
    let viewport_height = viewport_height_signal.get();
    let range = visible_range(&layout, scroll_top, viewport_height, &overscan);

    let before = layout.cumulative.get(range.start).copied().unwrap_or(0.0);
    let after = layout.total()
        - layout
            .cumulative
            .get(range.end)
            .copied()
            .unwrap_or_else(|| layout.total());

    let mut children = Vec::with_capacity(range.len() + 2);
    children.push(spacer(before));
    for i in range {
        let key = layout.keys[i].clone();
        let element = use_child_scope_keyed(key.clone(), || {
            if measures {
                let extents = extents.clone();
                let extents_version = extents_version.clone();
                let row_key = key.clone();
                use_committed_size(format!("{id}__row__{i}"), move |_w, h| {
                    let changed = extents.with_mut(|e| {
                        let before = e.get(&row_key).value();
                        e.measure(row_key.clone(), h);
                        (before - h).abs() > f32::EPSILON
                    });
                    if changed {
                        extents_version.set(extents_version.get() + 1);
                    }
                });
            }
            render_item(i)
        });
        children.push(element);
    }
    children.push(spacer(after.max(0.0)));

    (Element::Fragment(children), VirtualListHandle { scroll })
}

#[cfg(test)]
mod tests {
    use florui_style::{Arena, NodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::UiRuntime;

    fn viewport() -> Size<AvailableSpace> {
        Size {
            width: AvailableSpace::Definite(200.0),
            height: AvailableSpace::Definite(200.0),
        }
    }

    fn row(i: usize) -> Element {
        Element::node(
            "div",
            vec![("class".to_string(), "row".to_string())],
            vec![Element::text(i.to_string())],
        )
    }

    /// A fixed-height list of `item_count` rows inside a `viewport_height`px
    /// scrollable box, each row `item_height`px tall — the exact CSS
    /// numbers a test's own assertions reason about.
    fn build_runtime(
        item_count: usize,
        item_height: f32,
        viewport_height: f32,
        overscan: Overscan,
    ) -> UiRuntime {
        build_runtime_with_handle(item_count, item_height, viewport_height, overscan).0
    }

    fn build_runtime_with_handle(
        item_count: usize,
        item_height: f32,
        viewport_height: f32,
        overscan: Overscan,
    ) -> (
        UiRuntime,
        std::rc::Rc<std::cell::RefCell<Option<VirtualListHandle>>>,
    ) {
        let handle_slot: std::rc::Rc<std::cell::RefCell<Option<VirtualListHandle>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let root = {
            let handle_slot = std::rc::Rc::clone(&handle_slot);
            move || {
                let (content, handle) = use_virtual_list(
                    "list",
                    item_count,
                    (),
                    ItemHeight::Fixed(item_height),
                    overscan,
                    Key::from,
                    row,
                );
                *handle_slot.borrow_mut() = Some(handle);
                Element::node(
                    "div",
                    vec![
                        ("id".to_string(), "list".to_string()),
                        ("class".to_string(), "viewport".to_string()),
                    ],
                    vec![content],
                )
            }
        };
        let css = format!(
            ".viewport {{ width: 100px; height: {viewport_height}px; overflow-y: auto; }} \
             .row {{ height: {item_height}px; }}"
        );
        let mut runtime =
            UiRuntime::new(&css, root, viewport()).expect("this test's own CSS always parses");
        // The list's own real viewport height is only known to
        // `use_committed_size` *after* this first render's layout commits
        // -- its `Signal::set` schedules a render that hasn't happened
        // yet, so every test needs this second, real pass before the
        // mounted window reflects real geometry (the same "an effect's
        // own state change needs a follow-up update" pattern this crate's
        // other real-runtime tests already rely on).
        runtime.update(viewport());
        (runtime, handle_slot)
    }

    fn list_node(arena: &Arena) -> NodeId {
        arena
            .find(|a, id| a.id_attr(id) == Some("list"))
            .expect("the virtualized list's own viewport must carry the id it registered with")
    }

    fn mounted_row_texts(runtime: &UiRuntime) -> Vec<String> {
        let (arena, ..) = runtime.geometry();
        arena
            .children(list_node(arena))
            .iter()
            .filter(|&&id| arena.classes(id).iter().any(|c| c == "row"))
            .map(|&id| arena.text_content(id).to_string())
            .collect()
    }

    #[test]
    fn only_the_visible_window_is_mounted() {
        // 100 rows of 20px inside a 100px-tall viewport -- exactly 5 fit.
        let runtime = build_runtime(100, 20.0, 100.0, Overscan::Items(0));
        assert_eq!(
            mounted_row_texts(&runtime),
            vec!["0", "1", "2", "3", "4"],
            "only the strictly visible rows should ever be mounted, not the other 95"
        );
    }

    #[test]
    fn overscan_extends_the_mounted_window_on_both_sides() {
        // Scroll to the middle of a long list so overscan has room to
        // extend on both sides, not just clamp against an edge.
        let item_height = 20.0;
        let viewport_height = 100.0;
        let mut runtime = build_runtime(100, item_height, viewport_height, Overscan::Items(2));
        let target_offset = 40.0 * item_height; // scroll so item 40 is the first visible row
        assert!(
            runtime
                .scroll_registry()
                .scroll_to("list", 0.0, target_offset),
            "scrolling to a real, different offset must report a change"
        );
        runtime.update(viewport());

        // Visible window without overscan would be items 40..45; overscan
        // of 2 extends it to 38..47.
        let expected: Vec<String> = (38..47).map(|i| i.to_string()).collect();
        assert_eq!(mounted_row_texts(&runtime), expected);
    }

    #[test]
    fn scrolling_changes_which_rows_are_mounted() {
        let item_height = 20.0;
        let mut runtime = build_runtime(100, item_height, 100.0, Overscan::Items(0));
        assert_eq!(mounted_row_texts(&runtime)[0], "0");

        assert!(
            runtime
                .scroll_registry()
                .scroll_to("list", 0.0, 10.0 * item_height)
        );
        runtime.update(viewport());

        assert_eq!(
            mounted_row_texts(&runtime),
            vec!["10", "11", "12", "13", "14"],
            "scrolling must dispose the old window's rows and mount the new one"
        );
    }

    #[test]
    fn an_empty_dataset_mounts_no_rows_and_does_not_panic() {
        let runtime = build_runtime(0, 20.0, 100.0, Overscan::Items(3));
        assert!(mounted_row_texts(&runtime).is_empty());
    }
}

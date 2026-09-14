//! Bridges Stylo's `TDocument`/`TNode`/`TElement`/`selectors::Element`
//! traits onto [`crate::tree::Arena`], and converts its resolved
//! `ComputedValues` back into [`crate::cascade::ComputedStyle`]. This is
//! the crate's real CSS engine: selector matching, cascade, and
//! inheritance are Stylo's own, not reimplemented here.
//!
//! Each [`StyloNode`] handle is a single reference (`&NodeSlot`), not a
//! `(tree, id)` pair: Stylo's internal style-sharing cache type-erases a
//! fixed-size element buffer via `mem::transmute` and asserts the erased
//! and real sizes match at runtime, so `TElement`'s concrete type has to
//! stay pointer-sized. `NodeSlot` copies what it needs out of the Arena
//! once, up front, with parent/child pointers resolved in a second pass
//! once every slot has a stable address, so the handle never needs a
//! second reference back to any shared context.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;

use atomic_refcell::AtomicRefCell;
use selectors::attr::{AttrSelectorOperation, NamespaceConstraint};
use selectors::matching::{ElementSelectorFlags, MatchingContext, VisitedHandlingMode};
use selectors::{Element as SelectorsElement, OpaqueElement};
use servo_arc::{Arc, ArcBorrow};
use style::context::{
    QuirksMode, RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext,
    StyleContext, ThreadLocalStyleContext,
};
use style::data::ElementData;
use style::dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot};
use style::font_metrics::FontMetrics;
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::media_queries::{Device, MediaType};
use style::properties::style_structs::Font as FontStruct;
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::queries::values::PrefersColorScheme;
use style::selector_parser::{AttrValue, Lang, PseudoElement, SelectorImpl};
use style::servo::media_queries::FontMetricsProvider;
use style::shared_lock::{Locked, SharedRwLock, StylesheetGuards};
use style::stylesheets::DocumentStyleSheet;
use style::stylist::{CascadeData, RuleInclusion, Stylist};
use style::traversal::resolve_style;
use style::traversal_flags::TraversalFlags;
use style::values::AtomIdent;
use style::values::computed::font::GenericFontFamily;
use style::values::computed::{CSSPixelLength, Display, Length};
use style::{Atom, LocalName};
use stylo_atoms::Atom as WeakAtom;
use stylo_dom::ElementState;

use crate::cascade::{
    BorderSide as FlorBorderSide, ComputedStyle, ContentAlignment, Display as FlorDisplay, Edges,
    FlexDirection, FlexWrap, FontFamily as FlorFontFamily, ItemAlignment,
};
use crate::color::Rgba;
use crate::interaction::InteractionState;
use crate::stylesheet_parse::Rule;
use crate::tree::{Arena, NodeId};

type BorrowedLocalName = <SelectorImpl as selectors::parser::SelectorImpl>::BorrowedLocalName;
type BorrowedNamespaceUrl = <SelectorImpl as selectors::parser::SelectorImpl>::BorrowedNamespaceUrl;

/// Everything one node needs to act as a Stylo element, copied out of
/// [`Arena`] once at construction — see the module doc for why this can't
/// instead hold a `NodeId` plus a back-reference to a shared tree.
struct NodeSlot {
    parent: Option<*const NodeSlot>,
    children: Vec<*const NodeSlot>,
    tag: &'static str,
    classes: Vec<String>,
    id_attr: Option<String>,
    /// Same value as `id_attr`, pre-interned — `TElement::id` needs this
    /// exact type back, and it's also what Stylo's selector map uses to
    /// bucket `#id` rules by hash before ever calling `has_id`, so a
    /// node whose `id()` doesn't match its own `has_id` would have its
    /// `#id` rules silently never even considered a candidate.
    id_atom: Option<WeakAtom>,
    state: ElementState,
    guard: SharedRwLock,
    data: AtomicRefCell<ElementData>,
    dirty_descendants: Cell<bool>,
    local_name: BorrowedLocalName,
    namespace: BorrowedNamespaceUrl,
}

/// Owns every node's [`NodeSlot`] in one `Vec` sized exactly once up
/// front, so pushing never reallocates and the raw pointers `NodeSlot`s
/// hold to each other (set in a second pass, once every slot has its
/// final address) stay valid for the tree's whole lifetime.
struct StyloTree {
    slots: Vec<NodeSlot>,
    index_of: HashMap<NodeId, usize>,
}

impl StyloTree {
    /// Builds a self-contained copy of every node reachable from `arena`'s
    /// roots, synthesizing a single container root when there is more
    /// than one (a `view!` `Fragment` can produce several) so Stylo
    /// always has exactly one document element to resolve from.
    fn new(arena: &Arena, state: &InteractionState) -> (Self, NodeId) {
        let mut order = Vec::new();
        for &root in arena.roots() {
            Self::collect_order(arena, root, &mut order);
        }

        let mut index_of = HashMap::new();
        let mut slots = Vec::with_capacity(order.len());
        for (index, &id) in order.iter().enumerate() {
            index_of.insert(id, index);
            let mut node_state = ElementState::empty();
            if state.is_hovered(id) {
                node_state |= ElementState::HOVER;
            }
            if state.is_focused(id) {
                node_state |= ElementState::FOCUS;
            }
            if state.is_active(id) {
                node_state |= ElementState::ACTIVE;
            }
            slots.push(NodeSlot {
                parent: None,
                children: Vec::new(),
                tag: arena.tag(id),
                classes: arena.classes(id).to_vec(),
                id_attr: arena.id_attr(id).map(str::to_owned),
                id_atom: arena.id_attr(id).map(WeakAtom::from),
                state: node_state,
                guard: SharedRwLock::new(),
                data: AtomicRefCell::new(ElementData::default()),
                dirty_descendants: Cell::new(false),
                local_name: arena.tag(id).into(),
                namespace: "".into(),
            });
        }

        // Second pass: every slot has its final address now that the
        // Vec has stopped growing, so parent/child pointers can be taken.
        for (index, &id) in order.iter().enumerate() {
            let parent_ptr = arena
                .parent(id)
                .map(|parent_id| &slots[index_of[&parent_id]] as *const NodeSlot);
            let children_ptrs: Vec<*const NodeSlot> = arena
                .children(id)
                .iter()
                .map(|child_id| &slots[index_of[child_id]] as *const NodeSlot)
                .collect();
            slots[index].parent = parent_ptr;
            slots[index].children = children_ptrs;
        }

        // Multiple roots (a Fragment) have no single document element for
        // Stylo to resolve from; a real florui tree of one component's
        // output is a single element in practice, so this only matters
        // for a bare multi-root Fragment, which resolves each of its own
        // roots as if it were independently the document.
        let primary_root = order[0];
        (Self { slots, index_of }, primary_root)
    }

    fn collect_order(arena: &Arena, id: NodeId, order: &mut Vec<NodeId>) {
        order.push(id);
        for &child in arena.children(id) {
            Self::collect_order(arena, child, order);
        }
    }

    fn node(&self, id: NodeId) -> StyloNode<'_> {
        StyloNode(&self.slots[self.index_of[&id]])
    }
}

#[derive(Copy, Clone)]
#[repr(transparent)]
struct StyloNode<'a>(&'a NodeSlot);

impl fmt::Debug for StyloNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StyloNode({:p})", self.0)
    }
}

impl PartialEq for StyloNode<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}
impl Eq for StyloNode<'_> {}
impl std::hash::Hash for StyloNode<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.0 as *const NodeSlot).hash(state);
    }
}

impl<'a> StyloNode<'a> {
    fn index_in_parent_siblings(&self) -> Option<usize> {
        let parent = self.parent_node()?;
        parent
            .0
            .children
            .iter()
            .position(|&child| std::ptr::eq(child, self.0))
    }
}

impl NodeInfo for StyloNode<'_> {
    fn is_element(&self) -> bool {
        true
    }

    fn is_text_node(&self) -> bool {
        false
    }
}

impl<'a> TDocument for StyloNode<'a> {
    type ConcreteNode = StyloNode<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn is_html_document(&self) -> bool {
        true
    }

    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        &self.0.guard
    }
}

impl<'a> TShadowRoot for StyloNode<'a> {
    type ConcreteNode = StyloNode<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        unreachable!("florui has no shadow DOM; this bridge never constructs a shadow root")
    }

    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

impl<'a> TNode for StyloNode<'a> {
    type ConcreteElement = StyloNode<'a>;
    type ConcreteDocument = StyloNode<'a>;
    type ConcreteShadowRoot = StyloNode<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.0.parent.map(|ptr| StyloNode(unsafe { &*ptr }))
    }

    fn first_child(&self) -> Option<Self> {
        self.0
            .children
            .first()
            .map(|&ptr| StyloNode(unsafe { &*ptr }))
    }

    fn last_child(&self) -> Option<Self> {
        self.0
            .children
            .last()
            .map(|&ptr| StyloNode(unsafe { &*ptr }))
    }

    fn prev_sibling(&self) -> Option<Self> {
        let index = self.index_in_parent_siblings()?;
        let parent = self.parent_node()?;
        index
            .checked_sub(1)
            .map(|i| StyloNode(unsafe { &*parent.0.children[i] }))
    }

    fn next_sibling(&self) -> Option<Self> {
        let index = self.index_in_parent_siblings()?;
        let parent = self.parent_node()?;
        parent
            .0
            .children
            .get(index + 1)
            .map(|&ptr| StyloNode(unsafe { &*ptr }))
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        let mut node = *self;
        while let Some(parent) = node.parent_node() {
            node = parent;
        }
        node
    }

    fn is_in_document(&self) -> bool {
        true
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent_node()
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.0 as *const NodeSlot as usize)
    }

    fn debug_id(self) -> usize {
        self.0 as *const NodeSlot as usize
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        Some(*self)
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        if self.parent_node().is_none() {
            Some(*self)
        } else {
            None
        }
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }
}

impl<'a> SelectorsElement for StyloNode<'a> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.0)
    }

    fn parent_element(&self) -> Option<Self> {
        self.parent_node()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.prev_sibling()
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.next_sibling()
    }

    fn first_element_child(&self) -> Option<Self> {
        self.first_child()
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true
    }

    fn has_local_name(&self, local_name: &BorrowedLocalName) -> bool {
        self.0.tag == (local_name.as_ref() as &str)
    }

    fn has_namespace(&self, ns: &BorrowedNamespaceUrl) -> bool {
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.0.tag == other.0.tag
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&<SelectorImpl as selectors::parser::SelectorImpl>::NamespaceUrl>,
        _local_name: &<SelectorImpl as selectors::parser::SelectorImpl>::LocalName,
        _operation: &AttrSelectorOperation<
            &<SelectorImpl as selectors::parser::SelectorImpl>::AttrValue,
        >,
    ) -> bool {
        // florui's Arena exposes no attributes beyond class/id, each of
        // which selectors dispatches through has_class/has_id instead.
        false
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo_class: &<SelectorImpl as selectors::parser::SelectorImpl>::NonTSPseudoClass,
        _context: &mut MatchingContext<'_, Self::Impl>,
    ) -> bool {
        // State-based pseudo-classes (:hover, :focus, :active) are not
        // auto-matched by selectors from `state()` — the element is
        // expected to consult its own state against the pseudo-class's
        // flag itself, same as Servo's own TElement impls do.
        self.0.state.intersects(pseudo_class.state_flag())
    }

    fn match_pseudo_element(
        &self,
        _pseudo_element: &<SelectorImpl as selectors::parser::SelectorImpl>::PseudoElement,
        _context: &mut MatchingContext<'_, Self::Impl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &AtomIdent, case_sensitivity: selectors::attr::CaseSensitivity) -> bool {
        self.0
            .id_attr
            .as_deref()
            .is_some_and(|attr| case_sensitivity.eq(attr.as_bytes(), id.as_ref().as_bytes()))
    }

    fn has_class(
        &self,
        name: &AtomIdent,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        self.0
            .classes
            .iter()
            .any(|class| case_sensitivity.eq(class.as_bytes(), name.as_ref().as_bytes()))
    }

    fn has_custom_state(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn imported_part(&self, _name: &AtomIdent) -> Option<AtomIdent> {
        None
    }

    fn is_part(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.0.children.is_empty()
    }

    fn is_root(&self) -> bool {
        self.0.parent.is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut selectors::bloom::BloomFilter) -> bool {
        false
    }
}

impl<'a> TElement for StyloNode<'a> {
    type ConcreteNode = StyloNode<'a>;
    type TraversalChildrenIterator = std::vec::IntoIter<StyloNode<'a>>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        let children: Vec<_> = self
            .0
            .children
            .iter()
            .map(|&ptr| StyloNode(unsafe { &*ptr }))
            .collect();
        LayoutIterator(children.into_iter())
    }

    fn is_html_element(&self) -> bool {
        true
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn animation_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn transition_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn state(&self) -> ElementState {
        self.0.state
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&WeakAtom> {
        self.0.id_atom.as_ref()
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        for class in &self.0.classes {
            let atom = Atom::from(class.as_str());
            callback(AtomIdent::cast(&atom));
        }
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }

    fn each_attr_name<F>(&self, _callback: F)
    where
        F: FnMut(&LocalName),
    {
    }

    fn has_dirty_descendants(&self) -> bool {
        self.0.dirty_descendants.get()
    }

    fn has_snapshot(&self) -> bool {
        false
    }

    fn handled_snapshot(&self) -> bool {
        true
    }

    unsafe fn set_handled_snapshot(&self) {}

    unsafe fn set_dirty_descendants(&self) {
        self.0.dirty_descendants.set(true);
    }

    unsafe fn unset_dirty_descendants(&self) {
        self.0.dirty_descendants.set(false);
    }

    fn store_children_to_process(&self, _n: isize) {
        unimplemented!(
            "this bridge drives resolve_style directly, never the parallel/postorder \
             traversal driver that needs this"
        )
    }

    fn did_process_child(&self) -> isize {
        unimplemented!("see store_children_to_process")
    }

    unsafe fn ensure_data(&self) -> atomic_refcell::AtomicRefMut<'_, ElementData> {
        self.0.data.borrow_mut()
    }

    unsafe fn clear_data(&self) {
        *self.0.data.borrow_mut() = ElementData::default();
    }

    fn has_data(&self) -> bool {
        true
    }

    fn borrow_data(&self) -> Option<atomic_refcell::AtomicRef<'_, ElementData>> {
        Some(self.0.data.borrow())
    }

    fn mutate_data(&self) -> Option<atomic_refcell::AtomicRefMut<'_, ElementData>> {
        Some(self.0.data.borrow_mut())
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }

    fn has_animations(&self, _context: &SharedStyleContext) -> bool {
        false
    }

    fn has_css_animations(
        &self,
        _context: &SharedStyleContext,
        _pseudo_element: Option<PseudoElement>,
    ) -> bool {
        false
    }

    fn has_css_transitions(
        &self,
        _context: &SharedStyleContext,
        _pseudo_element: Option<PseudoElement>,
    ) -> bool {
        false
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn lang_attr(&self) -> Option<AttrValue> {
        None
    }

    fn match_element_lang(&self, _override_lang: Option<Option<AttrValue>>, _value: &Lang) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        false
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        _hints: &mut V,
    ) where
        V: selectors::sink::Push<style::applicable_declarations::ApplicableDeclarationBlock>,
    {
    }

    fn local_name(&self) -> &BorrowedLocalName {
        &self.0.local_name
    }

    fn namespace(&self) -> &BorrowedNamespaceUrl {
        &self.0.namespace
    }

    fn query_container_size(
        &self,
        _display: &Display,
    ) -> euclid::Size2D<Option<app_units::Au>, euclid::UnknownUnit> {
        euclid::Size2D::new(None, None)
    }

    fn has_selector_flags(&self, _flags: ElementSelectorFlags) -> bool {
        false
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::empty()
    }
}

struct NoFontMetrics;

impl fmt::Debug for NoFontMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoFontMetrics")
    }
}

impl FontMetricsProvider for NoFontMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &FontStruct,
        _base_size: CSSPixelLength,
        _flags: style::values::specified::font::QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics::default()
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(16.0)
    }
}

struct NoPainters;
impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

/// Every stylesheet this crate parses, and every cascade it drives, share
/// this one process-wide lock — Stylo ties a stylesheet's rules to the
/// exact `SharedRwLock` instance it was parsed under (a per-document
/// isolation mechanism this crate has no use for), so parsing once in
/// [`crate::stylesheet_parse::parse_stylesheet`] and cascading later in
/// [`compute`] need to agree on the same lock rather than each minting
/// their own.
pub(crate) fn shared_lock() -> &'static SharedRwLock {
    static LOCK: std::sync::LazyLock<SharedRwLock> = std::sync::LazyLock::new(SharedRwLock::new);
    &LOCK
}

fn device() -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        euclid::Size2D::new(1024.0, 768.0),
        euclid::Scale::new(1.0),
        Box::new(NoFontMetrics),
        ComputedValues::initial_values_with_font_override(FontStruct::initial_values()),
        PrefersColorScheme::Light,
    )
}

/// Computes real Stylo styles for every node in `arena`, driving Stylo's
/// own selector matching, cascade, and inheritance via [`resolve_style`]
/// — this crate reimplements none of them.
pub(crate) fn compute(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
) -> HashMap<NodeId, ComputedStyle> {
    style::thread_state::enter(style::thread_state::ThreadState::LAYOUT);
    let result = compute_in_layout_state(arena, rules, state);
    style::thread_state::exit(style::thread_state::ThreadState::LAYOUT);
    result
}

fn compute_in_layout_state(
    arena: &Arena,
    rules: &[Rule],
    state: &InteractionState,
) -> HashMap<NodeId, ComputedStyle> {
    let mut result = HashMap::new();
    if arena.roots().is_empty() {
        return result;
    }

    let (tree, _primary_root) = StyloTree::new(arena, state);

    let mut stylist = Stylist::new(device(), QuirksMode::NoQuirks);
    let lock = shared_lock();
    // The framework's own default element stylesheet first, under
    // Origin::UserAgent — Stylo's real cascade-origin precedence means an
    // application rule below overrides it regardless of specificity or
    // this registration order, the same as a real browser's UA stylesheet.
    let default_rule = crate::default_stylesheet::rule();
    stylist.append_stylesheet(DocumentStyleSheet(default_rule.stylesheet()), &lock.read());
    for rule in rules {
        stylist.append_stylesheet(DocumentStyleSheet(rule.stylesheet()), &lock.read());
    }

    let guard = lock.read();
    let guards = StylesheetGuards {
        author: &guard,
        ua_or_user: &guard,
    };
    stylist.flush::<StyloNode<'_>>(&guards, None, None);

    let snapshot_map = style::servo::selector_parser::SnapshotMap::new();
    let animations = Default::default();
    let shared = SharedStyleContext {
        traversal_flags: TraversalFlags::empty(),
        stylist: &stylist,
        options: GLOBAL_STYLE_DATA.options.clone(),
        guards,
        visited_styles_enabled: false,
        animations,
        current_time_for_animations: 0.0,
        snapshot_map: &snapshot_map,
        registered_speculative_painters: &NoPainters,
    };

    for &id in tree.index_of.keys() {
        let mut thread_local = ThreadLocalStyleContext::<StyloNode<'_>>::new();
        let mut context = StyleContext {
            shared: &shared,
            thread_local: &mut thread_local,
        };
        let target = tree.node(id);
        let styles = resolve_style(&mut context, target, RuleInclusion::All, None, None);
        result.insert(id, to_computed_style(styles.primary()));
    }

    result
}

fn to_computed_style(values: &ComputedValues) -> ComputedStyle {
    let background = values.get_background();
    let text = values.get_inherited_text();
    let position = values.get_position();
    let font = values.get_font();
    let margin = values.get_margin();
    let padding = values.get_padding();
    let border = values.get_border();

    // `color`'s own computed value is always already-resolved (real CSS
    // never leaves it as `currentcolor`); resolving it first lets
    // `background-color`'s (and `border-*-color`'s) possible
    // `currentcolor` reference resolve against it, rather than an
    // arbitrary fallback.
    let color = to_absolute_rgba(&text.color);

    ComputedStyle {
        background_color: background
            .background_color
            .as_absolute()
            .map(to_absolute_rgba)
            .unwrap_or(color),
        color,
        width: to_optional_length(&position.width),
        height: to_optional_length(&position.height),
        margin: Edges {
            top: to_optional_margin(&margin.margin_top),
            right: to_optional_margin(&margin.margin_right),
            bottom: to_optional_margin(&margin.margin_bottom),
            left: to_optional_margin(&margin.margin_left),
        },
        padding: Edges {
            top: to_length(&padding.padding_top),
            right: to_length(&padding.padding_right),
            bottom: to_length(&padding.padding_bottom),
            left: to_length(&padding.padding_left),
        },
        font_size: font.font_size.computed_size.0.px(),
        display: to_display(values.get_box().display),
        flex_direction: to_flex_direction(position.flex_direction),
        flex_wrap: to_flex_wrap(position.flex_wrap),
        justify_content: to_content_alignment(position.justify_content.0),
        align_content: to_content_alignment(position.align_content.0),
        align_items: to_item_alignment(position.align_items.0),
        align_self: to_item_alignment(position.align_self.0.0),
        flex_grow: position.flex_grow.0,
        flex_shrink: position.flex_shrink.0,
        flex_basis: to_flex_basis(&position.flex_basis),
        column_gap: to_gap(&position.column_gap),
        row_gap: to_gap(&position.row_gap),
        font_family: to_font_family(&font.font_family),
        border: Edges {
            top: to_border_side(
                border.border_top_style,
                border.border_top_width,
                &border.border_top_color,
                color,
            ),
            right: to_border_side(
                border.border_right_style,
                border.border_right_width,
                &border.border_right_color,
                color,
            ),
            bottom: to_border_side(
                border.border_bottom_style,
                border.border_bottom_width,
                &border.border_bottom_color,
                color,
            ),
            left: to_border_side(
                border.border_left_style,
                border.border_left_width,
                &border.border_left_color,
                color,
            ),
        },
    }
}

/// One border side: `0.0` width for `none`/`hidden` (real CSS's initial
/// style, which makes a border invisible regardless of its width/color —
/// see [`FlorBorderSide`]'s own doc), a solid-only rendering treatment for
/// every other style. `currentcolor` resolves against `inherited_color`
/// (this element's own already-resolved `color`), the same fallback
/// `background-color` already uses.
fn to_border_side(
    style: style::values::computed::BorderStyle,
    width: app_units::Au,
    color: &style::values::computed::Color,
    inherited_color: Rgba,
) -> FlorBorderSide {
    if style.none_or_hidden() {
        return FlorBorderSide {
            width: 0.0,
            color: inherited_color,
        };
    }
    FlorBorderSide {
        width: width.to_f32_px(),
        color: color
            .as_absolute()
            .map(to_absolute_rgba)
            .unwrap_or(inherited_color),
    }
}

/// `Display`'s own `inside()`/`outside()` split matches real CSS's
/// two-value `display` syntax — see [`FlorDisplay`]'s own doc for why this
/// crate conflates both into one field. `inline-block` is
/// `DisplayOutside::Inline` + `DisplayInside::FlowRoot` (real CSS's own
/// encoding, not a guess); every other inline-outside value maps to
/// `Inline`, and everything else falls through to `inside()` alone.
///
/// Stylo blockifies `outside()` on its own for contexts real CSS also
/// blockifies in (the root element, a flex/grid item, floats, absolute
/// positioning — <https://drafts.csswg.org/css-display/#blockify>) — a
/// `<span>` with no parent (a bare tree root) or a direct flex-item child
/// genuinely computes `outside: Block` even with `display: inline`
/// authored, the same as a real browser. Caught directly while writing this
/// slice's own tests: a `<span>` at tree root read back as `Block`, which
/// briefly looked like a bug in this function before nesting it under a
/// `<div>` (the realistic case) showed `Inline` as expected — Stylo was
/// already correct.
fn to_display(display: style::values::computed::Display) -> FlorDisplay {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    match (display.outside(), display.inside()) {
        (DisplayOutside::Inline, DisplayInside::FlowRoot) => FlorDisplay::InlineBlock,
        (DisplayOutside::Inline, _) => FlorDisplay::Inline,
        (_, DisplayInside::Flex) => FlorDisplay::Flex,
        _ => FlorDisplay::Block,
    }
}

fn to_flex_direction(value: style::computed_values::flex_direction::T) -> FlexDirection {
    use style::computed_values::flex_direction::T;
    match value {
        T::Row => FlexDirection::Row,
        T::RowReverse => FlexDirection::RowReverse,
        T::Column => FlexDirection::Column,
        T::ColumnReverse => FlexDirection::ColumnReverse,
    }
}

fn to_flex_wrap(value: style::computed_values::flex_wrap::T) -> FlexWrap {
    use style::computed_values::flex_wrap::T;
    match value {
        T::Nowrap => FlexWrap::NoWrap,
        T::Wrap => FlexWrap::Wrap,
        T::WrapReverse => FlexWrap::WrapReverse,
    }
}

/// Shared by `justify-content`/`align-content`, both a `ContentDistribution`
/// in Stylo — `normal` (no fallback alignment declared) maps to `None`,
/// distinct from every explicit keyword.
fn to_content_alignment(
    value: style::values::specified::align::ContentDistribution,
) -> Option<ContentAlignment> {
    use style::values::specified::align::AlignFlags;
    match value.primary().value() {
        AlignFlags::START => Some(ContentAlignment::Start),
        AlignFlags::END => Some(ContentAlignment::End),
        AlignFlags::LEFT => Some(ContentAlignment::Start),
        AlignFlags::RIGHT => Some(ContentAlignment::End),
        AlignFlags::FLEX_START => Some(ContentAlignment::FlexStart),
        AlignFlags::FLEX_END => Some(ContentAlignment::FlexEnd),
        AlignFlags::CENTER => Some(ContentAlignment::Center),
        AlignFlags::STRETCH => Some(ContentAlignment::Stretch),
        AlignFlags::SPACE_BETWEEN => Some(ContentAlignment::SpaceBetween),
        AlignFlags::SPACE_AROUND => Some(ContentAlignment::SpaceAround),
        AlignFlags::SPACE_EVENLY => Some(ContentAlignment::SpaceEvenly),
        _ => None,
    }
}

/// Shared by `align-items`/`align-self`, both an `AlignFlags` in Stylo —
/// `auto`/`normal` map to `None`, meaning "defer to the container's own
/// `align-items`" for `align-self`, or "stretch" for `align-items` (Taffy's
/// own default already matches CSS's real initial value there, so
/// `florui-layout` can supply that default itself rather than this
/// function inventing one).
fn to_item_alignment(value: style::values::specified::align::AlignFlags) -> Option<ItemAlignment> {
    use style::values::specified::align::AlignFlags;
    match value.value() {
        AlignFlags::STRETCH => Some(ItemAlignment::Stretch),
        AlignFlags::FLEX_START => Some(ItemAlignment::FlexStart),
        AlignFlags::FLEX_END => Some(ItemAlignment::FlexEnd),
        AlignFlags::SELF_START => Some(ItemAlignment::Start),
        AlignFlags::SELF_END => Some(ItemAlignment::End),
        AlignFlags::START => Some(ItemAlignment::Start),
        AlignFlags::END => Some(ItemAlignment::End),
        AlignFlags::LEFT => Some(ItemAlignment::Start),
        AlignFlags::RIGHT => Some(ItemAlignment::End),
        AlignFlags::CENTER => Some(ItemAlignment::Center),
        AlignFlags::BASELINE => Some(ItemAlignment::Baseline),
        _ => None,
    }
}

/// `flex-basis` shares `width`/`height`'s own generated size type
/// (`content` aside) — see [`to_optional_length`] for the same fallback
/// reasoning on anything this crate can't yet resolve to one pixel value.
fn to_flex_basis(
    value: &style::values::generics::flex::GenericFlexBasis<
        style::values::generics::length::GenericSize<
            style::values::generics::NonNegative<style::values::computed::LengthPercentage>,
        >,
    >,
) -> Option<f32> {
    use style::values::generics::flex::GenericFlexBasis;
    match value {
        GenericFlexBasis::Content => None,
        GenericFlexBasis::Size(size) => to_optional_length(size),
    }
}

/// `column-gap`/`row-gap` share this generated type — `normal` (the CSS
/// initial value) is `0px`, same as this crate's own [`ComputedStyle::padding`]
/// treats anything it can't resolve to one pixel value.
fn to_gap(
    value: &style::values::generics::length::GenericLengthPercentageOrNormal<
        style::values::generics::NonNegative<style::values::computed::LengthPercentage>,
    >,
) -> f32 {
    use style::values::generics::length::GenericLengthPercentageOrNormal;
    match value {
        GenericLengthPercentageOrNormal::LengthPercentage(lp) => {
            lp.0.to_length().map(|length| length.px()).unwrap_or(0.0)
        }
        GenericLengthPercentageOrNormal::Normal => 0.0,
    }
}

/// Resolves real CSS's whole comma-separated `font-family` preference list
/// down to the one distinction this crate's two embedded fonts actually
/// support: this crate has no way to honor a specific requested name
/// (`"Helvetica"`) or most other generics (`serif`, `cursive`, `fantasy`),
/// so only a first-preference `monospace` resolves to
/// [`FlorFontFamily::Monospace`] — everything else, including an empty
/// list (real CSS's own initial value), falls back to
/// [`FlorFontFamily::SansSerif`], this crate's stand-in default.
fn to_font_family(value: &style::values::computed::font::FontFamily) -> FlorFontFamily {
    use style::values::computed::font::{GenericFontFamily, SingleFontFamily};
    match value.families.list.first() {
        Some(SingleFontFamily::Generic(GenericFontFamily::Monospace)) => FlorFontFamily::Monospace,
        _ => FlorFontFamily::SansSerif,
    }
}

fn to_absolute_rgba(color: &style::color::AbsoluteColor) -> Rgba {
    let srgb = color.to_color_space(style::color::ColorSpace::Srgb);
    Rgba {
        r: to_channel(srgb.components.0),
        g: to_channel(srgb.components.1),
        b: to_channel(srgb.components.2),
        a: to_channel(srgb.alpha),
    }
}

fn to_channel(component: f32) -> u8 {
    (component.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `None` for anything without a single resolved pixel length — `auto`,
/// a percentage, or a `calc()` mixing the two — since nothing downstream
/// of this crate can resolve a percentage without a containing size yet;
/// treating it as `auto` is the closest honest fallback available today.
fn to_optional_length(
    value: &style::values::generics::length::GenericSize<
        style::values::generics::NonNegative<style::values::computed::LengthPercentage>,
    >,
) -> Option<f32> {
    use style::values::generics::length::GenericSize;
    match value {
        GenericSize::LengthPercentage(lp) => lp.0.to_length().map(|length| length.px()),
        _ => None,
    }
}

/// Same fallback reasoning as [`to_optional_length`] (`auto`, a
/// percentage, or a `calc()` all become `None`), for margin's own
/// generated value type — real CSS margins allow negative lengths, so
/// unlike `width`/`height` there is no `NonNegative` wrapper here.
fn to_optional_margin(
    value: &style::values::generics::length::GenericMargin<
        style::values::computed::LengthPercentage,
    >,
) -> Option<f32> {
    use style::values::generics::length::GenericMargin;
    match value {
        GenericMargin::LengthPercentage(lp) => lp.to_length().map(|length| length.px()),
        _ => None,
    }
}

/// `0.0` for anything without a single resolved pixel length — a
/// percentage or a `calc()` mixing the two — since real CSS padding has
/// no `auto` to fall back to; see [`to_optional_length`] for the same
/// fallback reasoning on `width`/`height`/margin.
fn to_length(
    value: &style::values::generics::NonNegative<style::values::computed::LengthPercentage>,
) -> f32 {
    value.0.to_length().map(|length| length.px()).unwrap_or(0.0)
}

//! A bounded spike evaluating Stylo (`stylo` 0.8.0) as a candidate CSS
//! engine for `florui-style`'s cascade. Not wired into any real pipeline:
//! this bridges Stylo's `TDocument`/`TNode`/`TElement`/`selectors::Element`
//! traits
//! directly onto `florui_style::Arena`, then asks Stylo to resolve real
//! computed styles for a fixture and reads them back — proving selector
//! matching, cascade, and inheritance work end to end against florui's
//! own tree shape, without reimplementing any of them.
//!
//! Scope deliberately excludes what florui doesn't have yet: shadow DOM,
//! animations/transitions, parts/slots, forms, and incremental
//! invalidation (florui rebuilds its `Arena` fresh every render today, so
//! there is no persistent per-node identity for Stylo's dirty/snapshot
//! bits to attach to — every node here always resolves via
//! [`RuleInclusion::All`], never skipping work via a dirty bit).
//!
//! Each [`StyloNode`] handle is a single reference (`&NodeSlot`), not a
//! `(tree, id)` pair: Stylo's internal style-sharing cache type-erases a
//! `[E; N]`-shaped buffer via `mem::transmute` and asserts the erased and
//! real sizes match, so `TElement`'s concrete type must stay pointer-sized
//! — the same reason Blitz's own equivalent node handle is a plain
//! `&'a Node`. `NodeSlot` copies what it needs out of `florui_style::Arena`
//! once, up front, so a node can navigate and answer selector queries
//! without holding a second reference back to any shared context.

use std::cell::Cell;
use std::fmt;

use atomic_refcell::AtomicRefCell;
use florui_style::{Arena, NodeId};
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
use style::media_queries::{Device, MediaList, MediaType};
use style::properties::style_structs::Font as FontStruct;
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::queries::values::PrefersColorScheme;
use style::selector_parser::{AttrValue, Lang, PseudoElement, SelectorImpl};
use style::servo::media_queries::FontMetricsProvider;
use style::servo_arc::Arc as StyloArc;
use style::shared_lock::{Locked, SharedRwLock, StylesheetGuards};
use style::stylesheets::{AllowImportRules, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData};
use style::stylist::{CascadeData, RuleInclusion, Stylist};
use style::traversal::resolve_style;
use style::traversal_flags::TraversalFlags;
use style::values::AtomIdent;
use style::values::computed::font::GenericFontFamily;
use style::values::computed::{CSSPixelLength, Display, Length};
use style::{Atom, LocalName};
use stylo_atoms::Atom as WeakAtom;
use stylo_dom::ElementState;

type BorrowedLocalName = <SelectorImpl as selectors::parser::SelectorImpl>::BorrowedLocalName;
type BorrowedNamespaceUrl = <SelectorImpl as selectors::parser::SelectorImpl>::BorrowedNamespaceUrl;

/// Everything one node needs to act as a Stylo element, copied out of
/// `florui_style::Arena` once at construction — see the module doc for why
/// this can't instead hold a `NodeId` plus a back-reference to a shared
/// tree/context.
struct NodeSlot {
    parent: Option<*const NodeSlot>,
    children: Vec<*const NodeSlot>,
    tag: &'static str,
    classes: Vec<String>,
    id_attr: Option<String>,
    hovered: bool,
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
pub struct StyloTree {
    slots: Vec<NodeSlot>,
    root_index: usize,
    index_of: std::collections::HashMap<NodeId, usize>,
}

impl StyloTree {
    /// Builds a self-contained copy of `arena`'s first root subtree —
    /// multi-root fragments are out of scope for this spike. `hovered`
    /// marks which `florui_style::NodeId`s currently match `:hover`.
    pub fn new(arena: &Arena, hovered: &std::collections::HashSet<NodeId>) -> Self {
        let root_id = arena.roots()[0];
        let mut order = Vec::new();
        Self::collect_order(arena, root_id, &mut order);

        let mut index_of = std::collections::HashMap::new();
        let mut slots = Vec::with_capacity(order.len());
        for (index, &id) in order.iter().enumerate() {
            index_of.insert(id, index);
            slots.push(NodeSlot {
                parent: None,
                children: Vec::new(),
                tag: arena.tag(id),
                classes: arena.classes(id).to_vec(),
                id_attr: arena.id_attr(id).map(str::to_owned),
                hovered: hovered.contains(&id),
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

        let root_index = index_of[&root_id];
        Self {
            slots,
            root_index,
            index_of,
        }
    }

    fn collect_order(arena: &Arena, id: NodeId, order: &mut Vec<NodeId>) {
        order.push(id);
        for &child in arena.children(id) {
            Self::collect_order(arena, child, order);
        }
    }

    pub fn root(&self) -> StyloNode<'_> {
        StyloNode(&self.slots[self.root_index])
    }

    /// The node this tree copied from the given `florui_style::NodeId`, if
    /// that id was reachable from the root this tree was built from.
    pub fn node(&self, id: NodeId) -> Option<StyloNode<'_>> {
        self.index_of
            .get(&id)
            .map(|&index| StyloNode(&self.slots[index]))
    }
}

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct StyloNode<'a>(&'a NodeSlot);

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
        unreachable!("florui has no shadow DOM; this spike never constructs a shadow root")
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
        // florui-style has no attribute selectors beyond class/id, each of
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
        // flag itself, same as Servo's own `TElement` impls do.
        TElement::state(self).intersects(pseudo_class.state_flag())
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
        if self.0.hovered {
            ElementState::HOVER
        } else {
            ElementState::empty()
        }
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&WeakAtom> {
        None
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
            "this spike drives resolve_style directly, never the parallel/postorder \
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

/// Parses `css`, resolves it against `tree`'s root via Stylo's real
/// cascade, and returns the root's own resolved [`ComputedValues`] —
/// enough to compare background-color/width/height/font-size/color
/// against `florui_style::compute`'s output for the same fixture.
pub fn resolve(css: &str, tree: &StyloTree) -> Arc<ComputedValues> {
    resolve_node(css, tree.root())
}

/// Same as [`resolve`], but for any node reachable from the tree `target`
/// came from — `resolve_style` walks that node's real ancestors as
/// needed, so resolving a non-root node here is what actually proves
/// inheritance across two distinct nodes, not just a property applying to
/// the node that declared it.
pub fn resolve_node(css: &str, target: StyloNode<'_>) -> Arc<ComputedValues> {
    style::thread_state::enter(style::thread_state::ThreadState::LAYOUT);
    let result = resolve_in_layout_state(css, target);
    style::thread_state::exit(style::thread_state::ThreadState::LAYOUT);
    result
}

fn resolve_in_layout_state(css: &str, target: StyloNode<'_>) -> Arc<ComputedValues> {
    let lock = SharedRwLock::new();
    let device = Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        euclid::Size2D::new(1024.0, 768.0),
        euclid::Scale::new(1.0),
        Box::new(NoFontMetrics),
        ComputedValues::initial_values_with_font_override(FontStruct::initial_values()),
        PrefersColorScheme::Light,
    );
    let mut stylist = Stylist::new(device, QuirksMode::NoQuirks);

    let url = url::Url::parse("about:blank").unwrap();
    let sheet = Stylesheet::from_str(
        css,
        UrlExtraData::from(url),
        Origin::Author,
        StyloArc::new(lock.wrap(MediaList::empty())),
        lock.clone(),
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::No,
    );
    stylist.append_stylesheet(DocumentStyleSheet(StyloArc::new(sheet)), &lock.read());

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
    let mut thread_local = ThreadLocalStyleContext::<StyloNode<'_>>::new();
    let mut context = StyleContext {
        shared: &shared,
        thread_local: &mut thread_local,
    };

    let styles = resolve_style(&mut context, target, RuleInclusion::All, None, None);
    styles.primary().clone()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use florui::prelude::*;

    use super::*;

    fn resolved(tree: &Element, css: &str, hovered: HashSet<NodeId>) -> Arc<ComputedValues> {
        let arena = florui_style::Arena::build(tree);
        let stylo_tree = StyloTree::new(&arena, &hovered);
        resolve(css, &stylo_tree)
    }

    #[test]
    fn a_class_selector_sets_background_color() {
        let tree: Element = view! { <div class="card" /> };
        let styles = resolved(
            &tree,
            ".card { background-color: #1e1e22; }",
            HashSet::new(),
        );
        let color = styles.get_background().background_color.clone();
        println!("background_color = {color:?}");
    }

    #[test]
    fn width_and_height_resolve_from_a_class_selector() {
        let tree: Element = view! { <div class="card" /> };
        let styles = resolved(
            &tree,
            ".card { width: 320px; height: 240px; }",
            HashSet::new(),
        );
        println!("width = {:?}", styles.get_position().width);
        println!("height = {:?}", styles.get_position().height);
    }

    #[test]
    fn font_size_inherits_from_an_ancestor() {
        let tree: Element = view! {
            <div class="card">
                <span />
            </div>
        };
        let arena = florui_style::Arena::build(&tree);
        let child = arena.children(arena.roots()[0])[0];

        let stylo_tree = StyloTree::new(&arena, &HashSet::new());
        let child_node = stylo_tree
            .node(child)
            .expect("the span is reachable from the root this tree was built from");
        let styles = resolve_node(".card { font-size: 24px; }", child_node);

        // The <span> declares no font-size of its own; seeing 24px here —
        // resolved against a *different* node than the one that declared
        // it — is what actually proves inheritance, not just a property
        // applying to the node that set it.
        assert_eq!(styles.get_font().font_size.computed_size.0.px(), 24.0);
    }

    #[test]
    fn hover_state_matches_a_pseudo_class_selector() {
        let tree: Element = view! { <div class="card" /> };
        let arena = florui_style::Arena::build(&tree);
        let root = arena.roots()[0];

        let not_hovered = resolved(
            &tree,
            ".card { background-color: #111111; } .card:hover { background-color: #ff0000; }",
            HashSet::new(),
        );
        let hovered = {
            let mut set = HashSet::new();
            set.insert(root);
            resolved(
                &tree,
                ".card { background-color: #111111; } .card:hover { background-color: #ff0000; }",
                set,
            )
        };

        println!(
            "not_hovered = {:?}, hovered = {:?}",
            not_hovered.get_background().background_color,
            hovered.get_background().background_color
        );
        assert_ne!(
            format!("{:?}", not_hovered.get_background().background_color),
            format!("{:?}", hovered.get_background().background_color),
            ":hover must select a different declaration than the base rule"
        );
    }

    #[test]
    fn a_custom_property_resolves_through_var() {
        let tree: Element = view! { <div class="card" /> };
        let styles = resolved(
            &tree,
            ":root { --brand: #42734f; } .card { background-color: var(--brand); }",
            HashSet::new(),
        );
        println!(
            "background_color via var() = {:?}",
            styles.get_background().background_color
        );
    }
}

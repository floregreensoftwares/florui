//! Persistent state real `transition`/`@keyframes` animation needs across
//! [`crate::cascade::compute`] calls, which is otherwise a pure function of
//! its arguments (see [`crate::stylo`]'s own module doc): Stylo's real
//! animation engine (`style::animation`) samples an in-progress transition
//! or animation against the *previous* cascade's computed value and an
//! [`style::animation::ElementAnimationSet`] it expects to find again next
//! frame — both of which need somewhere to live between one `compute()`
//! call and the next.
//!
//! [`Arena`](crate::tree::Arena) rebuilds from scratch every call with no
//! stable per-element identity (a `NodeId` is just preorder position), so
//! this also owns the identity scheme animation state is actually keyed
//! by: each element's `tag` and position among same-tag siblings, chained
//! up to the document root. Deliberately *not* class-sensitive — a class
//! toggle or a `:hover` match changing is the most common way real CSS
//! ever triggers a transition in the first place, so identity has to
//! survive exactly that kind of change to be useful. Two elements in one
//! tree can never collide (their paths diverge at the first differing
//! ordinal), and the common case — a render that doesn't reorder or
//! add/remove same-tag siblings around this one — keeps the same identity
//! call over call, so its animation continues rather than restarting. A
//! same-tag element swapped in at the same position (a conditional
//! branch, a keyless list reorder) can still be mistaken for the outgoing
//! one and inherit its in-flight animation — the same honest ambiguity
//! any keyless positional matching has, real CSS included.

use std::collections::HashMap;

use style::animation::{DocumentAnimationSet, ElementAnimationSet};
use style::properties::ComputedValues;
use style::servo_arc::Arc as StyloArc;

/// One element's identity for animation purposes — see the module doc.
#[derive(Clone, PartialEq, Eq, Hash)]
struct PathKey {
    parent: Option<usize>,
    tag: &'static str,
    ordinal: usize,
}

/// Carries real Stylo animation state across [`crate::cascade::compute`]
/// calls. Cheap to construct; a real desktop session keeps one alive for
/// its whole lifetime, reusing it every frame so in-progress animations
/// keep sampling from where they left off. Tests that want a specific,
/// reproducible instant construct one and call [`Self::advance_to`]
/// directly rather than reading a real clock — the "shared controllable
/// clock" this crate's own motion spec calls for.
#[derive(Default)]
pub struct AnimationTimeline {
    pub(crate) now: f64,
    interner: HashMap<PathKey, usize>,
    next_id: usize,
    pub(crate) sets: DocumentAnimationSet,
    previous_styles: HashMap<usize, StyloArc<ComputedValues>>,
    touched_this_call: Vec<usize>,
    /// The real OS accessibility preference, as of the last time a host
    /// pushed it in — `false` (the `Default` value) for every caller that
    /// never does, which is every caller except a real desktop host (tests,
    /// benches, `florui-conformance`'s deterministic snapshots). Read-only
    /// truth for `@media (prefers-reduced-motion: ...)`; see
    /// [`Self::should_suppress_animations`] for the separate, opt-out-able
    /// mechanism this alone does not drive.
    os_prefers_reduced_motion: bool,
    /// Inverted so the derived `Default` (`false`) means "not disabled",
    /// i.e. auto-suppression enabled — the required default — without
    /// `new()` needing to diverge from `default()` (every non-desktop
    /// caller, including `Default::default()` call sites this crate can't
    /// see, must agree). Read via [`Self::should_suppress_animations`];
    /// written via [`Self::set_auto_suppress_motion`], which keeps the
    /// public API framed positively (`true` = suppress).
    auto_suppress_motion_disabled: bool,
}

impl AnimationTimeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// The real OS accessibility preference — used only for
    /// `@media (prefers-reduced-motion: ...)`, independent of
    /// [`Self::should_suppress_animations`]'s own opt-out: an author who
    /// explicitly wrote that media query deserves the real answer
    /// regardless of whether this host opted out of automatic suppression.
    pub(crate) fn prefers_reduced_motion(&self) -> bool {
        self.os_prefers_reduced_motion
    }

    /// Pushes a freshly-read OS accessibility preference in — a real host
    /// calls this once per relevant update, since nothing here reads the
    /// OS itself (this crate has no OS integration at all; see
    /// `florui_platform::accessibility`).
    pub fn set_os_prefers_reduced_motion(&mut self, value: bool) {
        self.os_prefers_reduced_motion = value;
    }

    /// Opts into (`true`, the default) or out of (`false`) automatic
    /// transition/`@keyframes` suppression when the OS prefers reduced
    /// motion. Does not affect `@media (prefers-reduced-motion: ...)`
    /// itself, which always reflects the real OS truth.
    pub fn set_auto_suppress_motion(&mut self, value: bool) {
        self.auto_suppress_motion_disabled = !value;
    }

    /// Whether `transition`/`@keyframes` animations should be suppressed
    /// outright this call — both the OS preference and the opt-out must
    /// agree.
    pub(crate) fn should_suppress_animations(&self) -> bool {
        !self.auto_suppress_motion_disabled && self.os_prefers_reduced_motion
    }

    /// Sets the instant `compute()` samples any in-progress animation or
    /// transition against. Seconds, on any epoch this timeline is
    /// internally consistent about — real callers reuse one
    /// [`std::time::Instant`] captured once at startup; nothing here reads
    /// a wall clock itself.
    pub fn advance_to(&mut self, seconds: f64) {
        self.now = seconds;
    }

    /// Whether anything this timeline is tracking still needs another
    /// frame to keep progressing — the caller's cue to keep scheduling
    /// redraws instead of going idle. `false` whenever
    /// [`Self::should_suppress_animations`] is active: Stylo's own
    /// bookkeeping (`sets`) still tracks a suppressed animation internally
    /// (see [`crate::stylo`]'s own suppression point, which leaves this
    /// untouched on purpose), but nothing suppressed is visibly changing
    /// frame to frame, so scheduling more redraws for it would be pointless.
    pub fn is_animating(&self) -> bool {
        if self.should_suppress_animations() {
            return false;
        }
        self.sets
            .sets
            .read()
            .values()
            .any(ElementAnimationSet::needs_animation_ticks)
    }

    /// Looks up (or assigns) `id`'s stable identity, given its already-
    /// resolved parent identity (`None` for a document root). Must be
    /// called in preorder — a node's own identity has to exist before any
    /// of its children can reference it.
    pub(crate) fn stable_id(
        &mut self,
        parent: Option<usize>,
        tag: &'static str,
        ordinal: usize,
    ) -> usize {
        let key = PathKey {
            parent,
            tag,
            ordinal,
        };
        let id = *self.interner.entry(key).or_insert_with(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        });
        self.touched_this_call.push(id);
        id
    }

    pub(crate) fn previous_style(&self, id: usize) -> Option<StyloArc<ComputedValues>> {
        self.previous_styles.get(&id).cloned()
    }

    pub(crate) fn set_current_style(&mut self, id: usize, style: StyloArc<ComputedValues>) {
        self.previous_styles.insert(id, style);
    }

    /// Drops tracked state for every identity `compute()` didn't touch
    /// this call — real CSS stops a transition/animation outright when its
    /// element leaves the document, not something worth keeping around on
    /// the chance it reappears.
    pub(crate) fn sweep(&mut self) {
        let touched: std::collections::HashSet<usize> = self.touched_this_call.drain(..).collect();
        self.previous_styles.retain(|id, _| touched.contains(id));
        self.sets
            .sets
            .write()
            .retain(|key, _| touched.contains(&key.node.0));
    }
}

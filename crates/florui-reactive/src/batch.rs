//! [`batch`]: coalesces every [`crate::Signal::set`] during one
//! synchronous unit of work (typically one event handler) into a single
//! downstream wake per affected [`DirtyFlag`], instead of one wake per
//! write.
//!
//! Without this, a handler that calls `set` twice wakes its host twice —
//! two full re-renders for one user gesture, the second of which can
//! render a partially-updated frame the app never actually intended
//! anyone to see (one signal updated, a sibling one not yet). `batch`
//! defers *waking the host*, not the writes themselves: `Signal::get`
//! inside `f` always reflects the latest `set`, synchronously, exactly as
//! outside a batch. Only "tell whoever is watching that something
//! changed" is deferred, until `f` finishes.

use std::cell::{Cell, RefCell};

use crate::DirtyFlag;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static PENDING: RefCell<Vec<DirtyFlag>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f`, deferring every [`DirtyFlag`] wake it would otherwise trigger
/// until `f` returns — each distinct flag wakes at most once, no matter
/// how many writes touched it during `f`. Batches nest: an inner `batch`
/// call just extends the outer one's deferral window; only the outermost
/// call's return flushes the deferred wakes.
pub fn batch<T>(f: impl FnOnce() -> T) -> T {
    DEPTH.with(|depth| depth.set(depth.get() + 1));
    let result = f();
    let now_outermost = DEPTH.with(|depth| {
        let next = depth.get() - 1;
        depth.set(next);
        next == 0
    });
    if now_outermost {
        let pending = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        for flag in pending {
            flag.fire_waker();
        }
    }
    result
}

/// Called from [`DirtyFlag::mark`]: while a [`batch`] is active, records
/// `flag` to wake once the outermost batch finishes instead of firing its
/// waker right away (deduplicated, so marking the same flag repeatedly
/// during one batch still only wakes it once); otherwise wakes it
/// immediately, same as if `batch` were never involved.
pub(crate) fn defer_or_fire(flag: &DirtyFlag) {
    let deferred = DEPTH.with(|depth| {
        if depth.get() == 0 {
            return false;
        }
        PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            if !pending.iter().any(|already| already.same_flag_as(flag)) {
                pending.push(flag.clone());
            }
        });
        true
    });
    if !deferred {
        flag.fire_waker();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell as StdCell;
    use std::rc::Rc;

    use super::*;
    use crate::ComponentScope;

    #[test]
    fn two_writes_in_one_batch_wake_the_host_exactly_once() {
        let (scope, dirty) = ComponentScope::new();
        let wakes = Rc::new(StdCell::new(0));
        let wakes_in_waker = Rc::clone(&wakes);
        dirty.on_mark(move || wakes_in_waker.set(wakes_in_waker.get() + 1));

        batch(|| {
            scope.render(|| crate::use_signal(|| 0).set(1));
            scope.render(|| crate::use_signal(|| 0).set(2));
        });

        assert_eq!(
            wakes.get(),
            1,
            "two writes in one batch must wake only once"
        );
    }

    #[test]
    fn a_write_outside_any_batch_still_wakes_immediately() {
        let (scope, dirty) = ComponentScope::new();
        let wakes = Rc::new(StdCell::new(0));
        let wakes_in_waker = Rc::clone(&wakes);
        dirty.on_mark(move || wakes_in_waker.set(wakes_in_waker.get() + 1));

        scope.render(|| crate::use_signal(|| 0).set(1));

        assert_eq!(
            wakes.get(),
            1,
            "unbatched writes are unaffected by batch existing"
        );
    }

    #[test]
    fn a_read_inside_a_batch_sees_the_latest_write_synchronously() {
        let (scope, _dirty) = ComponentScope::new();
        let value = batch(|| {
            scope.render(|| {
                let signal = crate::use_signal(|| 0);
                signal.set(5);
                signal.get()
            })
        });
        assert_eq!(
            value, 5,
            "batching defers the wake, not the write — reads stay synchronous"
        );
    }

    #[test]
    fn nested_batches_only_wake_when_the_outermost_one_finishes() {
        let (scope, dirty) = ComponentScope::new();
        let wakes = Rc::new(StdCell::new(0));
        let wakes_in_waker = Rc::clone(&wakes);
        dirty.on_mark(move || wakes_in_waker.set(wakes_in_waker.get() + 1));

        batch(|| {
            scope.render(|| crate::use_signal(|| 0).set(1));
            batch(|| {
                scope.render(|| crate::use_signal(|| 0).set(2));
            });
            assert_eq!(
                wakes.get(),
                0,
                "the inner batch returning must not wake yet"
            );
        });

        assert_eq!(wakes.get(), 1);
    }

    #[test]
    fn two_independent_dirty_flags_each_wake_once() {
        let (scope_a, dirty_a) = ComponentScope::new();
        let (scope_b, dirty_b) = ComponentScope::new();
        let wakes_a = Rc::new(StdCell::new(0));
        let wakes_b = Rc::new(StdCell::new(0));
        let (wa, wb) = (Rc::clone(&wakes_a), Rc::clone(&wakes_b));
        dirty_a.on_mark(move || wa.set(wa.get() + 1));
        dirty_b.on_mark(move || wb.set(wb.get() + 1));

        batch(|| {
            scope_a.render(|| crate::use_signal(|| 0).set(1));
            scope_a.render(|| crate::use_signal(|| 0).set(2));
            scope_b.render(|| crate::use_signal(|| 0).set(3));
        });

        assert_eq!(wakes_a.get(), 1);
        assert_eq!(wakes_b.get(), 1);
    }
}

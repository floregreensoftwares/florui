//! Cached derived values via [`use_memo`].

use crate::scope::active_slot;

/// A cached derived value: `compute(&deps)` only re-runs when `deps`
/// compares unequal to last render's. `deps` must capture everything
/// `compute` actually depends on — nothing here tracks that for you.
///
/// # Panics
///
/// Panics outside a [`Scope::render`](crate::Scope::render) pass, or if
/// hooks ran in a different order or count than last render.
pub fn use_memo<D, T>(deps: D, compute: impl FnOnce(&D) -> T) -> T
where
    D: PartialEq + 'static,
    T: Clone + 'static,
{
    let (scope, index) = active_slot("use_memo");
    let mut slots = scope.slots.borrow_mut();
    if index == slots.len() {
        let value = compute(&deps);
        slots.push(Box::new((deps, value.clone())));
        value
    } else {
        let (stored_deps, stored_value) =
            slots[index].downcast_mut::<(D, T)>().unwrap_or_else(|| {
                panic!(
                    "hook order changed between renders at call position {index} — \
                     hooks must run unconditionally, in the same order, every render"
                )
            });
        if *stored_deps != deps {
            *stored_value = compute(&deps);
            *stored_deps = deps;
        }
        stored_value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scope;

    #[test]
    fn a_memo_computes_from_its_deps() {
        let (scope, _dirty) = Scope::new();
        let value = scope.render(|| use_memo(3, |n| n * 2));
        assert_eq!(value, 6);
    }

    #[test]
    fn a_memo_does_not_recompute_when_deps_are_unchanged() {
        let (scope, _dirty) = Scope::new();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));

        let render = || {
            let calls = calls.clone();
            scope.render(move || {
                use_memo(5, move |n| {
                    calls.set(calls.get() + 1);
                    n * 2
                })
            })
        };

        assert_eq!(render(), 10);
        assert_eq!(render(), 10);
        assert_eq!(calls.get(), 1, "compute must run once for the same deps");
    }

    #[test]
    fn a_memo_recomputes_when_deps_change() {
        let (scope, _dirty) = Scope::new();
        assert_eq!(scope.render(|| use_memo(2, |n| n * 10)), 20);
        assert_eq!(scope.render(|| use_memo(3, |n| n * 10)), 30);
    }

    #[test]
    #[should_panic(expected = "hook order changed between renders")]
    fn a_memo_with_a_different_deps_type_at_the_same_position_panics() {
        let (scope, _dirty) = Scope::new();
        scope.render(|| {
            use_memo(1_i32, |n| n.to_string());
        });
        scope.render(|| {
            use_memo("not an i32", |s| s.to_string());
        });
    }

    #[test]
    #[should_panic(expected = "use_memo called outside of Scope::render")]
    fn use_memo_outside_a_render_panics() {
        use_memo(1, |n| *n);
    }
}

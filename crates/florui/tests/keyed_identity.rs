//! `key={...}` on a component call: its own identity, addressed by that
//! key instead of position among its siblings — see
//! `florui_reactive::use_child_scope_keyed`.

use std::cell::RefCell;
use std::rc::Rc;

use florui::prelude::*;

/// Logs its own mount/unmount to `log` — proof, not an assumption, that a
/// `key`-addressed component's state (here, whether its mount effect has
/// already run) survives across renders even when its position among
/// siblings changes, and is disposed once its key is no longer rendered
/// at all.
#[component]
fn LoggedItem(id: String, log: Rc<RefCell<Vec<String>>>) -> Element {
    use_effect((), {
        let log = Rc::clone(&log);
        let id = id.clone();
        move || {
            log.borrow_mut().push(format!("mount {id}"));
            Some(Box::new(move || log.borrow_mut().push(format!("unmount {id}"))) as Cleanup)
        }
    });
    view! { <div /> }
}

#[test]
fn key_preserves_state_across_reordering_and_disposes_removed_items() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (scope, _dirty) = Scope::new();

    scope.render(|| {
        view! {
            <div>
                <LoggedItem key="a" id={"a".to_string()} log={Rc::clone(&log)} />
                <LoggedItem key="b" id={"b".to_string()} log={Rc::clone(&log)} />
            </div>
        }
    });
    assert_eq!(*log.borrow(), vec!["mount a", "mount b"]);

    // Same two keys, reversed order — neither should remount.
    log.borrow_mut().clear();
    scope.render(|| {
        view! {
            <div>
                <LoggedItem key="b" id={"b".to_string()} log={Rc::clone(&log)} />
                <LoggedItem key="a" id={"a".to_string()} log={Rc::clone(&log)} />
            </div>
        }
    });
    assert!(
        log.borrow().is_empty(),
        "reordering must not remount either item, got {:?}",
        log.borrow()
    );

    // "a" is gone from this render entirely.
    scope.render(|| {
        view! {
            <div>
                <LoggedItem key="b" id={"b".to_string()} log={Rc::clone(&log)} />
            </div>
        }
    });
    assert_eq!(
        *log.borrow(),
        vec!["unmount a"],
        "a key no longer rendered must dispose its item"
    );
}

#[test]
fn a_key_can_be_a_non_string_displayable_value() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (scope, _dirty) = Scope::new();

    scope.render(|| {
        view! {
            <div>
                <LoggedItem key={42} id={"forty-two".to_string()} log={Rc::clone(&log)} />
            </div>
        }
    });
    scope.render(|| {
        view! {
            <div>
                <LoggedItem key={42} id={"forty-two".to_string()} log={Rc::clone(&log)} />
            </div>
        }
    });

    assert_eq!(
        *log.borrow(),
        vec!["mount forty-two"],
        "the same numeric key across renders must not remount the item"
    );
}

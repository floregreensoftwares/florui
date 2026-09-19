//! `error_boundary`/`use_error_boundary` through the real `#[component]`/
//! `view!` macro surface — see `florui_reactive::error_boundary`.

use std::cell::RefCell;
use std::rc::Rc;

use florui::prelude::*;

#[derive(Debug, Clone, PartialEq)]
struct DemoError(&'static str);

/// Renders normally, but reports a failure to whichever boundary is
/// nearest above it (looked up via [`use_context`]) when `fail` is set.
#[component]
fn Content(fail: bool, log: Rc<RefCell<Vec<String>>>) -> Element {
    if fail {
        let reporter = use_context::<ErrorReporter<DemoError>>()
            .expect("a boundary above this component provided a reporter");
        reporter.report(DemoError("boom"));
    }
    log.borrow_mut().push("content".to_string());
    view! { <div /> }
}

/// A boundary around [`Content`], composed the same way application code
/// would: [`error_boundary`] picks children vs. fallback for us.
#[component]
fn Boundary(fail: bool, log: Rc<RefCell<Vec<String>>>) -> Element {
    error_boundary::<DemoError, _>(
        {
            let log = Rc::clone(&log);
            move || view! { <Content fail={fail} log={log} /> }
        },
        move |error: DemoError, _boundary| {
            log.borrow_mut().push(format!("fallback: {}", error.0));
            view! { <div /> }
        },
    )
}

#[test]
fn a_reported_error_switches_the_tree_to_the_fallback_on_the_next_render() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (scope, _dirty) = ComponentScope::new();

    scope.render(|| view! { <Boundary fail={false} log={Rc::clone(&log)} /> });
    assert_eq!(*log.borrow(), vec!["content"]);

    // Content reports a failure this render, but a report only takes
    // effect starting the *next* render — children still render normally
    // here.
    log.borrow_mut().clear();
    scope.render(|| view! { <Boundary fail={true} log={Rc::clone(&log)} /> });
    assert_eq!(*log.borrow(), vec!["content"]);

    // Now the boundary sees the reported error and renders its fallback
    // instead, even though this render itself doesn't ask Content to fail.
    log.borrow_mut().clear();
    scope.render(|| view! { <Boundary fail={false} log={Rc::clone(&log)} /> });
    assert_eq!(*log.borrow(), vec!["fallback: boom"]);
}

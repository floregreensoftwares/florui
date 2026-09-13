//! Cross-feature interaction that no single isolated feature branch's own
//! tests exercise: a scoped slot's rendered items seeing context from
//! their logical ancestor while still cleaning up correctly, an
//! attachment living inside content a loading boundary reveals, and
//! binding writes participating in event batching the same way plain
//! `Signal::set` calls do.

use std::cell::RefCell;
use std::rc::Rc;

use florui::prelude::*;
use florui::reactive::executor::LocalExecutor;
use florui::reactive::testing::{ManualFuture, manual_future};

type Log = Rc<RefCell<Vec<String>>>;

// ---- Scoped slots: identity, context, and cleanup together ----

#[derive(Clone)]
struct Theme(&'static str);

#[component]
fn ThemedItem(id: String, log: Log) -> Element {
    let theme = use_context::<Theme>().expect("Provider above provided a Theme");
    use_effect((), {
        let log = Rc::clone(&log);
        let id = id.clone();
        move || {
            log.borrow_mut()
                .push(format!("mount {id} theme={}", theme.0));
            Some(Box::new(move || log.borrow_mut().push(format!("unmount {id}"))) as Cleanup)
        }
    });
    view! { <div /> }
}

type RenderThemedItem = Box<dyn Fn(&str, Log) -> Element>;

#[component]
fn ThemedList(items: Vec<String>, log: Log, render: RenderThemedItem) -> Element {
    let children: Vec<Element> = items
        .iter()
        .map(|item| (render)(item, log.clone()))
        .collect();
    view! { <div>{children}</div> }
}

#[component]
fn Provider(items: Vec<String>, log: Log) -> Element {
    provide_context(Theme("dark"));
    let render: RenderThemedItem = Box::new(|item: &str, log: Log| {
        let id = item.to_string();
        view! { <ThemedItem key={id.clone()} id={id} log={log} /> }
    });
    view! { <ThemedList items={items} log={log} render={render} /> }
}

#[test]
fn a_scoped_slots_items_see_the_ancestors_context_and_clean_up_when_removed() {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let (scope, _dirty) = Scope::new();

    scope.render({
        let log = Rc::clone(&log);
        move || {
            view! {
                <Provider items={vec!["a".to_string(), "b".to_string()]} log={log} />
            }
        }
    });
    assert_eq!(
        *log.borrow(),
        vec!["mount a theme=dark", "mount b theme=dark"],
        "an item rendered through a scoped slot must see context provided by its logical \
         ancestor (Provider), not just its lexical position in ThemedList's own source"
    );

    log.borrow_mut().clear();
    scope.render({
        let log = Rc::clone(&log);
        move || view! { <Provider items={vec!["a".to_string()]} log={log} /> }
    });
    assert_eq!(
        *log.borrow(),
        vec!["unmount b"],
        "removing an item from the scoped slot's list must dispose its own identity"
    );
}

// ---- An attachment living inside loading_boundary content ----

#[component]
fn Watched(log: Log) -> Element {
    use_attachment((), (), {
        let log = Rc::clone(&log);
        move |_handle| {
            log.borrow_mut().push("attach".to_string());
            let log = Rc::clone(&log);
            Some(Box::new(move || log.borrow_mut().push("detach".to_string())) as Cleanup)
        }
    });
    view! { <div /> }
}

type DemoFuture = ManualFuture<Result<i32, String>>;

fn render_watched_boundary(
    executor: Rc<LocalExecutor>,
    fetch: Option<DemoFuture>,
    log: Log,
) -> impl FnOnce() -> ResourceHandle<i32, String> {
    move || {
        provide_context(Rc::clone(&executor) as Rc<dyn Executor>);
        let resource = use_resource("key", move |_| {
            fetch.expect("the fetch only runs when the key/retry actually changed")
        });
        loading_boundary(
            &[&resource],
            {
                let log = Rc::clone(&log);
                move |_refreshing| view! { <Watched log={log} /> }
            },
            || view! { <div /> },
        );
        resource
    }
}

#[test]
fn an_attachment_inside_loading_boundary_content_sets_up_once_revealed_and_survives_a_refresh() {
    let executor = Rc::new(LocalExecutor::new());
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let (scope, _dirty) = Scope::new();

    let (first, first_resolver) = manual_future::<Result<i32, String>>();
    scope.render(render_watched_boundary(
        Rc::clone(&executor),
        Some(first),
        Rc::clone(&log),
    ));
    executor.run_until_stalled();
    assert!(
        log.borrow().is_empty(),
        "the attachment must not set up while its content is still hidden behind the fallback"
    );

    first_resolver.resolve(Ok(1));
    executor.run_until_stalled();

    log.borrow_mut().clear();
    let handle = scope.render(render_watched_boundary(
        Rc::clone(&executor),
        None,
        Rc::clone(&log),
    ));
    assert_eq!(
        *log.borrow(),
        vec!["attach"],
        "revealing content must set up its attachment exactly once"
    );

    // A refresh (retry) keeps content mounted at the same generation — the
    // attachment must not tear down and re-attach just because the
    // boundary is showing is_refreshing again.
    handle.retry();
    let (second, second_resolver) = manual_future::<Result<i32, String>>();
    log.borrow_mut().clear();
    let handle = scope.render(render_watched_boundary(
        Rc::clone(&executor),
        Some(second),
        Rc::clone(&log),
    ));
    assert!(
        log.borrow().is_empty(),
        "a refresh must keep content (and its attachment) mounted, not tear down and reattach"
    );

    second_resolver.resolve(Ok(2));
    executor.run_until_stalled();
    log.borrow_mut().clear();
    scope.render(render_watched_boundary(executor, None, log.clone()));
    assert!(
        log.borrow().is_empty(),
        "settling again after the refresh must not re-attach either"
    );
    let _ = handle;
}

// ---- Binding writes participate in event batching ----

#[test]
fn two_binding_writes_in_one_batch_wake_the_host_exactly_once() {
    let (owner, dirty) = Scope::new();
    let (a, b) = owner.render(|| (use_signal(|| 0), use_signal(|| 0)));

    let wakes = Rc::new(RefCell::new(0));
    let wakes_in_waker = Rc::clone(&wakes);
    dirty.on_mark(move || *wakes_in_waker.borrow_mut() += 1);

    let a_binding = a.binding();
    let b_binding = b.binding();
    batch(|| {
        a_binding.request_update(1);
        b_binding.request_update(2);
    });

    assert_eq!(
        *wakes.borrow(),
        1,
        "two binding writes inside one batch must wake the host once, not twice — Binding's \
         default adapter writes through Signal::set as-is, so it inherits batch's coalescing"
    );
    assert_eq!(a.get(), 1);
    assert_eq!(b.get(), 2);
}

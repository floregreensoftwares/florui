use florui::prelude::*;
use florui_reactive::{use_memo, use_signal};

stylesheet!("./counter.css");

/// `should_increment` stands in for a real click handler, which `view!`
/// doesn't support yet — the host hit-tests the button externally and
/// passes `true` for one render (see `examples/counter.rs`).
#[component]
pub fn Counter(should_increment: bool) -> Element {
    let count = use_signal(|| 0);
    if should_increment {
        count.set(count.get() + 1);
    }
    let doubled = use_memo(count.get(), |n| n * 2);

    view! {
        <div class="counter">
            <span class="count">{count.get().to_string()}</span>
            <span class="doubled">{format!("x2 = {doubled}")}</span>
            <button class="increment">{"+1"}</button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use florui_reactive::Scope;
    use florui_style::Arena;

    use super::*;

    fn text_of_class(arena: &Arena, class: &str) -> String {
        let node = arena
            .find(|a, id| a.classes(id).iter().any(|c| c == class))
            .unwrap_or_else(|| panic!("no node with class {class:?}"));
        arena.text_content(node).to_string()
    }

    fn render(scope: &Scope, should_increment: bool) -> Arena {
        let tree = scope.render(|| Counter(CounterProps { should_increment }));
        Arena::build(&tree)
    }

    fn rendered_count(scope: &Scope, should_increment: bool) -> String {
        text_of_class(&render(scope, should_increment), "count")
    }

    #[test]
    fn starts_at_zero_and_survives_a_render_with_no_click() {
        let (scope, _dirty) = Scope::new();
        assert_eq!(rendered_count(&scope, false), "0");
        assert_eq!(
            rendered_count(&scope, false),
            "0",
            "use_signal's initializer must not re-run and reset the count on a later render"
        );
    }

    #[test]
    fn a_click_increments_and_the_next_non_click_render_keeps_it() {
        let (scope, _dirty) = Scope::new();
        assert_eq!(rendered_count(&scope, false), "0");
        assert_eq!(rendered_count(&scope, true), "1");
        assert_eq!(
            rendered_count(&scope, false),
            "1",
            "the count must persist without another click"
        );
        assert_eq!(rendered_count(&scope, true), "2");
    }

    #[test]
    fn a_click_marks_the_scope_dirty() {
        let (scope, dirty) = Scope::new();
        rendered_count(&scope, false);
        assert!(
            !dirty.get(),
            "a render with no click has nothing to notify a host about"
        );
        rendered_count(&scope, true);
        assert!(dirty.get());
    }

    #[test]
    fn the_doubled_memo_tracks_the_count() {
        let (scope, _dirty) = Scope::new();
        assert_eq!(text_of_class(&render(&scope, false), "doubled"), "x2 = 0");
        assert_eq!(text_of_class(&render(&scope, true), "doubled"), "x2 = 2");
        assert_eq!(text_of_class(&render(&scope, true), "doubled"), "x2 = 4");
    }
}

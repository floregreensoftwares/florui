//! First performance baseline for style: cascade cost as a tree scales in
//! depth and in width — the harness [`layout`'s own baseline] set the
//! convention for (build the tree once outside the timed closure, only
//! [`compute`] itself measured, representative "100, 1,000"-scale
//! workloads, no pass/fail budget). This is the direct successor to the
//! ad-hoc timing that first caught the cascade's near-quadratic cost in
//! deep trees — a permanent regression check for that fix now that one
//! exists, not a one-off measurement.
//!
//! [`layout`'s own baseline]: https://github.com/floregreensoftwares/florui/blob/grow/main/crates/florui-layout/benches/layout.rs

use criterion::{Criterion, criterion_group, criterion_main};
use florui::Element;
use florui_style::{AnimationTimeline, InteractionState, Viewport, compute, parse_stylesheet};

fn deep_tree(depth: usize) -> Element {
    let mut node = Element::node("div", vec![("class".into(), "leaf".into())], Vec::new());
    for _ in 0..depth {
        node = Element::node("div", Vec::new(), vec![node]);
    }
    node
}

fn wide_tree(n: usize) -> Element {
    let children = (0..n)
        .map(|_| Element::node("div", vec![("class".into(), "item".into())], Vec::new()))
        .collect();
    Element::node("div", Vec::new(), children)
}

/// Depth alone isn't the whole cascade story: a selector with real
/// specificity (a class, not just the bare-element rule every other group
/// here uses) forces actual selector matching per node, not just
/// inheritance walking. `.leaf`'s own single-class selector is deliberately
/// modest — this measures ordinary matching cost, not a pathological
/// selector.
fn deep_tree_with_matching(depth: usize) -> Element {
    let mut node = Element::node("div", vec![("class".into(), "leaf".into())], Vec::new());
    for i in 0..depth {
        node = Element::node(
            "div",
            vec![("class".into(), format!("ancestor-{}", i % 5))],
            vec![node],
        );
    }
    node
}

/// 1,500: comfortably past the depth (~1,500) that first exposed the
/// cascade's near-quadratic cost via ad-hoc timing (~1.9s there before the
/// fix, ~43ms after) — kept here as the permanent regression check for
/// that specific fix, not picked freshly for this benchmark.
fn bench_deep_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("style/deep_tree");
    group.sample_size(20);
    for &depth in &[500usize, 1500] {
        let tree = deep_tree(depth);
        let arena = florui_style::Arena::build(&tree);
        let rules =
            parse_stylesheet(".leaf { width: 10px; }").expect("benchmark CSS must be valid");
        group.bench_function(format!("{depth}_deep"), |b| {
            b.iter(|| {
                compute(
                    &arena,
                    &rules,
                    &InteractionState::new(),
                    Viewport::default(),
                    &mut AnimationTimeline::default(),
                )
            });
        });
    }
    group.finish();
}

fn bench_deep_tree_with_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("style/deep_tree_with_matching");
    group.sample_size(20);
    for &depth in &[500usize, 1500] {
        let tree = deep_tree_with_matching(depth);
        let arena = florui_style::Arena::build(&tree);
        let css = "
            .leaf { width: 10px; }
            .ancestor-0 { color: #111; }
            .ancestor-1 { color: #222; }
            .ancestor-2 { color: #333; }
            .ancestor-3 { color: #444; }
            .ancestor-4 { color: #555; }
        ";
        let rules = parse_stylesheet(css).expect("benchmark CSS must be valid");
        group.bench_function(format!("{depth}_deep"), |b| {
            b.iter(|| {
                compute(
                    &arena,
                    &rules,
                    &InteractionState::new(),
                    Viewport::default(),
                    &mut AnimationTimeline::default(),
                )
            });
        });
    }
    group.finish();
}

fn bench_wide_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("style/wide_tree");
    group.sample_size(20);
    for &n in &[100usize, 1000] {
        let tree = wide_tree(n);
        let arena = florui_style::Arena::build(&tree);
        let rules =
            parse_stylesheet(".item { width: 10px; }").expect("benchmark CSS must be valid");
        group.bench_function(format!("{n}_children"), |b| {
            b.iter(|| {
                compute(
                    &arena,
                    &rules,
                    &InteractionState::new(),
                    Viewport::default(),
                    &mut AnimationTimeline::default(),
                )
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_deep_tree,
    bench_deep_tree_with_matching,
    bench_wide_tree,
);
criterion_main!(benches);

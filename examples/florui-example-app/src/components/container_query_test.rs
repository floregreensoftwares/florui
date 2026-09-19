use florui::prelude::*;

stylesheet!("./container_query_test.css");

#[component]
pub fn ContainerQueryTest() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"The outer box's width follows the window -- resize wider than \
                  roughly 500px to see it turn green. The inner box sits inside \
                  it but declares its own fixed 300px width as a container, so \
                  its own @container condition always queries *that* width, \
                  never the window's or its outer ancestor's -- it stays gray no \
                  matter how wide you resize."}
            </span>
            <div class="outer">
                <div class="card">{"outer container (follows the window)"}</div>
                <div class="inner">
                    <div class="card">{"inner container (fixed 300px)"}</div>
                </div>
            </div>
        </div>
    }
}

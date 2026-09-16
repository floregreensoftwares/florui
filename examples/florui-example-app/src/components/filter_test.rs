use florui::prelude::*;

stylesheet!("./filter_test.css");

#[component]
pub fn FilterTest() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"Every box starts from the same purple fill (#8040c0). All ten run through \
                  the exact same real CSS `filter` property this project cascades and paints \
                  -- nothing here is a special case."}
            </span>
            <div class="grid">
                <div class="sample">
                    <div class="stage"><div class="box none"></div></div>
                    <span class="label">{"none (baseline)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box blur"></div></div>
                    <span class="label">{"blur(4px)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box dim"></div></div>
                    <span class="label">{"brightness(0.5)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box bright"></div></div>
                    <span class="label">{"brightness(1.8)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box low-contrast"></div></div>
                    <span class="label">{"contrast(0.3)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box high-contrast"></div></div>
                    <span class="label">{"contrast(2)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box desaturated"></div></div>
                    <span class="label">{"saturate(0)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box oversaturated"></div></div>
                    <span class="label">{"saturate(2.5)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box chained"></div></div>
                    <span class="label">{"brightness(1.5) contrast(1.3)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box grayscale"></div></div>
                    <span class="label">{"grayscale(1) -- unsupported, stays identity"}</span>
                </div>
            </div>
        </div>
    }
}

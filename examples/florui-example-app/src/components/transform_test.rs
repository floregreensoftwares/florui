use florui::prelude::*;

stylesheet!("./transform_test.css");

#[component]
pub fn TransformTest() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"Every box is 60x60 with a red L-shaped marker on its own top-left corner, \
                  so a rotation's direction and pivot are visible at a glance. All eleven run \
                  through the exact same real CSS `transform` property this project cascades \
                  and paints -- nothing here is a special case."}
            </span>
            <div class="grid">
                <div class="sample">
                    <div class="stage"><div class="box none"></div></div>
                    <span class="label">{"none (baseline)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box translate"></div></div>
                    <span class="label">{"translate(20px, 20px)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box translate-pct"></div></div>
                    <span class="label">{"translate(50%, 0)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box scale-up"></div></div>
                    <span class="label">{"scale(1.5)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box scale-x"></div></div>
                    <span class="label">{"scaleX(1.8)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box rotate"></div></div>
                    <span class="label">{"rotate(45deg)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box matrix"></div></div>
                    <span class="label">{"matrix(1, 0, 0, 1, 20, -20)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box translate-then-rotate"></div></div>
                    <span class="label">{"translate(30px,0) rotate(45deg)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box rotate-then-translate"></div></div>
                    <span class="label">{"rotate(45deg) translate(30px,0)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box faded-and-moved"></div></div>
                    <span class="label">{"opacity: 0.5 + translate(20px,20px)"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box skewed"></div></div>
                    <span class="label">{"skewX(30deg) -- unsupported, stays identity"}</span>
                </div>
            </div>
        </div>
    }
}

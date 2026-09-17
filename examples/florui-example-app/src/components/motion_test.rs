use florui::prelude::*;

stylesheet!("./motion_test.css");

#[component]
pub fn MotionTest() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"All motion below runs through this project's own real CSS transition/ \
                  @keyframes engine (Stylo's, unmodified). The top row only moves on hover \
                  -- a real property change is what starts a transition. The bottom row runs \
                  continuously from the moment this window opens, with no interaction needed."}
            </span>

            <span class="section-title">{"transition -- hover to trigger"}</span>
            <div class="grid">
                <div class="sample">
                    <div class="stage"><div class="box t-fast"></div></div>
                    <span class="label">{"background-color + scale, ease 0.35s"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box t-slow"></div></div>
                    <span class="label">{"background-color, linear 1.6s"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box t-ease-in"></div></div>
                    <span class="label">{"translateX, ease-in 0.6s"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box t-ease-out"></div></div>
                    <span class="label">{"opacity, ease-out 0.6s"}</span>
                </div>
            </div>

            <span class="section-title">{"@keyframes -- continuous"}</span>
            <div class="grid">
                <div class="sample">
                    <div class="stage"><div class="box k-pulse"></div></div>
                    <span class="label">{"opacity pulse, 2s infinite"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box k-slide"></div></div>
                    <span class="label">{"translateX, 1.4s alternate"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box k-spin"></div></div>
                    <span class="label">{"rotate, 3s linear infinite"}</span>
                </div>
                <div class="sample">
                    <div class="stage"><div class="box k-color"></div></div>
                    <span class="label">{"3-stop color cycle, 4s infinite"}</span>
                </div>
            </div>
        </div>
    }
}

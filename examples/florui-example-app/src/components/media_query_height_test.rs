use florui::prelude::*;

stylesheet!("./media_query_height_test.css");

#[component]
pub fn MediaQueryHeightTest() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"Each box is gray by default and turns green once its own @media \
                  height condition matches the real window height -- resize the \
                  window taller/shorter to cross each breakpoint live, no click \
                  needed."}
            </span>
            <div class="row">
                <div class="box min-height-box">{"min-height: 500px"}</div>
                <div class="box max-height-box">{"max-height: 900px"}</div>
                <div class="box both-box">{"min-height: 500px and max-height: 900px"}</div>
            </div>
        </div>
    }
}

use florui::prelude::*;

stylesheet!("./dpi_ruler.css");

#[component]
pub fn DpiRuler() -> Element {
    view! {
        <div class="page">
            <span class="instructions">
                {"Boxes are 50/100/150/200 logical px, labeled with their own size. \
                  Resize this window, change Windows' display scaling (Settings > \
                  Display > Scale), or drag it to a different-DPI monitor. The boxes \
                  should keep looking the same physical size on screen the whole time, \
                  not shrink, grow, or blur, and should update the instant the scale \
                  changes, with no click needed."}
            </span>
            <div class="row">
                <div class="box box-50">{"50px"}</div>
                <div class="box box-100">{"100px"}</div>
                <div class="box box-150">{"150px"}</div>
                <div class="box box-200">{"200px"}</div>
            </div>
        </div>
    }
}

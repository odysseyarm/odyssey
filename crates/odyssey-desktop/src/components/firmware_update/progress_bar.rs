use dioxus::prelude::*;

#[component]
pub(super) fn ProgressBar(percent: usize) -> Element {
    rsx! {
        div {
            class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
            div {
                class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                style: "width: {percent}%",
            }
        }
    }
}

use dioxus::prelude::*;

#[component]
pub fn Home() -> Element {
    let mut selected = use_signal(|| None);

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 dark:bg-gray-800",

            div {
                class: "w-full max-w-md flex flex-col items-stretch gap-4",

                h1 {
                    class: "text-center text-2xl font-bold text-gray-900 dark:text-white",
                    "Welcome!"
                }

                form {
                    class: "flex flex-col text-gray-500 dark:text-gray-400 gap-4",

                    label {
                        class: "text-base font-medium",
                        "Choose a display:"
                    }

                    div {
                        class: "flex flex-row items-center gap-2",

                        select {
                            class: "bg-gray-50 border border-gray-300 text-gray-900 text-base rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500",
                            onchange: move |evt| {
                                let value = evt.value().clone();
                                let sel = match value.as_str() {
                                    "Display1" => Some(DisplaySelection::Display1),
                                    "Display2" => Some(DisplaySelection::Display2),
                                    "Display3" => Some(DisplaySelection::Display3),
                                    _ => None,
                                };
                                selected.set(sel);
                            },
                            option { value: "", disabled: true, selected: selected().is_none(), "Select an option" }
                            option { value: "Display1", "Option 1" }
                            option { value: "Display2", "Option 2" }
                            option { value: "Display3", "Option 3" }
                        }

                        button {
                            class: "w-12 shrink-0 text-white bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-base p-2.5 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800",
                            r#type: "button",
                            onclick: move |_| {
                                if let Some(sel) = selected() {
                                    println!("Selected display: {:?}", sel);
                                } else {
                                    println!("No display selected.");
                                }
                            },
                            "Go"
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplaySelection {
    Display1,
    Display2,
    Display3,
}

impl std::fmt::Display for DisplaySelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisplaySelection::Display1 => write!(f, "Option 1"),
            DisplaySelection::Display2 => write!(f, "Option 2"),
            DisplaySelection::Display3 => write!(f, "Option 3"),
        }
    }
}

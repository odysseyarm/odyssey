use iui::UI;
use iui::controls::{Window, WindowType};
use leptos_reactive::{RwSignal, SignalGet, create_effect};

fn main() {
    let ui = UI::init().expect("Couldn't initialize UI library");
    let _ui_ctx = ui.async_context();
    let _leptos_rt = leptos_reactive::create_runtime();
    let mut main_win = Window::new(&ui, "waow speenbox", 640, 480, WindowType::NoMenubar);
    let spinbox_value = RwSignal::new(0);
    let spinbox2_value = RwSignal::new(0);
    odyssey_vision_tool::layout! { &ui,
        let vbox = VerticalBox(padded: true) {
            Compact: let spinbox = Spinbox(signal: spinbox_value)
            Compact: let spinbox2 = Spinbox(signal: spinbox2_value)
            Compact: let separator = HorizontalSeparator()
        }
    }
    create_effect(move |_| {
        println!("spinbox2 value is {}", spinbox2_value.get());
    });
    main_win.set_child(&ui, vbox);
    main_win.show(&ui);
    ui.main();
}

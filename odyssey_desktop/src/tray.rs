use std::sync::Arc;

use dioxus::{
    desktop::{
        tao::event::Event,
        trayicon::{
            menu::{Menu, MenuItem},
            Icon, TrayIconBuilder,
        },
        use_tray_menu_event_handler, use_wry_event_handler, WindowEvent,
    },
    prelude::*,
};

macro_rules! icon_from_bytes {
    ($rel_path:literal) => {{
        let _bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $rel_path));
        let image = image::load_from_memory(_bytes)
            .expect(concat!("Failed to load image from: ", $rel_path))
            .into_rgba8();
        let (icon_width, icon_height) = image.dimensions();
        let icon_rgba = image.into_raw();
        Icon::from_rgba(icon_rgba, icon_width, icon_height)
            .expect(concat!("Failed to build icon from: ", $rel_path))
    }};
}

/// Initialize the tray icon and its handlers for the app
///
/// **Needs to be called after the app is launched**
///
/// I have found that a reliable and simple way is to us just the main App component as the entry point
pub fn init() {
    // Loading the image we are going to use as the tray icon
    let icon = icon_from_bytes!("/assets/images/tray-icon.png");

    // Tray Menu
    // I have found that for compatibility its best to use the tray menu for actions instead of the tray icon click itself
    // I had problems with the tray icon click event not firing on ubuntu
    let menu = Menu::new();

    // We use "Open" as default text because we start the window in a hidden state
    let toggle_item = MenuItem::with_id("toggle", "Close", true, None);
    let quit_item = MenuItem::with_id("quit", "Quit", true, None);

    // Append items to the menu
    let _ = menu.append_items(&[&toggle_item, &quit_item]).unwrap();

    // Could also be made into a static variable using something like once_cell
    // For this example its only used internaly and therefor not needed
    // Gives use the ability to access the toggle_item from other parts of the code and change the text
    let toggle_arc = Arc::new(toggle_item);

    // Building the tray icon itself
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(icon);

    // Providing the context to the dioxus app
    // Basically we just did the exact same thing as the default `init_tray_icon` function would do
    // https://github.com/DioxusLabs/dioxus/blob/a729968ee47b066e6d55dd9e0f8c2a1f1aef79e0/packages/desktop/src/trayicon.rs#L31
    // We do it manually to have more control over the tray icon
    provide_context(builder.build().expect("tray icon builder failed"));

    // Using the event handler to listen for events from the tray menu
    // For our special toggle item we change the text and show/hide the window
    {
        let toggle_arc_clone = toggle_arc.clone();
        use_tray_menu_event_handler(move |event| {
            // Potentially there is a better way to do this.
            // The `0` is the id of the menu item
            match event.id.0.as_str() {
                "quit" => {
                    std::process::exit(0);
                }
                "toggle" => {
                    // According to the examples the correct way to access the context of the app
                    // https://github.com/DioxusLabs/dioxus/blob/a729968ee47b066e6d55dd9e0f8c2a1f1aef79e0/examples/window_zoom.rs#L23
                    let service = dioxus::desktop::window();
                    let window = &service.window;

                    let is_visible = window.is_visible();

                    // Setting the opposite of the current state because we toggle in the next line
                    toggle_arc_clone.set_text(match is_visible {
                        true => "Open",
                        false => "Close",
                    });
                    window.set_visible(!is_visible);
                }
                _ => {}
            }
        });
    }

    // User Experience upgrade
    // Since a user can also close the window, we can hook into the window events to properly update the tray icon text
    {
        let toggle_arc_clone = toggle_arc.clone();
        use_wry_event_handler(move |event, _| {
            if let Event::WindowEvent {
                window_id: _,
                event,
                ..
            } = event
            {
                // Additionall events could be handled here to improve experience when minimizing for example
                match event {
                    WindowEvent::CloseRequested => {
                        toggle_arc_clone.set_text("Open");

                        // Fixing the close behaviour to hide the window fully
                        // By default the app will only hide the webview and keep the window open
                        // Potentially this is something dioxus itself could improve
                        let service = dioxus::desktop::window();
                        let window = &service.window;
                        window.set_visible(false);
                    }
                    _ => {}
                }
            }
        });
    }
}

use iced::{Element, Theme};

use iced::{
    widget::{column, text},
    Task,
};

#[derive(Default)]
struct Odyssey {}

fn main() -> iced::Result {
    iced::application("Odyssey", update, view)
        .window_size(iced::Size::new(800.0, 600.0))
        .decorations(true)
        .theme(|_state: &Odyssey| system_theme_mode())
        .run_with(|| (Odyssey::default(), Task::none()))
}

fn update(_state: &mut Odyssey, _message: Message) -> Task<Message> {
    Task::none()
}

fn view(_state: &Odyssey) -> Element<Message> {
    column![text("Welcome!")]
        .spacing(20)
        .padding(20)
        .into()
}

#[derive(Debug, Clone)]
enum Message {}

fn system_theme_mode() -> Theme {
    match dark_light::detect() {
        Ok(dark_light::Mode::Light) => Theme::Light,
        _ => Theme::Dark,
    }
}

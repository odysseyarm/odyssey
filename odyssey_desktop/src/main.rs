use iced::{widget::{column, text}, Element, Task};

#[derive(Default)]
struct Odyssey {}

#[derive(Debug, Clone)]
enum Message {}

fn main() -> iced::Result {
    iced::application("Odyssey", update, view)
        .window_size(iced::Size::new(800.0, 600.0))
        .decorations(true)
        .run_with(|| (Odyssey::default(), Task::none()))
}

fn update(_state: &mut Odyssey, _message: Message) -> Task<Message> {
    Task::none()
}

fn view(_state: &Odyssey) -> Element<Message> {
    column![text("Welcome!")].spacing(20).padding(20).into()
}

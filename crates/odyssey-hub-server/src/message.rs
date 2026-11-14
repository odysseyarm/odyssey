/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Debug)]
pub enum Message {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    ServerInit(Result<Vec<odyssey_hub_common::config::ConfigCorruptionEvent>, std::io::Error>),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    BringToFront,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    Stop,
}

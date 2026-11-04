pub enum Message {
    ServerInit(Result<Vec<odyssey_hub_common::config::ConfigCorruptionEvent>, std::io::Error>),
    BringToFront,
    Stop,
}

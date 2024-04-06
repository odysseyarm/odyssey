use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
enum Request {
    Version,
    GetDevices,
}

#[derive(Serialize, Deserialize)]
enum Response {
    Version(u32),
    GetDevices(Vec<DeviceId>),
}

#[derive(Serialize, Deserialize)]
struct DeviceId(u32);

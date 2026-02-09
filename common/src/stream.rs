//! Stream info management for data streams (IMU, markers, etc.)

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use protodongers::PacketType;

/// Manages enabled/disabled state and request ID for a single data stream.
#[derive(Default)]
pub struct StreamInfo {
    req_id: AtomicU8,
    enable: AtomicBool,
}

impl StreamInfo {
    pub const fn new() -> Self {
        Self {
            req_id: AtomicU8::new(0),
            enable: AtomicBool::new(false),
        }
    }

    /// Check if the stream is enabled
    pub fn enabled(&self) -> bool {
        self.enable.load(Ordering::Relaxed)
    }

    /// Get the request ID for this stream
    pub fn req_id(&self) -> u8 {
        self.req_id.load(Ordering::Relaxed)
    }

    /// Set the request ID
    pub fn set_req_id(&self, req_id: u8) {
        self.req_id.store(req_id, Ordering::Relaxed);
    }

    /// Enable or disable the stream
    pub fn set_enable(&self, enable: bool) {
        self.enable.store(enable, Ordering::Relaxed);
    }
}

/// Collection of stream infos for common data types
#[derive(Default)]
pub struct StreamInfos {
    pub imu: StreamInfo,
    pub impact: StreamInfo,
    pub marker: StreamInfo,
}

impl StreamInfos {
    pub const fn new() -> Self {
        Self {
            imu: StreamInfo::new(),
            impact: StreamInfo::new(),
            marker: StreamInfo::new(),
        }
    }

    /// Disable all streams
    pub fn disable_all(&self) {
        self.imu.enable.store(false, Ordering::Relaxed);
        self.impact.enable.store(false, Ordering::Relaxed);
        self.marker.enable.store(false, Ordering::Relaxed);
    }

    /// Enable a stream by packet type
    pub fn enable(&self, req_id: u8, ty: PacketType) {
        defmt::info!(
            "Enable stream {:?}, id = {=u8}",
            defmt::Debug2Format(&ty),
            req_id
        );
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::PocMarkersReport() => &self.marker,
            _ => return,
        };
        si.set_req_id(req_id);
        si.set_enable(true);
    }

    /// Disable a stream by packet type
    pub fn disable(&self, ty: PacketType) {
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::PocMarkersReport() => &self.marker,
            _ => return,
        };
        si.set_enable(false);
    }
}

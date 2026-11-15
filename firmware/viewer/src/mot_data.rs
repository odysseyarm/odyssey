/// Since: Odyssey Desktop 0.6.0.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotData {
    /// Since: Odyssey Desktop 0.6.0.
    pub area: u16,
    /// Since: Odyssey Desktop 0.6.0.
    pub cx: u16,
    /// Since: Odyssey Desktop 0.6.0.
    pub cy: u16,
    /// Since: Odyssey Desktop 0.6.0.
    pub avg_brightness: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub max_brightness: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub range: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub radius: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub boundary_left: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub boundary_right: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub boundary_up: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub boundary_down: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub aspect_ratio: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub vx: u8,
    /// Since: Odyssey Desktop 0.6.0.
    pub vy: u8,
}

impl MotData {
    /// Since: Odyssey Desktop 0.6.0.
    pub fn parse(bytes: &mut &[u8]) -> Self {
        let mot_data = MotData {
            area: bytes[0] as u16 | ((bytes[1] as u16) << 8),
            cx: bytes[2] as u16 | ((bytes[3] & 0x0f) as u16) << 8,
            cy: bytes[4] as u16 | ((bytes[5] & 0x0f) as u16) << 8,
            avg_brightness: bytes[6],
            max_brightness: bytes[7],
            radius: bytes[8] & 0x0f,
            range: bytes[8] >> 4,
            boundary_left: bytes[9] & 0x7f,
            boundary_right: bytes[10] & 0x7f,
            boundary_up: bytes[11] & 0x7f,
            boundary_down: bytes[12] & 0x7f,
            aspect_ratio: bytes[13],
            vx: bytes[14],
            vy: bytes[15],
        };
        *bytes = &bytes[16..];
        mot_data
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&[
            self.area as u8,
            (self.area >> 8) as u8,
            self.cx as u8,
            (self.cx >> 8) as u8,
            self.cy as u8,
            (self.cy >> 8) as u8,
            self.avg_brightness,
            self.max_brightness,
            self.radius | self.range << 4,
            self.boundary_left,
            self.boundary_right,
            self.boundary_up,
            self.boundary_down,
            self.aspect_ratio,
            self.vx,
            self.vy,
        ]);
    }
}

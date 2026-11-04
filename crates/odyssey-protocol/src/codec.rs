use core::mem::MaybeUninit;
#[cfg(feature = "std")]
use std::vec::Vec;

use nalgebra::Point2;

use crate::{CombinedMarkersReport, Error, MotData, ObjectReport, PacketType};

pub trait Parse: Sized {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error>;
}

pub trait Serialize {
    const SIZE: usize;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]);

    #[cfg(feature = "std")]
    #[expect(
        unsafe_code,
        reason = "set_len over the spare capacity that serialize just filled"
    )]
    fn serialize_to_vec(&self, buf: &mut Vec<u8>) {
        buf.reserve(Self::SIZE);
        let () = self.serialize(&mut buf.spare_capacity_mut());
        unsafe {
            buf.set_len(buf.len() + Self::SIZE);
        }
    }
}

#[expect(
    unsafe_code,
    reason = "raw copy into an uninitialised wire buffer, bounds asserted above"
)]
#[inline]
pub(crate) fn push(buf: &mut &mut [MaybeUninit<u8>], data: &[u8]) {
    assert!(buf.len() >= data.len());
    let ptr = <[_]>::as_mut_ptr(buf).cast::<u8>();
    unsafe {
        ptr.copy_from_nonoverlapping(data.as_ptr(), data.len());
    }
    *buf = &mut core::mem::take(buf)[data.len()..];
}

impl Parse for MotData {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        let mot_data = Self {
            area: u16::from(bytes[0]) | (u16::from(bytes[1]) << 8),
            cx: u16::from(bytes[2]) | u16::from(bytes[3] & 0x0f) << 8,
            cy: u16::from(bytes[4]) | u16::from(bytes[5] & 0x0f) << 8,
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
        Ok(mot_data)
    }
}

impl Serialize for MotData {
    const SIZE: usize = 16;
    #[expect(unsafe_code, reason = "raw copy into an uninitialised wire buffer")]
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        let data = [
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
        ];
        let ptr = <[_]>::as_mut_ptr(buf).cast::<u8>();
        unsafe {
            ptr.copy_from_nonoverlapping(data.as_ptr(), data.len());
        }
    }
}

impl Parse for ObjectReport {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        use Error as E;
        let timestamp = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        *bytes = &bytes[4..];
        let data = &mut &bytes[..512];
        *bytes = &bytes[512..];
        let [_format, _, ..] = **bytes else {
            return Err(E::UnexpectedEof {
                packet_type: Some(PacketType::ObjectReport()),
            });
        };
        *bytes = &bytes[2..];
        Ok(Self {
            timestamp,
            mot_data_nf: [(); 16].map(|()| MotData::parse(data).expect("MotData parse error")),
            mot_data_wf: [(); 16].map(|()| MotData::parse(data).expect("MotData parse error")),
        })
    }
}

impl Serialize for ObjectReport {
    const SIZE: usize = 518;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        push(buf, &self.timestamp.to_le_bytes());
        for i in 0..16 {
            self.mot_data_nf[i].serialize(buf);
        }
        for i in 0..16 {
            self.mot_data_wf[i].serialize(buf);
        }
        push(buf, &[1, 0]);
    }
}

impl Parse for CombinedMarkersReport {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        use Error as E;
        let size = Self::SIZE;
        if bytes.len() < size {
            return Err(E::UnexpectedEof {
                packet_type: Some(PacketType::CombinedMarkersReport()),
            });
        }

        let data = &mut &bytes[..size];
        *bytes = &bytes[size..];

        let mut positions = [Point2::new(0, 0); 16 * 2];
        for i in 0..positions.len() {
            // x, y is 12 bits each
            let x = u16::from_le_bytes([data[0], data[1] & 0x0f]);
            let y = u16::from(data[1] >> 4) | (u16::from(data[2]) << 4);
            positions[i] = Point2::new(x, y);
            *data = &data[3..];
        }
        let nf_positions = positions[..16].try_into().unwrap();
        let wf_positions = positions[16..].try_into().unwrap();

        Ok(Self {
            nf_points: nf_positions,
            wf_points: wf_positions,
        })
    }
}

impl Serialize for CombinedMarkersReport {
    const SIZE: usize = 96;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        for p in self.nf_points.iter().chain(&self.wf_points) {
            let (x, y) = (p.x, p.y);
            let byte0 = x & 0xff;
            let byte1 = ((x >> 8) & 0x0f) | ((y & 0x0f) << 4);
            let byte2 = y >> 4;
            push(buf, &[byte0 as u8, byte1 as u8, byte2 as u8]);
        }
    }
}

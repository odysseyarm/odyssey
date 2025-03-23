use thiserror::Error;

pub const SLIP_FRAME_END: u8 = 0xc0;
const SLIP_FRAME_ESC: u8 = 0xdb;
const SLIP_FRAME_ESC_END: u8 = 0xdc;
const SLIP_FRAME_ESC_ESC: u8 = 0xdd;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Error, Debug)]
pub enum SlipDecodeError {
    #[error("double esc")]
    DoubleEsc,
    #[error("invalid esc")]
    InvalidEsc,
    #[error("trailing esc")]
    TrailingEsc,
}

/// Decode some data from a SLIP frame. This function does not handle SLIP_FRAME_END bytes.
pub fn decode_slip_frame(buf: &mut [u8]) -> Result<usize, SlipDecodeError> {
    let mut j = 0;
    let mut esc = false;
    for i in 0..buf.len() {
        match buf[i] {
            self::SLIP_FRAME_ESC => {
                if esc {
                    return Err(SlipDecodeError::DoubleEsc);
                }
                esc = true;
            }
            self::SLIP_FRAME_ESC_END if esc => {
                buf[j] = SLIP_FRAME_END;
                j += 1;
                esc = false;
            }
            self::SLIP_FRAME_ESC_ESC if esc => {
                buf[j] = SLIP_FRAME_ESC;
                j += 1;
                esc = false;
            }
            x => {
                if esc {
                    return Err(SlipDecodeError::InvalidEsc);
                }
                buf[j] = x;
                j += 1;
            }
        }
    }
    if esc {
        return Err(SlipDecodeError::TrailingEsc);
    }
    Ok(j)
}

#[derive(Error, Debug)]
pub enum SlipEncodeError {
    #[error("not enough capacity")]
    NotEnoughCapacity,
}

/// Encode some data into a SLIP frame. This does not write a header.
pub fn encode_slip_frame<'a>(data: &[u8], out: &'a mut [u8]) -> Result<&'a [u8], SlipEncodeError> {
    let mut i = 0;
    for &byte in data {
        match byte {
            SLIP_FRAME_END => {
                *out.get_mut(i).ok_or(SlipEncodeError::NotEnoughCapacity)? = SLIP_FRAME_ESC;
                *out.get_mut(i + 1)
                    .ok_or(SlipEncodeError::NotEnoughCapacity)? = SLIP_FRAME_ESC_END;
                i += 2;
            }
            SLIP_FRAME_ESC => {
                *out.get_mut(i).ok_or(SlipEncodeError::NotEnoughCapacity)? = SLIP_FRAME_ESC;
                *out.get_mut(i + 1)
                    .ok_or(SlipEncodeError::NotEnoughCapacity)? = SLIP_FRAME_ESC_ESC;
                i += 2;
            }
            x => {
                *out.get_mut(i).ok_or(SlipEncodeError::NotEnoughCapacity)? = x;
                i += 1;
            }
        }
    }
    *out.get_mut(i).ok_or(SlipEncodeError::NotEnoughCapacity)? = SLIP_FRAME_END;
    Ok(&out[..i + 1])
}

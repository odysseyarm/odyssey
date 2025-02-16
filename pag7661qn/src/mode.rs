use core::convert::Infallible;

mod sealed {
    pub trait Sealed {}
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    Idle,
    Image,
    Object,
}

#[derive(Debug, Copy, Clone)]
pub struct ModeError {
    pub expected: Mode,
    pub actual: Mode,
}

#[derive(Copy, Clone)]
pub struct Idle;
#[derive(Copy, Clone)]
pub struct Image;
#[derive(Copy, Clone)]
pub struct Object;

pub trait ModeT: sealed::Sealed + Clone + Copy {
    fn mode(self) -> Mode;
}
pub trait IsIdle: ModeT + Clone + Copy {
    type Error: ModeErrorT;
    fn is_idle(self) -> Result<(), Self::Error>;
}
pub trait IsImage: ModeT + Clone + Copy {
    type Error: ModeErrorT;
    fn is_image(self) -> Result<(), Self::Error>;
}
pub trait IsObject: ModeT + Clone + Copy {
    type Error: ModeErrorT;
    fn is_object(self) -> Result<(), Self::Error>;
}

pub trait ModeErrorT: sealed::Sealed {}
impl sealed::Sealed for Infallible {}
impl ModeErrorT for Infallible {}
impl sealed::Sealed for ModeError {}
impl ModeErrorT for ModeError {}

macro_rules! impl_mode {
    ($modename:ident $tr:ident $fn_name:ident) => {
        impl sealed::Sealed for $modename {}
        impl $tr for $modename {
            type Error = Infallible;
            fn $fn_name(self) -> Result<(), Self::Error> {
                Ok(())
            }
        }
        impl $tr for Mode {
            type Error = ModeError;
            fn $fn_name(self) -> Result<(), Self::Error> {
                if self == Mode::$modename {
                    Ok(())
                } else {
                    Err(ModeError {
                        expected: Mode::$modename,
                        actual: self,
                    })
                }
            }
        }
        impl ModeT for $modename {
            fn mode(self) -> Mode {
                Mode::$modename
            }
        }
    };
}

impl sealed::Sealed for Mode {}
impl ModeT for Mode {
    fn mode(self) -> Mode {
        self
    }
}

impl_mode!(Idle IsIdle is_idle);
impl_mode!(Image IsImage is_image);
impl_mode!(Object IsObject is_object);

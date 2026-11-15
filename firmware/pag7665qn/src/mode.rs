use core::convert::Infallible;

mod sealed {
    /// Since: Odyssey Desktop 0.6.0.
    pub trait Sealed {}
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug, Copy, Clone, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
pub enum Mode {
    /// Since: Odyssey Desktop 0.6.0.
    Idle,
    /// Since: Odyssey Desktop 0.6.0.
    Image,
    /// Since: Odyssey Desktop 0.6.0.
    Object,
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug, Copy, Clone)]
pub struct ModeError {
    /// Since: Odyssey Desktop 0.6.0.
    pub expected: Mode,
    /// Since: Odyssey Desktop 0.6.0.
    pub actual: Mode,
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Copy, Clone)]
pub struct Idle;
/// Since: Odyssey Desktop 0.6.0.
#[derive(Copy, Clone)]
pub struct Image;
/// Since: Odyssey Desktop 0.6.0.
#[derive(Copy, Clone)]
pub struct Object;

/// Since: Odyssey Desktop 0.6.0.
pub trait ModeT: sealed::Sealed + Clone + Copy {
    /// Since: Odyssey Desktop 0.6.0.
    fn mode(self) -> Mode;
}
/// Since: Odyssey Desktop 0.6.0.
pub trait IsIdle: ModeT + Clone + Copy {
    /// Since: Odyssey Desktop 0.6.0.
    type Error: ModeErrorT;
    /// Since: Odyssey Desktop 0.6.0.
    fn is_idle(self) -> Result<(), Self::Error>;
}
/// Since: Odyssey Desktop 0.6.0.
pub trait IsImage: ModeT + Clone + Copy {
    /// Since: Odyssey Desktop 0.6.0.
    type Error: ModeErrorT;
    /// Since: Odyssey Desktop 0.6.0.
    fn is_image(self) -> Result<(), Self::Error>;
}
/// Since: Odyssey Desktop 0.6.0.
pub trait IsObject: ModeT + Clone + Copy {
    /// Since: Odyssey Desktop 0.6.0.
    type Error: ModeErrorT;
    /// Since: Odyssey Desktop 0.6.0.
    fn is_object(self) -> Result<(), Self::Error>;
}

/// Since: Odyssey Desktop 0.6.0.
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

use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not, Shl, Shr};
use crypto_bigint::{Encoding as _, U2048};

#[repr(transparent)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct U2048x(pub U2048);

impl U2048x {
    pub const ZERO: Self = Self(U2048::ZERO);
    pub const ONE:  Self = Self(U2048::ONE);

    pub fn to_le_bytes(self) -> [u8; 256] { self.0.to_le_bytes() }
    pub fn from_le_bytes(b: [u8; 256]) -> Self { Self(U2048::from_le_bytes(b)) }
}

impl From<U2048> for U2048x { fn from(v: U2048) -> Self { Self(v) } }
impl From<U2048x> for U2048 { fn from(v: U2048x) -> Self { v.0 } }

impl BitAnd for U2048x { type Output = Self; #[inline] fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) } }
impl BitOr  for U2048x { type Output = Self; #[inline] fn bitor (self, rhs: Self) -> Self { Self(self.0 | rhs.0) } }
impl BitXor for U2048x { type Output = Self; #[inline] fn bitxor(self, rhs: Self) -> Self { Self(self.0 ^ rhs.0) } }
impl Not    for U2048x { type Output = Self; #[inline] fn not(self) -> Self { Self(!self.0) } }

impl BitAndAssign for U2048x { #[inline] fn bitand_assign(&mut self, rhs: Self) { self.0 &= rhs.0; } }
impl BitOrAssign  for U2048x { #[inline] fn bitor_assign (&mut self, rhs: Self) { self.0 |= rhs.0; } }
impl BitXorAssign for U2048x { #[inline] fn bitxor_assign(&mut self, rhs: Self) { self.0 ^= rhs.0; } }

impl Shr<usize> for U2048x { type Output = Self; fn shr(self, rhs: usize) -> Self { Self(self.0.shr_vartime(rhs as u32)) } }
impl Shl<usize> for U2048x { type Output = Self; fn shl(self, rhs: usize) -> Self { Self(self.0.shl_vartime(rhs as u32)) } }

impl device_driver::ops::TruncateToU8 for U2048x {
    #[inline]
    fn truncate(self) -> u8 {
        self.0.to_le_bytes()[0]
    }
    
    #[inline]
    fn detruncate(x: u8) -> Self {
        let mut b = [0u8; 256];
        b[0] = x;
        U2048x::from_le_bytes(b)
    }
}

impl device_driver::ops::Integer for U2048x {
    type DedupType = u128;

    const SIGN_EXTEND_ONES: Self = Self::ZERO;
    const ONE: Self = Self::ONE;

    #[inline]
    fn cast_deduplicate(self) -> Self::DedupType {
        let bytes = self.to_le_bytes();
        let mut lo128 = [0u8; 16];
        lo128.copy_from_slice(&bytes[..16]);
        u128::from_le_bytes(lo128)
    }

    #[inline]
    fn cast_deduplicate_back(val: Self::DedupType) -> Self {
        let mut bytes = [0u8; 256];
        bytes[..16].copy_from_slice(&val.to_le_bytes());
        U2048x::from_le_bytes(bytes)
    }
}

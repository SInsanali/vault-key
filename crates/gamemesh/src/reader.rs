//! A bounds-checked little-endian cursor over an object's native data.

use crate::error::{MeshError, Result};

pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(MeshError::Truncated(format!("{n} bytes at offset {}", self.pos)));
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub(crate) fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub(crate) fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    /// An array count, which must leave room for `count * min_element` bytes.
    pub(crate) fn count(&mut self, min_element: usize, what: &str) -> Result<usize> {
        let n = self.i32()?;
        let n = usize::try_from(n).map_err(|_| MeshError::Unsupported(format!("{what}: count {n}")))?;
        if n.checked_mul(min_element.max(1)).is_none_or(|bytes| bytes > self.remaining()) {
            return Err(MeshError::Truncated(format!("{what}: count {n}")));
        }
        Ok(n)
    }
}

/// An IEEE 754 half-precision float, as the engine stores texture coordinates.
pub(crate) fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exponent = i32::from((h >> 10) & 0x1F);
    let mantissa = f32::from(h & 0x3FF);
    let magnitude = match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => {
            if mantissa == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    };
    sign * magnitude
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0x3555), 0.333_251_95);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(half_to_f32(0x7C00), f32::INFINITY);
        assert!(half_to_f32(0x7E00).is_nan());
    }

    #[test]
    fn reads_are_bounds_checked() {
        let mut c = Cursor::new(&[1, 0, 0, 0, 2, 0]);
        assert_eq!(c.i32().unwrap(), 1);
        assert_eq!(c.u16().unwrap(), 2);
        assert!(c.u8().is_err());
        // A count that the remaining bytes cannot hold, and a negative one.
        let mut c = Cursor::new(&[5, 0, 0, 0, 0, 0, 0, 0]);
        assert!(c.count(2, "x").is_err());
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(matches!(c.count(1, "x"), Err(MeshError::Unsupported(_))));
        let mut c = Cursor::new(&[2, 0, 0, 0, 9, 9]);
        assert_eq!(c.count(1, "x").unwrap(), 2);
    }
}

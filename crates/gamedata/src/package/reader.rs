use crate::error::{GameDataError, Result};

/// Longest FString accepted, in characters. Real ones are far shorter.
const MAX_FSTRING: usize = 1_048_576;

/// Bounds-checked little-endian reader over a byte slice.
pub(crate) struct ByteReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            end: bytes.len(),
        }
    }

    /// A reader over `bytes[offset..end]`. An `end` past the slice is clamped; an `offset`
    /// past `end` makes every read fail.
    pub(crate) fn with_range(bytes: &'a [u8], offset: usize, end: usize) -> Self {
        Self {
            bytes,
            pos: offset,
            end: end.min(bytes.len()),
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos == self.end
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos > self.end || n > self.end - self.pos {
            return Err(GameDataError::Truncated(format!(
                "{n} bytes at offset {}",
                self.pos
            )));
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
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

    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub(crate) fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    /// A u32 used as a size or offset.
    pub(crate) fn u32_size(&mut self) -> Result<usize> {
        let v = self.u32()?;
        usize::try_from(v).map_err(|_| GameDataError::UnexpectedData(format!("size {v}")))
    }

    /// An i32 used as a count, size or offset: negative values are rejected.
    pub(crate) fn i32_size(&mut self, what: &str) -> Result<usize> {
        let v = self.i32()?;
        usize::try_from(v).map_err(|_| GameDataError::UnexpectedData(format!("{what} {v}")))
    }

    /// An FString: i32 length including the NUL; a negative length means UTF-16LE.
    /// Single-byte strings are read as Latin-1.
    pub(crate) fn fstring(&mut self) -> Result<String> {
        let len = self.i32()?;
        if len == 0 {
            return Ok(String::new());
        }
        let n = len.unsigned_abs() as usize;
        if n > MAX_FSTRING {
            return Err(GameDataError::UnexpectedData(format!(
                "FString length {len}"
            )));
        }
        if len > 0 {
            let s = self.take(n)?;
            let body = s.strip_suffix(&[0]).unwrap_or(s);
            return Ok(body.iter().map(|&b| char::from(b)).collect());
        }
        let s = self.take(n * 2)?;
        let mut units: Vec<u16> = s
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        if units.last() == Some(&0) {
            units.pop();
        }
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_scalars() {
        let bytes = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut r = ByteReader::new(&bytes);
        assert_eq!(r.u16().unwrap(), 0x0201);
        assert_eq!(r.u32().unwrap(), 0x0605_0403);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u64().unwrap(), 0x0F0E_0D0C_0B0A_0908);
        assert!(r.at_end());
        let mut f = ByteReader::new(&[0, 0, 0x20, 0x41]);
        assert_eq!(f.f32().unwrap(), 10.0);
        let mut i = ByteReader::new(&[0xFE, 0xFF, 0xFF, 0xFF]);
        assert_eq!(i.i32().unwrap(), -2);
        let mut neg = ByteReader::new(&[0xFE, 0xFF, 0xFF, 0xFF]);
        assert!(matches!(
            neg.i32_size("count"),
            Err(GameDataError::UnexpectedData(_))
        ));
    }

    #[test]
    fn reads_fstrings() {
        let mut a = vec![7u8, 0, 0, 0];
        a.extend_from_slice(b"Vladof\0");
        assert_eq!(ByteReader::new(&a).fstring().unwrap(), "Vladof");

        let mut u = vec![0xFBu8, 0xFF, 0xFF, 0xFF];
        for c in "Zer0\0".encode_utf16() {
            u.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(ByteReader::new(&u).fstring().unwrap(), "Zer0");

        assert_eq!(ByteReader::new(&[0, 0, 0, 0]).fstring().unwrap(), "");
        assert_eq!(ByteReader::new(&[1, 0, 0, 0, 0]).fstring().unwrap(), "");
        // Latin-1, and no terminator.
        assert_eq!(
            ByteReader::new(&[2, 0, 0, 0, 0xE9, b'!'])
                .fstring()
                .unwrap(),
            "\u{e9}!"
        );
    }

    #[test]
    fn truncated_reads_fail() {
        assert!(matches!(
            ByteReader::new(&[1, 2, 3]).u32(),
            Err(GameDataError::Truncated(_))
        ));
        assert!(ByteReader::new(&[50, 0, 0, 0, 65, 66, 67])
            .fstring()
            .is_err());
        assert!(ByteReader::with_range(&[1, 2, 3, 4, 5, 6], 0, 2)
            .u32()
            .is_err());
        // A huge length is rejected before any allocation.
        assert!(matches!(
            ByteReader::new(&[0xFF, 0xFF, 0xFF, 0x7F]).fstring(),
            Err(GameDataError::UnexpectedData(_))
        ));
        assert!(ByteReader::new(&[0, 0, 0, 0x80]).fstring().is_err());
        // An offset past the end fails instead of panicking.
        let mut past = ByteReader::with_range(&[1, 2, 3], 9, 3);
        assert!(past.u8().is_err());
        assert_eq!(past.remaining(), 0);
        let mut s = ByteReader::new(&[1, 2, 3]);
        assert!(s.skip(4).is_err());
        assert_eq!(s.position(), 0);
    }
}

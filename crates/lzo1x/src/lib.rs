//! LZO1X: a standard, bounds-checked decompressor (every instruction form) and a
//! literal-run-only compressor (one literal run, then the end-of-stream marker).
//!
//! Used for save files (`docs/format/bl2-save-format.md` section 2) and for the
//! game's package files (`docs/research/2026-11-08-game-data-spike.md` section 2).

use std::fmt;

/// Why a stream could not be decompressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LzoError(pub &'static str);

impl fmt::Display for LzoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "corrupt LZO1X stream: {}", self.0)
    }
}

impl std::error::Error for LzoError {}

/// Decoder state between instructions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// The next byte below 16 starts a literal run.
    ExpectLiteralRun,
    /// After 1 to 3 literals: a byte below 16 is a 2-byte M1 match.
    AfterShortLiterals,
    /// After a literal run of 4 or more: a byte below 16 is a 3-byte M1 match.
    AfterLongLiterals,
}

struct Decoder<'a> {
    src: &'a [u8],
    ip: usize,
    out: Vec<u8>,
    expected: usize,
}

impl Decoder<'_> {
    fn byte(&mut self) -> Result<usize, LzoError> {
        let b = *self.src.get(self.ip).ok_or(LzoError("input overrun"))?;
        self.ip += 1;
        Ok(b as usize)
    }

    /// Zero-run length extension: k zero bytes, then a non-zero byte b, give k*255 + b + base.
    fn extended(&mut self, base: usize) -> Result<usize, LzoError> {
        let mut n = 0usize;
        loop {
            let b = self.byte()?;
            if b != 0 {
                return Ok(n + b + base);
            }
            n = n.checked_add(255).ok_or(LzoError("length overflow"))?;
            if n > self.expected.saturating_add(255) {
                return Err(LzoError("length exceeds expected output"));
            }
        }
    }

    fn literals(&mut self, count: usize) -> Result<(), LzoError> {
        if count > self.src.len() - self.ip {
            return Err(LzoError("literal run overruns input"));
        }
        if count > self.expected - self.out.len() {
            return Err(LzoError("literal run overruns output"));
        }
        self.out.extend_from_slice(&self.src[self.ip..self.ip + count]);
        self.ip += count;
        Ok(())
    }

    fn copy_match(&mut self, distance: usize, length: usize) -> Result<(), LzoError> {
        if distance < 1 || distance > self.out.len() {
            return Err(LzoError("match distance out of range"));
        }
        if length > self.expected - self.out.len() {
            return Err(LzoError("match overruns output"));
        }
        // Matches may overlap their own output, so copy byte by byte.
        let start = self.out.len() - distance;
        for from in start..start + length {
            let b = self.out[from];
            self.out.push(b);
        }
        Ok(())
    }
}

/// Largest output buffer reserved up front; a larger `expected_len` still works, it just grows.
const MAX_RESERVE: usize = 1 << 26;

/// Decompress a raw LZO1X stream.
///
/// Fails on any input or output overrun, a bad match distance, a missing end-of-stream
/// marker, bytes after the marker, or an output length different from `expected_len`.
pub fn decompress(input: &[u8], expected_len: usize) -> Result<Vec<u8>, LzoError> {
    let mut d = Decoder {
        src: input,
        ip: 0,
        out: Vec::with_capacity(expected_len.min(MAX_RESERVE)),
        expected: expected_len,
    };
    let mut state = State::ExpectLiteralRun;
    let mut first = true;
    loop {
        let t = d.byte()?;
        if first {
            first = false;
            if t > 17 {
                let n = t - 17;
                d.literals(n)?;
                state = if n < 4 { State::AfterShortLiterals } else { State::AfterLongLiterals };
                continue;
            }
        }
        let trailing = if t >= 64 {
            // M2
            let next = d.byte()?;
            d.copy_match(1 + ((t >> 2) & 7) + (next << 3), (t >> 5) + 1)?;
            t & 3
        } else if t >= 32 {
            // M3
            let len = if t & 31 == 0 { d.extended(31 + 2)? } else { (t & 31) + 2 };
            let dd = d.byte()? | (d.byte()? << 8);
            d.copy_match(1 + (dd >> 2), len)?;
            dd & 3
        } else if t >= 16 {
            // M4, or end of stream
            let len = if t & 7 == 0 { d.extended(7 + 2)? } else { (t & 7) + 2 };
            let dd = d.byte()? | (d.byte()? << 8);
            let dist = ((t & 8) << 11) + (dd >> 2);
            if dist == 0 {
                if d.ip != d.src.len() {
                    return Err(LzoError("trailing bytes after end of stream"));
                }
                if d.out.len() != d.expected {
                    return Err(LzoError("output length differs from expected"));
                }
                return Ok(d.out);
            }
            d.copy_match(0x4000 + dist, len)?;
            dd & 3
        } else if state == State::ExpectLiteralRun {
            let n = if t == 0 { d.extended(15 + 3)? } else { t + 3 };
            d.literals(n)?;
            state = State::AfterLongLiterals;
            continue;
        } else {
            // M1
            let next = d.byte()?;
            if state == State::AfterLongLiterals {
                d.copy_match(1 + 0x0800 + (t >> 2) + (next << 2), 3)?;
            } else {
                d.copy_match(1 + (t >> 2) + (next << 2), 2)?;
            }
            t & 3
        };
        if trailing > 0 {
            d.literals(trailing)?;
            state = State::AfterShortLiterals;
        } else {
            state = State::ExpectLiteralRun;
        }
    }
}

/// Compress as one literal run followed by the `11 00 00` end-of-stream marker.
///
/// The output is slightly larger than the input. The game accepts it (proven in game).
pub fn compress_literals(input: &[u8]) -> Vec<u8> {
    let len = input.len();
    let mut out = Vec::with_capacity(len + len / 255 + 8);
    if len > 0 {
        if len <= 238 {
            out.push((17 + len) as u8);
        } else {
            out.push(0);
            let mut tt = len - 18;
            while tt > 255 {
                out.push(0);
                tt -= 255;
            }
            // tt is now 1..=255: the remainder byte must be non-zero.
            out.push(tt as u8);
        }
        out.extend_from_slice(input);
    }
    out.extend_from_slice(&[0x11, 0x00, 0x00]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SplitMix64, so tests are repeatable without a dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
    }

    #[test]
    fn literal_round_trip() {
        for n in [0, 1, 3, 4, 17, 18, 238, 239, 255, 256, 270, 273, 3 * 255 + 3, 3 * 255 + 18, 65_536, 200_000] {
            let data = Rng(n as u64 + 1).bytes(n);
            let packed = compress_literals(&data);
            assert_eq!(decompress(&packed, n).unwrap(), data, "length {n}");
        }
    }

    #[test]
    fn extension_remainder_is_never_zero() {
        // Lengths where (len - 18) is a multiple of 255 are where a naive encoder writes a zero.
        for k in 1..6usize {
            let n = 18 + 255 * k;
            let data = Rng(n as u64).bytes(n);
            let packed = compress_literals(&data);
            assert_eq!(packed[0], 0);
            assert_eq!(packed[k], 255, "length {n}");
            assert_eq!(decompress(&packed, n).unwrap(), data);
        }
    }

    #[test]
    fn rejects_truncated_stream() {
        let packed = compress_literals(&Rng(7).bytes(500));
        assert!(decompress(&packed[..packed.len() - 3], 500).is_err());
        assert!(decompress(&packed[..100], 500).is_err());
    }

    #[test]
    fn rejects_wrong_length_and_trailing_bytes() {
        let data = Rng(3).bytes(100);
        let mut packed = compress_literals(&data);
        assert!(decompress(&packed, 99).is_err());
        assert!(decompress(&packed, 101).is_err());
        packed.push(0);
        assert_eq!(decompress(&packed, 100), Err(LzoError("trailing bytes after end of stream")));
    }

    #[test]
    fn garbage_never_panics_or_hangs() {
        let mut rng = Rng(42);
        for _ in 0..20_000 {
            let n = (rng.next() % 65) as usize;
            let junk = rng.bytes(n);
            let expected = (rng.next() % 201) as usize;
            let _ = decompress(&junk, expected);
        }
        // A long run of zero extension bytes must stop instead of counting forever.
        assert!(decompress(&vec![0u8; 1 << 20], 10).is_err());
    }

    #[test]
    fn decodes_m2_matches() {
        // literal "abcd", M2 match length 4 distance 4, end of stream
        let s = [21, 0x61, 0x62, 0x63, 0x64, 0x6C, 0x00, 0x11, 0x00, 0x00];
        assert_eq!(decompress(&s, 8).unwrap(), b"abcdabcd");
        // overlapping: literal "a", M2 length 5 distance 1
        let o = [18, 0x61, 0x80, 0x00, 0x11, 0x00, 0x00];
        assert_eq!(decompress(&o, 6).unwrap(), b"aaaaaa");
        // match distance beyond the output so far
        let bad = [18, 0x61, 0x80, 0x08, 0x11, 0x00, 0x00];
        assert_eq!(decompress(&bad, 6), Err(LzoError("match distance out of range")));
    }

    #[test]
    fn decodes_m1_m3_and_trailing_literals() {
        // literal "abcdef" (t = 17 + 6), then M1 after a long run: 3 bytes at distance 0x801 is
        // out of range, so it must be rejected rather than read as a literal run.
        let m1_long = [23, b'a', b'b', b'c', b'd', b'e', b'f', 0x00, 0x00, 0x11, 0x00, 0x00];
        assert_eq!(decompress(&m1_long, 9), Err(LzoError("match distance out of range")));
        // literal "ab" (short), then M1 2-byte match at distance 2 with 1 trailing literal "c".
        // t = (dist-1)<<2 | trailing = (1<<2)|1 = 5, next = 0.
        let m1_short = [19, b'a', b'b', 0x05, 0x00, b'c', 0x11, 0x00, 0x00];
        assert_eq!(decompress(&m1_short, 5).unwrap(), b"ababc");
        // literal "abcd", M3 length 3 (t = 32 | 1) distance 4 (d = 3 << 2), 2 trailing literals.
        let m3 = [21, b'a', b'b', b'c', b'd', 33, (3 << 2) | 2, 0x00, b'x', b'y', 0x11, 0x00, 0x00];
        assert_eq!(decompress(&m3, 9).unwrap(), b"abcdabcxy");
        // literal "abcd", M3 with extended length: t = 32, one extension byte 1 -> 1 + 31 + 2 = 34.
        let m3_long = [21, b'a', b'b', b'c', b'd', 32, 1, 3 << 2, 0x00, 0x11, 0x00, 0x00];
        let out = decompress(&m3_long, 38).unwrap();
        assert_eq!(out.len(), 38);
        assert!(out.chunks(4).all(|c| b"abcd".starts_with(c)));
    }
}

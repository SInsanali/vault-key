//! A PNG writer: 8-bit RGBA, a filter chosen per row, compressed with [`crate::deflate`].
//! Written from the PNG specification (RFC 2083).

use crate::deflate;

/// Encodes `rgba` (`width * height * 4` bytes, rows top to bottom, straight alpha) as a PNG
/// file.
///
/// # Panics
/// If `rgba` is not exactly `width * height * 4` bytes long or a dimension is zero. These are
/// the caller's own numbers, never data from a file.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert!(width > 0 && height > 0, "empty image");
    let row = width as usize * 4;
    assert_eq!(rgba.len(), row * height as usize, "pixel buffer size");

    let z = deflate::zlib(&filter(rgba, row));
    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend([0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut header = Vec::with_capacity(13);
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]); // 8 bits, colour type 6 (RGBA), deflate, adaptive filters, no interlace
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Bytes per pixel, the distance the filters look back along a row.
const BPP: usize = 4;

/// The Paeth predictor: whichever of left, above, upper-left is nearest to left + above −
/// upper-left.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Applies filter `kind` (0 none, 1 sub, 2 up, 3 average, 4 Paeth) to one row.
fn filter_row(kind: u8, line: &[u8], above: &[u8], out: &mut Vec<u8>) {
    for (i, &x) in line.iter().enumerate() {
        let a = if i >= BPP { line[i - BPP] } else { 0 };
        let b = above[i];
        let c = if i >= BPP { above[i - BPP] } else { 0 };
        let prediction = match kind {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
            _ => paeth(a, b, c),
        };
        out.push(x.wrapping_sub(prediction));
    }
}

/// The scanlines, each led by the filter type that makes its bytes smallest (the usual
/// "minimum sum of absolute differences" rule).
fn filter(rgba: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() + rgba.len() / row);
    let blank = vec![0u8; row];
    let mut above: &[u8] = &blank;
    let mut candidate = Vec::with_capacity(row);
    let mut best = Vec::with_capacity(row);
    for line in rgba.chunks_exact(row) {
        let mut best_kind = 0;
        let mut best_score = u64::MAX;
        for kind in 0..5u8 {
            candidate.clear();
            filter_row(kind, line, above, &mut candidate);
            let score: u64 = candidate.iter().map(|&v| u64::from((v as i8).unsigned_abs())).sum();
            if score < best_score {
                best_score = score;
                best_kind = kind;
                std::mem::swap(&mut best, &mut candidate);
            }
        }
        out.push(best_kind);
        out.extend_from_slice(&best);
        above = line;
    }
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend_from_slice(data);
    out.extend(crc32(&[kind, data]).to_be_bytes());
}

/// CRC-32 (polynomial 0xEDB88320) over the parts in order.
fn crc32(parts: &[&[u8]]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0;
        while n < 256 {
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
                k += 1;
            }
            table[n] = c;
            n += 1;
        }
        table
    };
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &byte in *part {
            crc = TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
        }
    }
    !crc
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deflate::inflate::unzlib;

    /// Reads a PNG written by [`encode_rgba`] back to (width, height, RGBA), checking every
    /// chunk's CRC and undoing the row filters. Independent of the encoder's filter code.
    pub(crate) fn decode(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
        if png.get(..8) != Some(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Err("signature".into());
        }
        let (mut at, mut header, mut data) = (8, Vec::new(), Vec::new());
        loop {
            let len = u32::from_be_bytes(png.get(at..at + 4).ok_or("chunk length")?.try_into().unwrap()) as usize;
            let kind = png.get(at + 4..at + 8).ok_or("chunk type")?;
            let body = png.get(at + 8..at + 8 + len).ok_or("chunk body")?;
            let crc = png.get(at + 8 + len..at + 12 + len).ok_or("chunk crc")?;
            if crc != crc32(&[kind, body]).to_be_bytes() {
                return Err("crc".into());
            }
            at += 12 + len;
            match kind {
                b"IHDR" => header = body.to_vec(),
                b"IDAT" => data.extend_from_slice(body),
                b"IEND" => break,
                _ => return Err("unexpected chunk".into()),
            }
        }
        if at != png.len() || header.len() != 13 || header[8..] != [8, 6, 0, 0, 0] {
            return Err("header".into());
        }
        let width = u32::from_be_bytes(header[..4].try_into().unwrap());
        let height = u32::from_be_bytes(header[4..8].try_into().unwrap());
        let row = width as usize * 4;
        let raw = unzlib(&data)?;
        if raw.len() != (row + 1) * height as usize {
            return Err("raw size".into());
        }
        let mut out: Vec<u8> = Vec::with_capacity(row * height as usize);
        for (y, line) in raw.chunks_exact(row + 1).enumerate() {
            for i in 0..row {
                let a = if i >= 4 { out[y * row + i - 4] } else { 0 };
                let b = if y > 0 { out[(y - 1) * row + i] } else { 0 };
                let c = if i >= 4 && y > 0 { out[(y - 1) * row + i - 4] } else { 0 };
                let prediction = match line[0] {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    4 => {
                        // Written out again rather than shared with the encoder.
                        let p = i32::from(a) + i32::from(b) - i32::from(c);
                        let d = |v: u8| (p - i32::from(v)).abs();
                        if d(a) <= d(b) && d(a) <= d(c) {
                            a
                        } else if d(b) <= d(c) {
                            b
                        } else {
                            c
                        }
                    }
                    _ => return Err("filter type".into()),
                };
                out.push(line[1 + i].wrapping_add(prediction));
            }
        }
        Ok((width, height, out))
    }

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[b"1234", b"56789"]), 0xCBF4_3926);
    }

    #[test]
    fn file_structure_is_exact() {
        let png = encode_rgba(1, 1, &[255, 0, 0, 255]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[8..16], &[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        assert_eq!(&png[16..29], &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        assert_eq!(&png[29..33], &[0x1F, 0x15, 0xC4, 0x89]);
        assert_eq!(&png[37..41], b"IDAT");
        assert_eq!(&png[png.len() - 12..], &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
        assert_eq!(decode(&png), Ok((1, 1, vec![255, 0, 0, 255])));
    }

    #[test]
    fn pictures_round_trip_and_shrink() {
        // A flat shape with a soft edge on a transparent background, like a rendered gun.
        let (w, h) = (300u32, 120u32);
        let mut flat = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let inside = (40..260).contains(&x) && (30..90).contains(&y);
                let edge = x == 40 || x == 259 || y == 30 || y == 89;
                flat.extend(match (inside, edge) {
                    (true, true) => [20, 20, 30, 128],
                    (true, false) => [230, 200, (x / 50) as u8 * 40, 255],
                    _ => [0, 0, 0, 0],
                });
            }
        }
        let png = encode_rgba(w, h, &flat);
        assert_eq!(decode(&png), Ok((w, h, flat.clone())));
        assert!(png.len() < flat.len() / 50, "{} bytes", png.len());

        // Gradients (where the row filters matter) and noise.
        let gradient: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, (x + y) as u8, 255])).collect();
        let png = encode_rgba(w, h, &gradient);
        assert_eq!(decode(&png), Ok((w, h, gradient.clone())));
        assert!(png.len() < gradient.len() / 20, "{} bytes", png.len());
        let mut seed = 77u32;
        let noise: Vec<u8> = (0..w * h * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        assert_eq!(decode(&encode_rgba(w, h, &noise)), Ok((w, h, noise.clone())));
        // Odd sizes.
        for (w, h) in [(1u32, 7u32), (7, 1), (3, 3)] {
            let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 31) as u8).collect();
            assert_eq!(decode(&encode_rgba(w, h, &px)), Ok((w, h, px)));
        }
    }
}

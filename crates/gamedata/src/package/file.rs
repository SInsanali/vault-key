//! Undoes the LZO wrapper of UE3 package files (both layouts) so callers see a normal
//! uncompressed package. Reads only; never writes.

use std::path::Path;

use super::reader::ByteReader;
use crate::error::{GameDataError, Result};

/// The UE3 package tag, also the tag of every chunk stream.
pub const PACKAGE_TAG: u32 = 0x9E2A_83C1;

const MAX_BLOCK_SIZE: usize = 1_048_576;
/// Largest decompressed package accepted (512 MiB). The biggest real one is about 60 MB.
const MAX_OUTPUT: usize = 536_870_912;

/// Reads a package file (opened read-only) and returns its uncompressed bytes.
pub fn read_package(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path)
        .map_err(|_| GameDataError::InstallNotFound(path.display().to_string()))?;
    decompress(&raw, &file_name(path))
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Returns the uncompressed package for `raw`, whichever layout it uses. `name` only labels errors.
///
/// Layout (a), whole-file compressed: chunk streams back to back until the end of the file. It is
/// recognised by the u16 after the tag (where a package has its version) being below 500.
/// Layout (b): a normal header whose chunk table points at chunk streams. A package with an empty
/// chunk table is returned unchanged.
pub fn decompress(raw: &[u8], name: &str) -> Result<Vec<u8>> {
    if raw.len() < 6 {
        return Err(GameDataError::Truncated(format!("{name}: header")));
    }
    let mut r = ByteReader::new(raw);
    if r.u32()? != PACKAGE_TAG {
        return Err(GameDataError::NotAPackage(name.to_string()));
    }
    if r.u16()? < 500 {
        whole_file(raw, name)
    } else {
        chunked(raw, name)
    }
}

fn whole_file(raw: &[u8], name: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < raw.len() {
        offset = stream(raw, offset, raw.len(), &mut out, MAX_OUTPUT, name)?;
    }
    Ok(out)
}

struct Chunk {
    uncomp_offset: usize,
    uncomp_size: usize,
    comp_offset: usize,
    comp_size: usize,
}

fn chunked(raw: &[u8], name: &str) -> Result<Vec<u8>> {
    let mut r = ByteReader::with_range(raw, 8, raw.len());
    r.skip(4)?; // totalHeaderSize
    r.fstring()?; // folder
    r.skip(4)?; // packageFlags
    r.skip(6 * 4)?; // name, export, import: count and offset
    r.skip(4)?; // dependsOffset
    r.skip(3 * 4)?; // import/export guids
    r.skip(4)?; // thumbnailTableOffset
    r.skip(16)?; // guid
    let generations = r.i32_size(&format!("{name}: generations"))?;
    if generations > raw.len() / 12 {
        return Err(GameDataError::UnexpectedData(format!(
            "{name}: generations {generations}"
        )));
    }
    r.skip(generations * 12)?;
    r.skip(8)?; // engine, cooker
    r.skip(4)?; // compressionFlags
    let count = r.i32_size(&format!("{name}: chunk count"))?;
    if count == 0 {
        return Ok(raw.to_vec());
    }
    if count > r.remaining() / 16 {
        return Err(GameDataError::Truncated(format!("{name}: chunk table")));
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let what = format!("{name}: chunk entry");
        let chunk = Chunk {
            uncomp_offset: r.i32_size(&what)?,
            uncomp_size: r.i32_size(&what)?,
            comp_offset: r.i32_size(&what)?,
            comp_size: r.i32_size(&what)?,
        };
        if chunk.comp_offset > raw.len() || chunk.comp_size > raw.len() - chunk.comp_offset {
            return Err(GameDataError::UnexpectedData(what));
        }
        chunks.push(chunk);
    }
    // The bytes before the first chunk (the header) are stored as they are.
    let first = chunks[0].uncomp_offset;
    if first > raw.len() {
        return Err(GameDataError::UnexpectedData(format!(
            "{name}: chunk offset"
        )));
    }
    let mut out = raw[..first].to_vec();
    for chunk in &chunks {
        if chunk.uncomp_offset != out.len() {
            return Err(GameDataError::UnexpectedData(format!(
                "{name}: non-contiguous chunk"
            )));
        }
        if chunk.uncomp_size > MAX_OUTPUT - out.len() {
            return Err(GameDataError::UnexpectedData(format!(
                "{name}: output too large"
            )));
        }
        let limit = out.len() + chunk.uncomp_size;
        let end = chunk.comp_offset + chunk.comp_size;
        let mut offset = chunk.comp_offset;
        while offset < end {
            offset = stream(raw, offset, end, &mut out, limit, name)?;
        }
        if out.len() != limit {
            return Err(GameDataError::UnexpectedData(format!(
                "{name}: chunk holds {} bytes, expected {}",
                out.len() - chunk.uncomp_offset,
                chunk.uncomp_size
            )));
        }
    }
    Ok(out)
}

/// Decodes one chunk stream (`tag, blockSize, compTotal, uncompTotal`, the `(compSize,
/// uncompSize)` pairs, then the LZO1X blocks) from `raw[start..end]`, appending to `out`, which
/// may not grow past `limit`. Returns the offset just past the stream.
fn stream(
    raw: &[u8],
    start: usize,
    end: usize,
    out: &mut Vec<u8>,
    limit: usize,
    name: &str,
) -> Result<usize> {
    let mut r = ByteReader::with_range(raw, start, end);
    if r.u32()? != PACKAGE_TAG {
        return Err(GameDataError::NotAPackage(format!(
            "{name}: chunk stream tag"
        )));
    }
    let block_size = r.u32_size()?;
    let comp_total = r.u32_size()?;
    let uncomp_total = r.u32_size()?;
    if !(1..=MAX_BLOCK_SIZE).contains(&block_size) {
        return Err(GameDataError::UnexpectedData(format!(
            "{name}: block size {block_size}"
        )));
    }
    if uncomp_total > limit.saturating_sub(out.len()) {
        return Err(GameDataError::UnexpectedData(format!(
            "{name}: output too large"
        )));
    }
    let blocks = uncomp_total.div_ceil(block_size);
    if blocks > r.remaining() / 8 {
        return Err(GameDataError::Truncated(format!("{name}: block table")));
    }
    let mut pairs = Vec::with_capacity(blocks);
    let (mut comp_sum, mut uncomp_sum) = (0usize, 0usize);
    for _ in 0..blocks {
        let (comp, uncomp) = (r.u32_size()?, r.u32_size()?);
        if uncomp > block_size {
            return Err(GameDataError::UnexpectedData(format!(
                "{name}: block size {uncomp}"
            )));
        }
        if comp > end {
            return Err(GameDataError::UnexpectedData(format!(
                "{name}: block compressed size"
            )));
        }
        comp_sum = comp_sum.saturating_add(comp);
        uncomp_sum = uncomp_sum.saturating_add(uncomp);
        pairs.push((comp, uncomp));
    }
    if comp_sum != comp_total || uncomp_sum != uncomp_total {
        return Err(GameDataError::UnexpectedData(format!(
            "{name}: block sums do not match stream totals"
        )));
    }
    out.reserve(uncomp_total);
    for (comp, uncomp) in pairs {
        let block = r.take(comp)?;
        let data = lzo1x::decompress(block, uncomp)
            .map_err(|e| GameDataError::CorruptCompression(format!("{name}: {}", e.0)))?;
        out.extend_from_slice(&data);
    }
    Ok(r.position())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        chunk_stream, chunked, property_body, whole_file_compressed, Pkg, FLAGS_OFFSET, HEADER_SIZE,
    };

    /// A small valid package, with `pad` extra bytes of varied data in its one export.
    fn sample(pad: usize) -> Vec<u8> {
        let mut body = property_body(&[]);
        body.extend((0..pad).map(|i| (i.wrapping_mul(7) + (i >> 8)) as u8));
        let mut pkg = Pkg::new();
        let class = pkg.import("Package", 0, "Core");
        pkg.raw_export(class, 0, "Thing", body);
        pkg.build()
    }

    fn error_of(raw: &[u8]) -> GameDataError {
        decompress(raw, "t")
            .map(|out| out.len())
            .expect_err("decompressing should fail")
    }

    #[test]
    fn uncompressed_passes_through() {
        let p = sample(0);
        assert!(decompress(&p, "t").unwrap() == p);
    }

    #[test]
    fn whole_file_layout_round_trips() {
        let mut p = sample(0);
        p.resize(300_000, 0x5A);
        // Two streams: one tag at 0, one after the first stream.
        let c = whole_file_compressed(&p, 0x20000, 0x40000);
        assert!(c.len() > 300_000);
        assert_eq!(u16::from_le_bytes([c[4], c[5]]), 0);
        assert!(decompress(&c, "t").unwrap() == p);
        // Smaller blocks, so each stream holds several. (The block size's low 16 bits must stay
        // below 500, as the game's 0x20000 does: that is how the layout is recognised.)
        assert!(decompress(&whole_file_compressed(&p, 0x10000, 0x40000), "t").unwrap() == p);
    }

    #[test]
    fn chunk_table_layout_round_trips() {
        let p = sample(200_000);
        let out = decompress(&chunked(&p, 0x20000), "t").unwrap();
        // The header keeps its size, so only the compression flags and chunk count differ.
        let mut expected = p.clone();
        expected[FLAGS_OFFSET..FLAGS_OFFSET + 8].copy_from_slice(&[2, 0, 0, 0, 1, 0, 0, 0]);
        assert!(out == expected);
        assert!(out[HEADER_SIZE..] == p[HEADER_SIZE..]);
    }

    #[test]
    fn corrupt_packages_are_errors() {
        assert!(matches!(error_of(&[]), GameDataError::Truncated(_)));
        assert!(matches!(
            error_of(&[1, 2, 3, 4, 5, 6, 7, 8]),
            GameDataError::NotAPackage(_)
        ));

        let mut flipped = sample(0);
        flipped[0] ^= 0xFF;
        assert!(matches!(error_of(&flipped), GameDataError::NotAPackage(_)));

        // Cut in half, and cut at every length near the start.
        let whole = whole_file_compressed(&sample(100_000), 0x20000, 0x10000);
        assert!(decompress(&whole[..whole.len() / 2], "t").is_err());
        for len in 0..200 {
            assert!(decompress(&whole[..len], "t").is_err(), "{len}");
        }
        let chunks = chunked(&sample(1000), 0x20000);
        for len in 0..chunks.len() {
            assert!(decompress(&chunks[..len], "t").is_err(), "{len}");
        }

        // A block that claims 0x40000000 bytes with a block size of 0x20000.
        let mut s = chunk_stream(&sample(0), 0x20000);
        s[20..24].copy_from_slice(&0x4000_0000u32.to_le_bytes());
        assert!(matches!(error_of(&s), GameDataError::UnexpectedData(_)));

        // A stream that claims a huge total, and one with a zero block size.
        let mut s = chunk_stream(&sample(0), 0x20000);
        s[12..16].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(decompress(&s, "t").is_err());
        let mut s = chunk_stream(&sample(0), 0x20000);
        s[4..8].copy_from_slice(&[0; 4]);
        assert!(matches!(error_of(&s), GameDataError::UnexpectedData(_)));

        // A damaged LZO end-of-stream marker.
        let mut damaged = chunked(&sample(1000), 0x20000);
        let n = damaged.len();
        damaged[n - 3] ^= 0xFF;
        assert!(matches!(
            error_of(&damaged),
            GameDataError::CorruptCompression(_) | GameDataError::Truncated(_)
        ));
    }

    #[test]
    fn chunk_table_is_checked() {
        let good = chunked(&sample(1000), 0x20000);
        let entry = HEADER_SIZE; // uncompOffset, uncompSize, compOffset, compSize
        let patched = |offset: usize, value: i32| {
            let mut c = good.clone();
            c[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            c
        };
        // The chunk count: absurd, negative.
        assert!(matches!(
            error_of(&patched(FLAGS_OFFSET + 4, i32::MAX)),
            GameDataError::Truncated(_)
        ));
        assert!(decompress(&patched(FLAGS_OFFSET + 4, -1), "t").is_err());
        // A first chunk that starts past the end of the file.
        assert!(matches!(
            error_of(&patched(entry, 1 << 20)),
            GameDataError::UnexpectedData(_)
        ));
        // The chunk claims more, or less, than its stream holds.
        assert!(matches!(
            error_of(&patched(entry + 4, 5000)),
            GameDataError::UnexpectedData(_)
        ));
        assert!(matches!(
            error_of(&patched(entry + 4, 10)),
            GameDataError::UnexpectedData(_)
        ));
        // Compressed data outside the file.
        assert!(matches!(
            error_of(&patched(entry + 8, i32::MAX)),
            GameDataError::UnexpectedData(_)
        ));
        assert!(matches!(
            error_of(&patched(entry + 12, i32::MAX)),
            GameDataError::UnexpectedData(_)
        ));
        assert!(decompress(&patched(entry + 8, -5), "t").is_err());
    }
}

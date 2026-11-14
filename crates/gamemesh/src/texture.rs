//! `Texture2D` objects: the mip table, pixel data stored inline or in a `.tfc` texture cache
//! beside the package, and DXT1/DXT5 decoding. Layout worked out from the game's files; see
//! the research document, §5.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use gamedata::package::UPackage;

use crate::error::{MeshError, Result};
use crate::reader::Cursor;

/// Bulk-data flag: the payload is in a separate file (the texture cache).
const STORED_SEPARATELY: u32 = 0x01;
/// Bulk-data flag: the payload is an LZO chunk stream.
const COMPRESSED_LZO: u32 = 0x10;
/// Bulk-data flag: there is no payload.
const UNUSED: u32 = 0x20;
/// The tag that starts a compressed chunk stream (the package tag).
const CHUNK_TAG: u32 = 0x9E2A_83C1;
/// Largest mip payload accepted (a 4096 × 4096 DXT5 mip is 16 MiB).
const MAX_MIP_BYTES: usize = 64 << 20;
/// Largest mip side decoded: the game's largest textures are 4096 on a side. A mip this size
/// decodes to 64 MiB of pixels.
const MAX_SIDE: u32 = 4096;
/// Most mips a texture may list.
const MAX_MIPS: usize = 32;
/// Largest block of a texture cache chunk stream. The game's caches use 128 KiB blocks; this
/// is the package reader's limit too.
const MAX_BLOCK_SIZE: usize = 1 << 20;
/// Longest texture cache name accepted. The longest the game names is 25 characters.
const MAX_CACHE_NAME: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Dxt1,
    Dxt5,
    /// Uncompressed, 4 bytes a texel, stored B, G, R, A.
    Bgra8,
}

/// One mip level of a texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mip {
    pub width: u32,
    pub height: u32,
    flags: u32,
    /// Uncompressed payload size.
    byte_count: usize,
    size_on_disk: usize,
    /// Offset in the texture cache file when stored separately.
    offset: u64,
    /// The payload when it is stored in the package.
    inline: Option<Vec<u8>>,
}

impl Mip {
    pub fn is_inline(&self) -> bool {
        self.inline.is_some()
    }
}

/// A `Texture2D`'s description and mip table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub path: String,
    pub format: PixelFormat,
    /// The texture cache holding the mips that are not inline, such as `CharTextures`.
    pub cache_name: Option<String>,
    /// Largest first.
    pub mips: Vec<Mip>,
}

/// Decoded pixels, RGBA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    /// Whether the buffer really holds `width * height` texels (the fields are public).
    fn is_sound(&self) -> bool {
        self.width > 0
            && self.height > 0
            && (self.width as usize).checked_mul(self.height as usize).and_then(|n| n.checked_mul(4)) == Some(self.rgba.len())
    }

    /// Bytes of memory the pixels take.
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

    /// The texel at texture coordinate (u, v), wrapping, nearest neighbour.
    pub fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        if !self.is_sound() {
            return [0; 4];
        }
        let wrap = |t: f32, size: u32| -> usize {
            let t = if t.is_finite() { t - t.floor() } else { 0.0 };
            ((t * size as f32) as usize).min(size as usize - 1)
        };
        let at = (wrap(v, self.height) * self.width as usize + wrap(u, self.width)) * 4;
        [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2], self.rgba[at + 3]]
    }
}

impl Pixels {
    /// The texture at (u, v), wrapping, blended from the four nearest texels; 0…255 per channel.
    pub fn sample_linear(&self, u: f32, v: f32) -> [f32; 4] {
        if !self.is_sound() || !u.is_finite() || !v.is_finite() {
            return [0.0; 4];
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let x = (u - u.floor()) * w as f32 - 0.5;
        let y = (v - v.floor()) * h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let texel = |dx: usize, dy: usize| -> [f32; 4] {
            let xi = (x0 as isize + dx as isize).rem_euclid(w as isize) as usize;
            let yi = (y0 as isize + dy as isize).rem_euclid(h as isize) as usize;
            let at = (yi * w + xi) * 4;
            [0, 1, 2, 3].map(|c| f32::from(self.rgba[at + c]))
        };
        let (a, b, c, d) = (texel(0, 0), texel(1, 0), texel(0, 1), texel(1, 1));
        [0, 1, 2, 3].map(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

impl Texture {
    /// Reads the texture stored in export `index` of `pkg`.
    pub fn from_export(pkg: &UPackage, index: usize) -> Result<Self> {
        let path = pkg.export_path(index)?;
        let class = pkg.class_name_of_export(index);
        if class != "Texture2D" {
            return Err(MeshError::Unsupported(format!("{path} is a {class}, not a Texture2D")));
        }
        let (props, native) = pkg.properties_and_native_data(index)?;
        let format = match props.enum_name("Format") {
            Some("PF_DXT1") => PixelFormat::Dxt1,
            Some("PF_DXT5") => PixelFormat::Dxt5,
            Some("PF_A8R8G8B8") => PixelFormat::Bgra8,
            other => return Err(MeshError::Unsupported(format!("{path}: pixel format {other:?}"))),
        };
        let cache_name = props.name("TextureFileCacheName").filter(|n| *n != "None").map(str::to_string);

        let mut c = Cursor::new(native);
        c.skip(16)?; // the source-art bulk-data header, empty in cooked packages
        let count = c.count(24, "mips")?;
        if count > MAX_MIPS {
            return Err(MeshError::Unsupported(format!("{path}: {count} mips")));
        }
        let mut mips = Vec::with_capacity(count);
        for _ in 0..count {
            let flags = c.u32()?;
            let byte_count = c.i32()?;
            let byte_count = usize::try_from(byte_count)
                .ok()
                .filter(|&n| n <= MAX_MIP_BYTES)
                .ok_or_else(|| MeshError::Unsupported(format!("{path}: mip of {byte_count} bytes")))?;
            let size_on_disk = c.i32()?;
            let offset = c.i32()?;
            let inline = if flags & (STORED_SEPARATELY | UNUSED) == 0 {
                if flags != 0 {
                    return Err(MeshError::Unsupported(format!("{path}: inline mip with flags {flags:#x}")));
                }
                Some(c.take(byte_count)?.to_vec())
            } else {
                None
            };
            let (width, height) = (c.u32()?, c.u32()?);
            mips.push(Mip {
                width,
                height,
                flags,
                byte_count,
                size_on_disk: usize::try_from(size_on_disk).unwrap_or(0),
                offset: u64::try_from(offset).unwrap_or(0),
                inline,
            });
        }
        Ok(Self { path, format, cache_name, mips })
    }

    /// The largest mip no wider or taller than `max_size` (or the smallest mip there is).
    pub fn mip_for(&self, max_size: u32) -> Option<&Mip> {
        self.mips
            .iter()
            .filter(|m| m.flags & UNUSED == 0)
            .find(|m| m.width <= max_size && m.height <= max_size)
            .or_else(|| self.mips.iter().rev().find(|m| m.flags & UNUSED == 0))
    }

    /// As [`Texture::decode`], looking for the `.tfc` cache in each of `dirs` in turn (a DLC
    /// texture's cache is in that DLC's folder).
    pub fn decode_from(&self, mip: &Mip, dirs: &[&Path]) -> Result<Pixels> {
        let dir = if mip.inline.is_some() {
            dirs.first()
        } else {
            // Checked before the name goes anywhere near the file system.
            let file = format!("{}.tfc", self.checked_cache_name()?);
            dirs.iter().find(|dir| dir.join(&file).is_file()).or(dirs.first())
        };
        let dir = dir.ok_or_else(|| MeshError::Missing(format!("{}: a folder for its texture cache", self.path)))?;
        self.decode(mip, dir)
    }

    /// Decodes a mip. `package_dir` is the folder holding the package and its `.tfc` files;
    /// it is only read.
    pub fn decode(&self, mip: &Mip, package_dir: &Path) -> Result<Pixels> {
        let data = match &mip.inline {
            Some(data) => data.clone(),
            None => {
                let file = package_dir.join(format!("{}.tfc", self.checked_cache_name()?));
                read_cached_mip(&file, mip)?
            }
        };
        if mip.width == 0 || mip.height == 0 || mip.width > MAX_SIDE || mip.height > MAX_SIDE {
            return Err(MeshError::Unsupported(format!("{}: a {}x{} mip", self.path, mip.width, mip.height)));
        }
        let (w, h) = (mip.width as usize, mip.height as usize);
        let expected = match self.format {
            PixelFormat::Dxt1 => w.div_ceil(4) * h.div_ceil(4) * 8,
            PixelFormat::Dxt5 => w.div_ceil(4) * h.div_ceil(4) * 16,
            PixelFormat::Bgra8 => w * h * 4,
        };
        if data.len() < expected {
            return Err(MeshError::Truncated(format!("{}: {} bytes for {w}x{h}", self.path, data.len())));
        }
        let rgba = match self.format {
            PixelFormat::Bgra8 => data[..expected].as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
            _ => decode_dxt(&data, w, h, self.format),
        };
        Ok(Pixels { width: mip.width, height: mip.height, rgba })
    }

    /// The texture cache's name, once it is known to be a plain file name (see
    /// [`is_plain_cache_name`]). The name comes from the package, so it is checked before it is
    /// ever joined to a folder.
    fn checked_cache_name(&self) -> Result<&str> {
        let cache = self
            .cache_name
            .as_deref()
            .ok_or_else(|| MeshError::Missing(format!("{}: texture cache name", self.path)))?;
        if !is_plain_cache_name(cache) {
            return Err(MeshError::Unsupported(format!("{}: texture cache name {cache:?}", self.path)));
        }
        Ok(cache)
    }
}

/// Whether a texture cache name from a package is safe to look up beside it: 1 to
/// [`MAX_CACHE_NAME`] ASCII letters, digits and underscores (so no separator, drive, dot or
/// `\\server\share` prefix), and not a Windows device name such as `CON` or `COM1`, which
/// would open the device instead of a file.
fn is_plain_cache_name(name: &str) -> bool {
    const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    let numbered_device = |prefix: &str| {
        name.len() == 4 && name[..3].eq_ignore_ascii_case(prefix) && name.as_bytes()[3].is_ascii_digit()
    };
    (1..=MAX_CACHE_NAME).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !DEVICES.iter().any(|device| name.eq_ignore_ascii_case(device))
        && !numbered_device("COM")
        && !numbered_device("LPT")
}

fn read_cached_mip(file: &Path, mip: &Mip) -> Result<Vec<u8>> {
    if mip.size_on_disk > MAX_MIP_BYTES || mip.byte_count > MAX_MIP_BYTES {
        return Err(MeshError::Unsupported(format!("mip of {} bytes", mip.byte_count)));
    }
    let io = |e: std::io::Error| MeshError::Missing(format!("{}: {e}", file.display()));
    let mut f = File::open(file).map_err(io)?;
    // Nothing is allocated for an entry that would run past the end of the file.
    let length = f.metadata().map_err(io)?.len();
    if mip.offset.checked_add(mip.size_on_disk as u64).is_none_or(|end| end > length) {
        return Err(MeshError::Truncated(format!(
            "{}: {} bytes at offset {} of a {length}-byte file",
            file.display(),
            mip.size_on_disk,
            mip.offset
        )));
    }
    f.seek(SeekFrom::Start(mip.offset)).map_err(io)?;
    let mut raw = vec![0u8; mip.size_on_disk];
    f.read_exact(&mut raw).map_err(io)?;
    if mip.flags & COMPRESSED_LZO == 0 {
        return Ok(raw);
    }
    // A chunk stream: tag, block size, compressed and uncompressed totals, one (compressed,
    // uncompressed) size pair per block, then the LZO1X blocks.
    let mut c = Cursor::new(&raw);
    if c.u32()? != CHUNK_TAG {
        return Err(MeshError::Unsupported("texture cache entry does not start with the chunk tag".into()));
    }
    let block_size = c.u32()? as usize;
    c.skip(4)?;
    let total = c.u32()? as usize;
    if !(1..=MAX_BLOCK_SIZE).contains(&block_size) {
        return Err(MeshError::Unsupported(format!("texture cache blocks of {block_size} bytes")));
    }
    if total != mip.byte_count {
        return Err(MeshError::Unsupported(format!("chunk stream of {total} bytes for a mip of {}", mip.byte_count)));
    }
    let blocks = total.div_ceil(block_size);
    if blocks > c.remaining() / 8 {
        return Err(MeshError::Truncated(format!("{blocks} blocks in a texture cache entry")));
    }
    // Every block's size is checked before anything is decompressed: each holds at most a
    // block and at most what is left of the mip, and the compressed sizes fit in the entry.
    let mut sizes = Vec::with_capacity(blocks);
    let (mut left, mut compressed_total) = (total, 0usize);
    for _ in 0..blocks {
        let (compressed, uncompressed) = (c.u32()? as usize, c.u32()? as usize);
        if uncompressed > block_size || uncompressed > left {
            return Err(MeshError::Unsupported(format!("block of {uncompressed} bytes, {left} left of the mip")));
        }
        left -= uncompressed;
        compressed_total = compressed_total.saturating_add(compressed);
        sizes.push((compressed, uncompressed));
    }
    if left != 0 || compressed_total > c.remaining() {
        return Err(MeshError::Truncated(format!("texture cache blocks short of the {total}-byte mip")));
    }
    let mut out = Vec::with_capacity(total);
    for (compressed, uncompressed) in sizes {
        let block = lzo1x::decompress(c.take(compressed)?, uncompressed)
            .map_err(|e| MeshError::Unsupported(format!("texture cache block: {e:?}")))?;
        out.extend_from_slice(&block);
    }
    if out.len() != total {
        return Err(MeshError::Truncated(format!("mip decompressed to {} of {total} bytes", out.len())));
    }
    Ok(out)
}

fn rgb565(c: u16) -> [u32; 3] {
    [u32::from(c >> 11) * 255 / 31, u32::from((c >> 5) & 63) * 255 / 63, u32::from(c & 31) * 255 / 31]
}

/// Decodes S3TC blocks (DXT1, or DXT5 with its interpolated alpha block) to RGBA.
fn decode_dxt(data: &[u8], w: usize, h: usize, format: PixelFormat) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let mut p = 0;
    for by in (0..h).step_by(4) {
        for bx in (0..w).step_by(4) {
            let mut alpha = [255u8; 16];
            if format == PixelFormat::Dxt5 {
                let (a0, a1) = (u32::from(data[p]), u32::from(data[p + 1]));
                let mut table = [a0, a1, 0, 0, 0, 0, 0, 255];
                if a0 > a1 {
                    for i in 1..7 {
                        table[i as usize + 1] = ((7 - i) * a0 + i * a1) / 7;
                    }
                } else {
                    for i in 1..5 {
                        table[i as usize + 1] = ((5 - i) * a0 + i * a1) / 5;
                    }
                }
                let bits = data[p + 2..p + 8].iter().rev().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
                for (i, a) in alpha.iter_mut().enumerate() {
                    *a = table[((bits >> (3 * i)) & 7) as usize] as u8;
                }
                p += 8;
            }
            let c0 = u16::from_le_bytes([data[p], data[p + 1]]);
            let c1 = u16::from_le_bytes([data[p + 2], data[p + 3]]);
            let bits = u32::from_le_bytes([data[p + 4], data[p + 5], data[p + 6], data[p + 7]]);
            p += 8;
            let (r0, r1) = (rgb565(c0), rgb565(c1));
            let mix = |a: [u32; 3], b: [u32; 3], wa: u32, wb: u32| [0, 1, 2].map(|i| (a[i] * wa + b[i] * wb) / (wa + wb));
            let colours = if c0 > c1 || format == PixelFormat::Dxt5 {
                [r0, r1, mix(r0, r1, 2, 1), mix(r0, r1, 1, 2)]
            } else {
                [r0, r1, mix(r0, r1, 1, 1), [0, 0, 0]]
            };
            for i in 0..16 {
                let (x, y) = (bx + (i & 3), by + (i >> 2));
                if x < w && y < h {
                    let c = colours[((bits >> (2 * i)) & 3) as usize];
                    let at = (y * w + x) * 4;
                    out[at..at + 4].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, alpha[i]]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt1_block_decodes() {
        // Colour 0 pure red, colour 1 pure blue (c0 > c1: four-colour mode); the index rows
        // select colour 0, 1, 2, 3 for the whole row.
        let block = [0x00, 0xF8, 0x1F, 0x00, 0b0000_0000, 0b0101_0101, 0b1010_1010, 0b1111_1111];
        let px = decode_dxt(&block, 4, 4, PixelFormat::Dxt1);
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[16..20], &[0, 0, 255, 255]);
        assert_eq!(&px[32..36], &[170, 0, 85, 255]);
        assert_eq!(&px[48..52], &[85, 0, 170, 255]);
        // c0 <= c1: three colours and black.
        let block = [0x1F, 0x00, 0x00, 0xF8, 0xFF, 0xFF, 0xAA, 0xAA];
        let px = decode_dxt(&block, 4, 4, PixelFormat::Dxt1);
        assert_eq!(&px[0..4], &[0, 0, 0, 255]);
        assert_eq!(&px[32..36], &[127, 0, 127, 255]);
    }

    #[test]
    fn dxt5_alpha_block_decodes() {
        // Alpha endpoints 255 and 0 (eight-value mode); the first texel uses index 0, the second
        // index 1, the third index 2 (6/7 of the way to 255).
        let mut block = vec![255, 0, 0b1000_1000, 0b0000_0000, 0, 0, 0, 0];
        block.extend([0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]); // white
        let px = decode_dxt(&block, 4, 4, PixelFormat::Dxt5);
        assert_eq!(&px[0..4], &[255, 255, 255, 255]);
        assert_eq!(px[7], 0);
        assert_eq!(px[11], 218);
    }

    /// A folder of its own in the system's temporary folder, removed when dropped.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gamemesh-unit-{}-{label}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Four BGRA texels: a 2 × 2 mip.
    const TEXELS: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

    /// A chunk stream holding `data` in blocks of `split` bytes, with `block_size` in its header
    /// and each block's declared uncompressed size taken from `declared` (its real size when
    /// `declared` runs out).
    fn chunk_stream(data: &[u8], block_size: u32, split: usize, declared: &[u32]) -> Vec<u8> {
        let blocks: Vec<&[u8]> = data.chunks(split).collect();
        let compressed: Vec<Vec<u8>> = blocks.iter().map(|b| lzo1x::compress_literals(b)).collect();
        let mut out = Vec::new();
        for word in [CHUNK_TAG, block_size, compressed.iter().map(Vec::len).sum::<usize>() as u32, data.len() as u32] {
            out.extend(word.to_le_bytes());
        }
        for (i, (block, packed)) in blocks.iter().zip(&compressed).enumerate() {
            let size = declared.get(i).copied().unwrap_or(block.len() as u32);
            out.extend((packed.len() as u32).to_le_bytes());
            out.extend(size.to_le_bytes());
        }
        out.extend(compressed.concat());
        out
    }

    /// A 2 × 2 texture whose one mip is the whole of `Cache.tfc` (`entry`), or inline.
    fn texture(cache_name: &str, entry_len: usize, inline: bool) -> Texture {
        let mip = Mip {
            width: 2,
            height: 2,
            flags: if inline { 0 } else { STORED_SEPARATELY | COMPRESSED_LZO },
            byte_count: TEXELS.len(),
            size_on_disk: entry_len,
            offset: 0,
            inline: inline.then(|| TEXELS.to_vec()),
        };
        Texture { path: "T".into(), format: PixelFormat::Bgra8, cache_name: Some(cache_name.into()), mips: vec![mip] }
    }

    #[test]
    fn cache_names_must_be_plain_file_names() {
        for good in ["CharTextures", "Textures", "Lighting_physx", "Dlc3_Textures", "COM", "CONSOLE", "LPT", "COMA"] {
            assert!(is_plain_cache_name(good), "{good}");
        }
        let longest = "T".repeat(MAX_CACHE_NAME);
        assert!(is_plain_cache_name(&longest));
        let too_long = "T".repeat(MAX_CACHE_NAME + 1);
        for bad in [
            "",
            too_long.as_str(),
            r"\\host\share\x",
            "//host/share/x",
            r"..\Textures",
            "C:Textures",
            "Textures.tfc",
            "Tex tures",
            "Tëxtures",
            "CON",
            "nul",
            "Aux",
            "PRN",
            "com1",
            "LPT9",
        ] {
            assert!(!is_plain_cache_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_bad_cache_name_is_refused_before_any_lookup() {
        let folder = Folder::new("names");
        let dirs = [folder.0.as_path()];
        for bad in [r"\\host\share\x", "CON", r"..\..\x"] {
            let texture = texture(bad, 0, false);
            let refused = texture.decode_from(&texture.mips[0], &dirs).unwrap_err();
            assert!(matches!(&refused, MeshError::Unsupported(m) if m.contains("texture cache name")), "{refused:?}");
            assert!(texture.decode(&texture.mips[0], &folder.0).is_err());
        }
        // An inline mip never uses the name.
        let inline = texture(r"\\host\share\x", 0, true);
        let pixels = inline.decode_from(&inline.mips[0], &dirs).unwrap();
        assert_eq!(&pixels.rgba[..4], &[3, 2, 1, 4]);
    }

    #[test]
    fn cache_blocks_are_bounded_before_decompressing() {
        let folder = Folder::new("blocks");
        let decode = |entry: &[u8], size_on_disk: usize| {
            std::fs::write(folder.0.join("Cache.tfc"), entry).unwrap();
            let texture = texture("Cache", size_on_disk, false);
            texture.decode_from(&texture.mips[0], &[folder.0.as_path()])
        };
        // Sound entries: one 128 KiB block, and two blocks of 8 bytes.
        let one = chunk_stream(&TEXELS, 0x20000, 16, &[]);
        assert_eq!(decode(&one, one.len()).unwrap().rgba[..4], [3, 2, 1, 4]);
        let two = chunk_stream(&TEXELS, 8, 8, &[]);
        assert_eq!(decode(&two, two.len()).unwrap().rgba[12..], [15, 14, 13, 16]);

        // A block size of nothing, or of gigabytes.
        for block_size in [0, (MAX_BLOCK_SIZE + 1) as u32, u32::MAX] {
            let entry = chunk_stream(&TEXELS, block_size, 16, &[]);
            assert!(matches!(decode(&entry, entry.len()), Err(MeshError::Unsupported(_))), "{block_size}");
        }
        // A block that claims more than a block, or more than is left of the mip.
        let entry = chunk_stream(&TEXELS, 16, 16, &[u32::MAX]);
        assert!(matches!(decode(&entry, entry.len()), Err(MeshError::Unsupported(_))));
        let entry = chunk_stream(&TEXELS, 10, 10, &[10, 10]);
        assert!(matches!(decode(&entry, entry.len()), Err(MeshError::Unsupported(m)) if m.contains("6 left")));
        // Blocks that add up to less than the mip.
        let entry = chunk_stream(&TEXELS, 10, 10, &[6, 6]);
        assert!(matches!(decode(&entry, entry.len()), Err(MeshError::Truncated(_))));
        // An entry that runs past the end of the file is refused before it is read.
        assert!(matches!(decode(&one, one.len() + 1), Err(MeshError::Truncated(_))));
        assert!(matches!(decode(&one, MAX_MIP_BYTES), Err(MeshError::Truncated(_))));
    }

    #[test]
    fn mips_larger_than_the_game_uses_are_not_decoded() {
        let mut texture = texture("Cache", 0, true);
        let folder = Folder::new("sides");
        for (side, refused) in [(4096, false), (4097, true), (8192, true)] {
            let mip = &mut texture.mips[0];
            (mip.width, mip.height) = (side, 2);
            let result = texture.decode(&texture.mips[0], &folder.0);
            // A side within the limit gets as far as finding the data short.
            assert_eq!(matches!(result, Err(MeshError::Unsupported(_))), refused, "{side}");
            assert_eq!(matches!(result, Err(MeshError::Truncated(_))), !refused, "{side}");
        }
    }

    #[test]
    fn sampling_wraps() {
        let pixels = Pixels { width: 2, height: 1, rgba: vec![1, 1, 1, 1, 2, 2, 2, 2] };
        assert_eq!(pixels.sample(0.1, 0.5)[0], 1);
        assert_eq!(pixels.sample(0.9, 0.5)[0], 2);
        assert_eq!(pixels.sample(1.1, -3.5)[0], 1);
        assert_eq!(pixels.sample(-0.1, 0.0)[0], 2);
        // Texel centres give the texel; half-way between gives the mean, also across the wrap.
        assert_eq!(pixels.sample_linear(0.25, 0.5)[0], 1.0);
        assert_eq!(pixels.sample_linear(0.75, 0.5)[0], 2.0);
        assert_eq!(pixels.sample_linear(0.5, 0.5)[0], 1.5);
        assert_eq!(pixels.sample_linear(0.0, 0.5)[0], 1.5);
    }
}

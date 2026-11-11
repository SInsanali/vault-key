use crate::checksums::crc32;
use crate::error::{Result, SaveKitError};
use crate::items::base64;

const WEAPON_FLAG: u8 = 0x80;
const VERSION_MASK: u8 = 0x7F;
/// Header byte, then the 4-byte key.
const HEADER_LEN: usize = 5;
/// For the checksum only, item bytes are padded with `0xFF` to this length.
const CHECKSUM_PAD_LEN: usize = 33;
const CODE_PREFIX: &str = "BL2(";
const CODE_SUFFIX: &str = ")";

/// An item or weapon blob (format doc 6.2 to 6.5), decrypted: a header byte, a key, and the
/// item bytes. The raw form is `header | key (i32 BE) | obfuscated(checksum | item bytes)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemBlob {
    pub is_weapon: bool,
    /// Item struct version, 7 bits: 7 in Borderlands 2.
    pub version: u8,
    /// Obfuscation key. Key 0 leaves the body as plain bytes.
    pub key: i32,
    /// The item bitstream, see [`crate::ItemFields`].
    pub item_bytes: Vec<u8>,
}

impl ItemBlob {
    /// Smallest raw blob: header (1), key (4) and checksum (2).
    pub const MIN_RAW_LEN: usize = 7;

    pub fn new(is_weapon: bool, version: u8, key: i32, item_bytes: Vec<u8>) -> Self {
        ItemBlob { is_weapon, version: version & VERSION_MASK, key, item_bytes }
    }

    /// Decrypts a raw blob and verifies its checksum. A blob too short to hold a checksum
    /// fails with [`SaveKitError::ItemChecksumMismatch`] as well.
    pub fn from_raw(raw: &[u8]) -> Result<Self> {
        if raw.len() < Self::MIN_RAW_LEN {
            return Err(SaveKitError::ItemChecksumMismatch);
        }
        let (header, body) = raw.split_at(HEADER_LEN);
        let key = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        let plain = crypt(body, key, Direction::Decrypt);
        let (stored, item) = plain.split_at(2);
        if stored != checksum(header, item).to_be_bytes() {
            return Err(SaveKitError::ItemChecksumMismatch);
        }
        Ok(ItemBlob {
            is_weapon: header[0] & WEAPON_FLAG != 0,
            version: header[0] & VERSION_MASK,
            key,
            item_bytes: item.to_vec(),
        })
    }

    /// Parses an item code, `BL2(<Base64 of a raw blob>)`. Space around the code is ignored.
    pub fn from_code(code: &str) -> Result<Self> {
        let invalid = |reason: &str| SaveKitError::invalid("item", reason);
        let base64_text = code
            .trim()
            .strip_prefix(CODE_PREFIX)
            .and_then(|rest| rest.strip_suffix(CODE_SUFFIX))
            .ok_or_else(|| invalid("item code must look like BL2(...)"))?;
        let raw = base64::decode(base64_text).ok_or_else(|| invalid("invalid Base64 in item code"))?;
        Self::from_raw(&raw)
    }

    /// Encrypts with `self.key`, computing the checksum.
    pub fn to_raw(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(Self::MIN_RAW_LEN + self.item_bytes.len());
        let weapon_flag = if self.is_weapon { WEAPON_FLAG } else { 0 };
        raw.push(weapon_flag | (self.version & VERSION_MASK));
        raw.extend_from_slice(&self.key.to_be_bytes());
        let mut plain = Vec::with_capacity(2 + self.item_bytes.len());
        plain.extend_from_slice(&checksum(&raw, &self.item_bytes).to_be_bytes());
        plain.extend_from_slice(&self.item_bytes);
        raw.extend_from_slice(&crypt(&plain, self.key, Direction::Encrypt));
        raw
    }

    /// The same item under another key.
    pub fn rekeyed(&self, key: i32) -> ItemBlob {
        ItemBlob { key, ..self.clone() }
    }

    /// The item code: `BL2(` + Base64 of the key-0 blob + `)`.
    pub fn code(&self) -> String {
        format!("{CODE_PREFIX}{}{CODE_SUFFIX}", base64::encode(&self.rekeyed(0).to_raw()))
    }
}

/// CRC-32 over `header | FF FF | item bytes padded to 33 with FF`, folded to 16 bits.
fn checksum(header: &[u8], item: &[u8]) -> u16 {
    let mut input = Vec::with_capacity(HEADER_LEN + 2 + item.len().max(CHECKSUM_PAD_LEN));
    input.extend_from_slice(header);
    input.extend_from_slice(&[0xFF, 0xFF]);
    input.extend_from_slice(item);
    let padded_len = input.len().max(HEADER_LEN + 2 + CHECKSUM_PAD_LEN);
    input.resize(padded_len, 0xFF);
    let crc = crc32(&input);
    ((crc >> 16) ^ (crc & 0xFFFF)) as u16
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Encrypt,
    Decrypt,
}

/// Encrypt: rotate left by `key & 31` bytes, then XOR with the keystream.
/// Decrypt: XOR with the keystream, then rotate right.
fn crypt(input: &[u8], key: i32, direction: Direction) -> Vec<u8> {
    let mut buf = input.to_vec();
    if buf.is_empty() {
        return buf;
    }
    let rotation = (key & 31) as usize % buf.len();
    if direction == Direction::Encrypt {
        buf.rotate_left(rotation);
    }
    // The seed is an arithmetic shift of the signed key; the modulus is the prime 2^32 - 5.
    let mut state = u64::from((key >> 5) as u32);
    for byte in &mut buf {
        state = (state * 279_470_273) % 4_294_967_291;
        *byte ^= (state & 0xFF) as u8;
    }
    if direction == Direction::Decrypt {
        buf.rotate_right(rotation);
    }
    buf
}

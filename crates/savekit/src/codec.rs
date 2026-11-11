//! The outer `.sav` container (format doc section 1):
//! `SHA-1 | u32 BE length | LZO1X(WSG blob)`, where the WSG blob is
//! `size | "WSG" | version | CRC-32 | length | Huffman stream | 4 trailing bytes`.

use crate::checksums::{crc32, sha1};
use crate::error::{Result, SaveKitError};
use crate::huffman;

/// Real saves are tens of kilobytes. A file that declares a WSG blob or a payload larger
/// than this is rejected, and [`encode`] refuses to write one.
pub const MAX_DECLARED_LEN: usize = 16 * 1024 * 1024;

const HASH_LEN: usize = 20;
/// SHA-1, then the big-endian decompressed length.
const FILE_HEADER_LEN: usize = HASH_LEN + 4;
/// `size`, magic, version, CRC and length: the Huffman stream starts here.
const WSG_HEADER_LEN: usize = 19;
const WSG_MAGIC: &[u8] = b"WSG";
const WSG_VERSION: u32 = 2;

fn not_a_save(why: &str) -> SaveKitError {
    SaveKitError::NotASave(why.to_string())
}

fn four_bytes(bytes: &[u8], offset: usize) -> Result<[u8; 4]> {
    offset
        .checked_add(4)
        .and_then(|end| bytes.get(offset..end))
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .ok_or_else(|| not_a_save("header too short"))
}

/// Unwraps a `.sav` file to its protobuf payload, checking the SHA-1 and the CRC-32.
///
/// The WSG `size` field and the 4 bytes after the Huffman stream are not checked: the
/// Windows game leaves non-zero bytes there.
pub fn decode(file: &[u8]) -> Result<Vec<u8>> {
    if file.len() < FILE_HEADER_LEN {
        return Err(not_a_save("file too short"));
    }
    let (digest, body) = file.split_at(HASH_LEN);
    if sha1(body) != digest {
        return Err(SaveKitError::HashMismatch);
    }
    let declared = u32::from_be_bytes(four_bytes(body, 0)?) as usize;
    if declared > MAX_DECLARED_LEN {
        return Err(not_a_save("declared length too large"));
    }

    let wsg = lzo1x::decompress(&body[4..], declared)?;
    if wsg.len() < WSG_HEADER_LEN {
        return Err(not_a_save("WSG blob too short"));
    }
    if &wsg[4..7] != WSG_MAGIC {
        return Err(not_a_save("missing WSG magic"));
    }
    let version = four_bytes(&wsg, 7)?;
    if u32::from_be_bytes(version) == WSG_VERSION {
        return Err(SaveKitError::UnsupportedByteOrder);
    }
    if u32::from_le_bytes(version) != WSG_VERSION {
        return Err(SaveKitError::UnsupportedVersion(u32::from_le_bytes(version)));
    }
    let crc = u32::from_le_bytes(four_bytes(&wsg, 11)?);
    let length = u32::from_le_bytes(four_bytes(&wsg, 15)?) as usize;
    if length > MAX_DECLARED_LEN {
        return Err(not_a_save("payload length too large"));
    }

    let payload = huffman::decode(&wsg[WSG_HEADER_LEN..], length)?;
    if crc32(&payload) != crc {
        return Err(SaveKitError::CrcMismatch);
    }
    Ok(payload)
}

/// Wraps a protobuf payload as a `.sav` file: a fresh Huffman tree, four zero bytes after the
/// Huffman stream, and one LZO1X literal run (which the game accepts).
///
/// Fails only if the result would be larger than [`decode`] accepts.
pub fn encode(payload: &[u8]) -> Result<Vec<u8>> {
    let huff = huffman::encode(payload);
    let wsg_len = WSG_HEADER_LEN + huff.len() + 4;
    if payload.len() > MAX_DECLARED_LEN || wsg_len > MAX_DECLARED_LEN {
        return Err(SaveKitError::invalid("save", format!("too large to write ({} bytes of save data)", payload.len())));
    }

    let mut wsg = Vec::with_capacity(wsg_len);
    wsg.extend_from_slice(&((wsg_len - 4) as u32).to_be_bytes());
    wsg.extend_from_slice(WSG_MAGIC);
    wsg.extend_from_slice(&WSG_VERSION.to_le_bytes());
    wsg.extend_from_slice(&crc32(payload).to_le_bytes());
    wsg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wsg.extend_from_slice(&huff);
    wsg.extend_from_slice(&[0, 0, 0, 0]);

    let mut body = Vec::with_capacity(wsg.len() + wsg.len() / 255 + 16);
    body.extend_from_slice(&(wsg.len() as u32).to_be_bytes());
    body.extend_from_slice(&lzo1x::compress_literals(&wsg));

    let mut file = Vec::with_capacity(HASH_LEN + body.len());
    file.extend_from_slice(&sha1(&body));
    file.extend_from_slice(&body);
    Ok(file)
}

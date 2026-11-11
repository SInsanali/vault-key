use savekit::checksums::{crc32, sha1};
use savekit::{codec, huffman, SaveKitError};

use crate::support::{fixtures, sample_files, Rng};

/// Wraps a WSG blob as a `.sav` file: BE length, one LZO literal run, SHA-1.
fn wrap(wsg: &[u8]) -> Vec<u8> {
    let mut body = (wsg.len() as u32).to_be_bytes().to_vec();
    body.extend_from_slice(&lzo1x::compress_literals(wsg));
    rehash(&body)
}

/// Puts a correct SHA-1 in front of `body` (the BE length and the LZO stream).
fn rehash(body: &[u8]) -> Vec<u8> {
    let mut file = sha1(body).to_vec();
    file.extend_from_slice(body);
    file
}

/// The WSG blob inside a `.sav` file.
fn unwrap_wsg(file: &[u8]) -> Vec<u8> {
    let declared = u32::from_be_bytes(file[20..24].try_into().unwrap()) as usize;
    lzo1x::decompress(&file[24..], declared).unwrap()
}

struct Wsg<'a> {
    payload: &'a [u8],
    magic: [u8; 3],
    version: u32,
    crc: Option<u32>,
    length: Option<u32>,
    trailer: [u8; 4],
    big_endian: bool,
}

impl<'a> Wsg<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Wsg { payload, magic: *b"WSG", version: 2, crc: None, length: None, trailer: [0; 4], big_endian: false }
    }

    fn build(&self) -> Vec<u8> {
        let order = |v: u32| if self.big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
        let huff = huffman::encode(self.payload);
        let mut wsg = ((15 + huff.len() + 4) as u32).to_be_bytes().to_vec();
        wsg.extend_from_slice(&self.magic);
        wsg.extend_from_slice(&order(self.version));
        wsg.extend_from_slice(&order(self.crc.unwrap_or_else(|| crc32(self.payload))));
        wsg.extend_from_slice(&order(self.length.unwrap_or(self.payload.len() as u32)));
        wsg.extend_from_slice(&huff);
        wsg.extend_from_slice(&self.trailer);
        wsg
    }

    fn file(&self) -> Vec<u8> {
        wrap(&self.build())
    }
}

const PAYLOAD: &[u8] = b"hello world hello";

#[test]
fn round_trip() {
    let mut payloads = vec![Vec::new(), vec![7], PAYLOAD.to_vec(), Rng(1).bytes(70_000)];
    payloads.extend(sample_files().iter().map(|(_, file)| codec::decode(file).unwrap()));
    for payload in payloads {
        let file = codec::encode(&payload).unwrap();
        assert_eq!(codec::decode(&file).unwrap(), payload, "{} bytes", payload.len());
    }
}

/// Format doc sections 1.1, 1.2 and 2.3: what the encoder writes, field by field.
#[test]
fn encoder_writes_the_documented_layout() {
    let payload = Rng(3).bytes(5_000);
    let file = codec::encode(&payload).unwrap();
    assert_eq!(file[..20], sha1(&file[20..]));
    assert_eq!(file[file.len() - 3..], [0x11, 0x00, 0x00], "LZO end-of-stream marker");

    let wsg = unwrap_wsg(&file);
    assert_eq!(u32::from_be_bytes(wsg[..4].try_into().unwrap()) as usize, wsg.len() - 4, "size field");
    assert_eq!(&wsg[4..7], b"WSG");
    assert_eq!(wsg[7..11], [2, 0, 0, 0], "version, little-endian");
    assert_eq!(wsg[11..15], crc32(&payload).to_le_bytes());
    assert_eq!(wsg[15..19], (payload.len() as u32).to_le_bytes());
    assert_eq!(wsg[wsg.len() - 4..], [0, 0, 0, 0], "four zero bytes after the Huffman stream");
    assert_eq!(wsg[19..wsg.len() - 4], huffman::encode(&payload));
    assert_eq!(wsg, Wsg::new(&payload).build());
}

/// The Windows game leaves non-zero bytes after the Huffman stream, and the reference
/// editor checks neither them nor the `size` field.
#[test]
fn trailing_bytes_and_size_field_are_not_checked() {
    let mut wsg = Wsg::new(PAYLOAD);
    wsg.trailer = [0xD4, 0x93, 0x9F, 0x1A];
    assert_eq!(codec::decode(&wsg.file()).unwrap(), PAYLOAD);

    let mut blob = Wsg::new(PAYLOAD).build();
    blob[..4].copy_from_slice(&12_345u32.to_be_bytes());
    assert_eq!(codec::decode(&wrap(&blob)).unwrap(), PAYLOAD);

    // No trailing bytes at all.
    let mut blob = Wsg::new(PAYLOAD).build();
    blob.truncate(blob.len() - 4);
    assert_eq!(codec::decode(&wrap(&blob)).unwrap(), PAYLOAD);
}

#[test]
fn empty_and_garbage_are_errors() {
    assert_eq!(codec::decode(&[]), Err(SaveKitError::NotASave("file too short".into())));
    let garbage: Vec<u8> = (0..10u8).map(|i| i.wrapping_mul(37).wrapping_add(5)).collect();
    assert!(codec::decode(&garbage).is_err());
    assert_eq!(codec::decode(&[0; 24]), Err(SaveKitError::HashMismatch));
    let mut rng = Rng(11);
    for _ in 0..2_000 {
        let len = rng.below(200);
        assert!(codec::decode(&rehash(&rng.bytes(len))).is_err());
    }
}

#[test]
fn oversized_lengths_are_rejected_before_any_work() {
    let mut body = (32u32 * 1024 * 1024).to_be_bytes().to_vec();
    body.extend_from_slice(&lzo1x::compress_literals(&[1; 10]));
    assert_eq!(codec::decode(&rehash(&body)), Err(SaveKitError::NotASave("declared length too large".into())));

    let mut wsg = Wsg::new(PAYLOAD);
    wsg.length = Some(u32::MAX);
    assert_eq!(codec::decode(&wsg.file()), Err(SaveKitError::NotASave("payload length too large".into())));
}

#[test]
fn encoder_refuses_a_save_the_decoder_would_reject() {
    let too_big = vec![0u8; codec::MAX_DECLARED_LEN + 1];
    assert!(matches!(codec::encode(&too_big), Err(SaveKitError::InvalidValue { .. })));
}

#[test]
fn a_flipped_byte_is_a_hash_mismatch() {
    for (name, mut file) in sample_files() {
        for at in [0, 19, 20, 23, 30, file.len() - 1] {
            file[at] ^= 0xFF;
            assert_eq!(codec::decode(&file), Err(SaveKitError::HashMismatch), "{name} byte {at}");
            file[at] ^= 0xFF;
        }
        assert!(codec::decode(&file).is_ok(), "{name}");
    }
}

#[test]
fn a_truncated_stream_with_a_good_hash_is_corrupt_compression() {
    for (name, file) in sample_files() {
        let half = 24 + (file.len() - 24) / 2;
        let result = codec::decode(&rehash(&file[20..half]));
        assert!(matches!(result, Err(SaveKitError::CorruptCompression(_))), "{name}: {result:?}");
    }
}

#[test]
fn wrong_headers_are_rejected() {
    assert_eq!(codec::decode(&Wsg::new(PAYLOAD).file()).unwrap(), PAYLOAD);

    let mut console = Wsg::new(PAYLOAD);
    console.big_endian = true;
    assert_eq!(codec::decode(&console.file()), Err(SaveKitError::UnsupportedByteOrder));

    let mut magic = Wsg::new(PAYLOAD);
    magic.magic = [1, 2, 3];
    assert_eq!(codec::decode(&magic.file()), Err(SaveKitError::NotASave("missing WSG magic".into())));

    let mut version = Wsg::new(PAYLOAD);
    version.version = 3;
    assert_eq!(codec::decode(&version.file()), Err(SaveKitError::UnsupportedVersion(3)));

    let mut crc = Wsg::new(PAYLOAD);
    crc.crc = Some(1);
    assert_eq!(codec::decode(&crc.file()), Err(SaveKitError::CrcMismatch));

    // One symbol fewer than the CRC was computed over.
    let mut length = Wsg::new(PAYLOAD);
    length.length = Some(PAYLOAD.len() as u32 - 1);
    assert_eq!(codec::decode(&length.file()), Err(SaveKitError::CrcMismatch));

    assert_eq!(codec::decode(&wrap(&[0; 18])), Err(SaveKitError::NotASave("WSG blob too short".into())));
}

/// The container of every real save: hash, declared length, WSG header.
#[test]
fn fixture_containers_are_consistent() {
    for fixture in fixtures() {
        let file = &fixture.bytes;
        assert_eq!(file[..20], sha1(&file[20..]), "{}: SHA-1", fixture.name);
        let wsg = unwrap_wsg(file);
        assert_eq!(u32::from_be_bytes(wsg[..4].try_into().unwrap()) as usize, wsg.len() - 4, "{}: size", fixture.name);
        assert_eq!(&wsg[4..7], b"WSG", "{}", fixture.name);
        assert_eq!(wsg[7..11], [2, 0, 0, 0], "{}: version", fixture.name);
        let payload = codec::decode(file).unwrap();
        assert_eq!(wsg[11..15], crc32(&payload).to_le_bytes(), "{}: CRC", fixture.name);
        assert_eq!(wsg[15..19], (payload.len() as u32).to_le_bytes(), "{}: length", fixture.name);
    }
}

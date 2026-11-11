use savekit::huffman::{decode, encode};
use savekit::SaveKitError;

use crate::support::Rng;

#[test]
fn round_trip() {
    let inputs: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![0x41],
        vec![0; 1000],
        (0..=255).collect(),
        Rng(42).bytes(50_000),
        // A skewed distribution, for long codes.
        (0..40u32).flat_map(|i| vec![i as u8; 1usize << (i / 2)]).collect(),
    ];
    for input in inputs {
        assert_eq!(decode(&encode(&input), input.len()).unwrap(), input, "{} bytes", input.len());
    }
}

/// Format doc 3.4 by hand: "aab" sorts to [b (1), a (2)], so the tree is (b, a), b = 0, a = 1.
/// Bits: 0, 1 01100010, 1 01100001, then codes 1 1 0, then two zero bits of padding.
#[test]
fn known_answer_for_a_tiny_payload() {
    assert_eq!(encode(b"aab"), [0x58, 0xAC, 0x38]);
    assert_eq!(decode(&[0x58, 0xAC, 0x38], 3).unwrap(), b"aab");
}

/// Equal weights: leaves sort by byte value, and a new internal node goes after existing
/// nodes of the same weight. "abcd" gives ((a, b), (c, d)).
#[test]
fn tie_breaking_follows_the_reference() {
    let mut bits = vec![false, false];
    leaf(&mut bits, b'a');
    leaf(&mut bits, b'b');
    bits.push(false);
    leaf(&mut bits, b'c');
    leaf(&mut bits, b'd');
    bits.extend([false, false, false, true, true, false, true, true]);
    assert_eq!(encode(b"abcd"), pack(&bits));
}

/// Appends a serialised leaf: the flag bit 1, then the byte, most significant bit first.
fn leaf(bits: &mut Vec<bool>, value: u8) {
    bits.push(true);
    bits.extend((0..8).rev().map(|i| (value >> i) & 1 == 1));
}

fn pack(bits: &[bool]) -> Vec<u8> {
    let mut bytes = vec![0u8; bits.len().div_ceil(8)];
    for (i, _) in bits.iter().enumerate().filter(|(_, &bit)| bit) {
        bytes[i / 8] |= 0x80 >> (i % 8);
    }
    bytes
}

#[test]
fn codes_longer_than_32_bits_decode() {
    // A comb of depth 40: (leaf 0, (leaf 1, (leaf 2, ...))). The deepest leaf has a 40-bit code.
    let depth = 40u8;
    let mut bits: Vec<bool> = Vec::new();
    for v in 0..depth {
        bits.push(false);
        leaf(&mut bits, v);
    }
    leaf(&mut bits, depth);
    // Symbols: the deepest leaf (40 ones), then leaf 0 (a single zero bit).
    bits.extend(vec![true; usize::from(depth)]);
    bits.push(false);
    assert_eq!(decode(&pack(&bits), 2).unwrap(), [depth, 0]);
}

#[test]
fn bits_after_the_last_symbol_are_ignored() {
    let mut stream = encode(b"hello world");
    stream.extend_from_slice(&[0xA1, 0x78, 0x9B, 0x9E, 0xD4, 0x93, 0x9F, 0x1A]);
    assert_eq!(decode(&stream, 11).unwrap(), b"hello world");
}

#[test]
fn malformed_input_is_an_error() {
    let corrupt = |r: savekit::Result<Vec<u8>>| matches!(r, Err(SaveKitError::CorruptHuffman(_)));
    // Truncated tree.
    assert!(corrupt(decode(&[0x00], 1)));
    // Empty stream.
    assert!(corrupt(decode(&[], 1)));
    assert!(corrupt(decode(&[], 0)));
    // A valid tree, but too few symbols.
    assert!(corrupt(decode(&encode(&[1, 2, 1, 2]), 100_000)));
    // Internal nodes without end: more nodes than any tree of bytes can have.
    assert!(corrupt(decode(&[0; 64], 1)));
    // A tree that is a single leaf has no codes.
    assert!(corrupt(decode(&[0b1010_0000, 0b1000_0000], 1)));
    assert_eq!(decode(&[0b1010_0000, 0b1000_0000], 0).unwrap(), []);
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(7);
    for _ in 0..20_000 {
        let len = rng.below(80);
        let junk = rng.bytes(len);
        let _ = decode(&junk, rng.below(300));
    }
}

use savekit::checksums::{crc32, sha1};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn crc32_known_answers() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
}

#[test]
fn sha1_known_answers() {
    assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(
        hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
        "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
    );
    assert_eq!(
        hex(&sha1(b"The quick brown fox jumps over the lazy dog")),
        "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
    );
    assert_eq!(hex(&sha1(&vec![b'a'; 1_000_000])), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
}

/// Lengths around the padding boundaries (55/56 bytes and the 64-byte block), checked
/// against `sha1sum`.
#[test]
fn sha1_padding_boundaries() {
    let expected = [
        (55, "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
        (56, "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
        (57, "f08f24908d682555111be7ff6f004e78283d989a"),
        (63, "03f09f5b158a7a8cdad920bddc29b81c18a551f5"),
        (64, "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
        (65, "11655326c708d70319be2610e8a57d9a5b959d3b"),
        (119, "ee971065aaa017e0632a8ca6c77bb3bf8b1dfc56"),
        (120, "f34c1488385346a55709ba056ddd08280dd4c6d6"),
        (128, "ad5b3fdbcb526778c2839d2f151ea753995e26a0"),
    ];
    for (len, digest) in expected {
        assert_eq!(hex(&sha1(&vec![b'a'; len])), digest, "{len} bytes");
    }
}

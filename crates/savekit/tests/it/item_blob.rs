use savekit::{EntropyKeys, ItemBlob, KeySource, SaveKitError};

use crate::support::{virtual_item_bytes, Rng, ITEM_CODE, ITEM_CODE_KEY_0, WEAPON_CODE, WEAPON_CODE_KEY_0};

/// Format doc 6.5.
#[test]
fn decodes_spec_vectors() {
    let weapon = ItemBlob::from_code(WEAPON_CODE).unwrap();
    assert!(weapon.is_weapon);
    assert_eq!(weapon.version, 7);
    assert_eq!(weapon.key, 1_105_057_183);
    assert_eq!(weapon.item_bytes.len(), 32);
    assert_eq!(weapon.code(), WEAPON_CODE_KEY_0);

    let item = ItemBlob::from_code(ITEM_CODE).unwrap();
    assert!(!item.is_weapon);
    assert_eq!(item.version, 7);
    assert_eq!(item.key, 1_810_808_715);
    assert_eq!(item.item_bytes.len(), 25);
    assert_eq!(item.code(), ITEM_CODE_KEY_0);

    // Each re-keyed code round-trips back to the original exactly.
    assert_eq!(ItemBlob::from_code(WEAPON_CODE_KEY_0).unwrap().rekeyed(weapon.key), weapon);
    assert_eq!(ItemBlob::from_code(ITEM_CODE_KEY_0).unwrap().rekeyed(item.key), item);
    // Space around a code is ignored.
    assert_eq!(ItemBlob::from_code(&format!("  {WEAPON_CODE}\r\n")), Ok(weapon));
}

#[test]
fn rekey_round_trip() {
    for code in [WEAPON_CODE, ITEM_CODE] {
        let blob = ItemBlob::from_code(code).unwrap();
        let original = blob.to_raw();
        for key in [0, 1, -1, 31, 32, -32, i32::MIN, i32::MAX, 1_105_057_183] {
            let rekeyed = blob.rekeyed(key);
            assert_eq!(rekeyed.key, key);
            let raw = rekeyed.to_raw();
            assert_eq!(raw[0], original[0], "header byte");
            assert_eq!(raw[1..5], key.to_be_bytes(), "the key is big-endian");
            let back = ItemBlob::from_raw(&raw).unwrap();
            assert_eq!(back.item_bytes, blob.item_bytes);
            assert_eq!(back, rekeyed);
        }
        assert_eq!(blob.rekeyed(blob.key).to_raw(), original);
    }
}

/// Known answers for negative keys, where the seed's arithmetic shift matters. They come
/// from a separate implementation of format doc 6.3 to 6.5 (a Python script written for
/// this test), which also reproduces both of the doc's key-0 vectors.
#[test]
fn rekeying_known_answers() {
    let vectors = [
        (WEAPON_CODE, -1, "BL2(h/////+FxE6ob7OF5mV3Sy/m6c6PEffv3BHxCCOe8oTlHUFuB9qt)"),
        (WEAPON_CODE, i32::MIN, "BL2(h4AAAADZy+1nLfyJLX91GJF1R4R1VlDDwLPPAu15hnn3CldHPOZA)"),
        (WEAPON_CODE, -579_283_378, "BL2(h9141k4pRd0uPpWNFD1cNoO6G0moaMJiV7UTxOICJbPTWkedHMpB)"),
        (WEAPON_CODE, i32::MAX, "BL2(h3////9Is1izqba8MgcMWp0D0MsqH6HqmEW3JxdAag7J7pirxeNu)"),
        (ITEM_CODE, -1, "BL2(B/////8Pi5UrfDIBASE3gfGK//Lga4nAcpA090OC6oU=)"),
        (ITEM_CODE, i32::MIN, "BL2(B4AAAABl3O37waYRB09n8VKlwsWaCq76wDM5JfnJe4Y=)"),
        (ITEM_CODE, -579_283_378, "BL2(B9141k5oqoHQB5UN4hpIhn5FvG6o8kgtjKX2RWblYfM=)"),
        (ITEM_CODE, i32::MAX, "BL2(B3/////C/IMIjzc41UNMkENvxvdFZd/FNsRy2E9pcg8=)"),
    ];
    for (code, key, expected) in vectors {
        let blob = ItemBlob::from_code(code).unwrap();
        let rekeyed = ItemBlob::from_code(expected).unwrap();
        assert_eq!(rekeyed, blob.rekeyed(key), "key {key}");
        assert_eq!(rekeyed.to_raw(), blob.rekeyed(key).to_raw(), "key {key}");
        assert_eq!(rekeyed.code(), blob.code(), "key {key}");
    }
}

/// Format doc 5.13: the OP level record's blob, `07 00 00 00 00 | 39 2A | FF 00x32`.
#[test]
fn virtual_item_blob_matches_the_spec() {
    let blob = ItemBlob::new(false, 7, 0, virtual_item_bytes());
    let mut expected = vec![0x07, 0, 0, 0, 0, 0x39, 0x2A, 0xFF];
    expected.extend_from_slice(&[0; 32]);
    assert_eq!(blob.to_raw(), expected);
    assert_eq!(ItemBlob::from_raw(&expected).unwrap(), blob);
}

#[test]
fn key_zero_is_plain_text() {
    let blob = ItemBlob::from_code(WEAPON_CODE).unwrap().rekeyed(0);
    let raw = blob.to_raw();
    assert_eq!(raw[0], 0x87, "weapon flag and version 7");
    assert_eq!(raw[7..], blob.item_bytes[..]);
}

#[test]
fn every_length_and_key_round_trips() {
    // Lengths on both sides of the 33-byte checksum padding, and every rotation.
    let mut rng = Rng(9);
    for len in [0, 1, 2, 30, 31, 32, 33, 34, 40, 100] {
        for _ in 0..40 {
            let blob = ItemBlob::new(rng.next() & 1 == 1, 7, rng.next() as i32, rng.bytes(len));
            assert_eq!(ItemBlob::from_raw(&blob.to_raw()).unwrap(), blob, "{len} bytes, key {}", blob.key);
        }
    }
}

#[test]
fn version_is_seven_bits() {
    let blob = ItemBlob::new(true, 0xFF, 0, vec![1, 2, 3]);
    assert_eq!(blob.version, 0x7F);
    let mut loud = blob.clone();
    loud.version = 0xFF;
    assert_eq!(loud.to_raw(), blob.to_raw());
}

#[test]
fn detects_tampered_blob() {
    for code in [WEAPON_CODE, ITEM_CODE] {
        let raw = ItemBlob::from_code(code).unwrap().to_raw();
        for at in 0..raw.len() {
            let mut bad = raw.clone();
            bad[at] ^= 0x01;
            assert_eq!(ItemBlob::from_raw(&bad), Err(SaveKitError::ItemChecksumMismatch), "byte {at}");
        }
    }
}

#[test]
fn rejects_short_blobs_and_bad_codes() {
    for len in 0..7 {
        assert_eq!(ItemBlob::from_raw(&vec![0; len]), Err(SaveKitError::ItemChecksumMismatch));
    }
    for code in ["XX(AAAA)", "BL2(AAAA", "BL2(!!!!)", "BL2", "", "nonsense", "BL2(h0Hd1Z+jY/s2Qy)", "BLOZ(AAAA)"] {
        let result = ItemBlob::from_code(code);
        assert!(matches!(result, Err(SaveKitError::InvalidValue { .. })), "{code:?}: {result:?}");
    }
    // Well-formed codes whose blob is too short or fails its checksum.
    assert_eq!(ItemBlob::from_code("BL2()"), Err(SaveKitError::ItemChecksumMismatch));
    assert_eq!(ItemBlob::from_code("BL2(AAAAAAAAAAAA)"), Err(SaveKitError::ItemChecksumMismatch));
}

#[test]
fn garbage_blobs_never_panic() {
    let mut rng = Rng(5);
    for _ in 0..20_000 {
        let len = rng.below(64);
        let _ = ItemBlob::from_raw(&rng.bytes(len));
    }
}

#[test]
fn entropy_keys_vary() {
    let mut keys = EntropyKeys::new();
    let drawn: std::collections::HashSet<i32> = (0..64).map(|_| keys.next_key()).collect();
    assert!(drawn.len() > 32, "only {} distinct keys in 64 draws", drawn.len());

    let mut fixed = || 42;
    let source: &mut dyn KeySource = &mut fixed;
    assert_eq!(source.next_key(), 42);
}

use savekit::{field, ItemBlob, ItemFields, ProtoMessage, PART_COUNT};

use crate::support::{sample_docs, virtual_item_bytes, Rng, ITEM_CODE, WEAPON_CODE};

fn decoded(code: &str) -> (ItemBlob, ItemFields) {
    let blob = ItemBlob::from_code(code).unwrap();
    let fields = ItemFields::from_blob(&blob);
    (blob, fields)
}

/// Format doc 6.5: set 0, type 1924, balance 131111, manufacturer 133, grade 50, stage 50,
/// then 11 parts, some of them 131071 (all ones).
#[test]
fn decodes_weapon_vector() {
    let (_, f) = decoded(WEAPON_CODE);
    assert_eq!(f.set, Some(0));
    assert_eq!(f.item_type, Some(1924));
    assert_eq!(f.balance, Some(131_111));
    assert_eq!(f.manufacturer, Some(133));
    assert_eq!(f.grade_index, Some(50));
    assert_eq!(f.game_stage, Some(50));
    assert!(f.parts.iter().all(Option::is_some));
    assert!(f.parts.contains(&Some(131_071)));
}

#[test]
fn decodes_item_vector() {
    let (_, f) = decoded(ITEM_CODE);
    assert_eq!(f.set, Some(0));
    assert_eq!(f.item_type, Some(2840));
    assert_eq!(f.balance, Some(27_695));
    assert_eq!(f.manufacturer, Some(137));
    assert_eq!(f.grade_index, Some(1));
    assert_eq!(f.game_stage, Some(1));
    let present = [0, 1, 65_535, 65_535, 65_535, 65_535, 52, 14].map(Some);
    assert_eq!(f.parts[..8], present);
    assert_eq!(f.parts[8..], [None; 3]);
}

#[test]
fn split_and_virtual() {
    assert_eq!(ItemFields::split(0x1234, 8), (0x12, 0x34));
    assert_eq!(ItemFields::split(2840, 8), (11, 24));
    assert_eq!(ItemFields::split(7, 0), (7, 0));
    assert_eq!(ItemFields::split(u32::MAX, 32), (0, u32::MAX));
    assert_eq!(ItemFields::split(u32::MAX, 200), (0, u32::MAX));

    let mut f = ItemFields::decode(&[0xFF], false);
    assert!(f.is_virtual());
    f.item_type = Some(1);
    assert!(!f.is_virtual());
    assert!(ItemFields::decode(&virtual_item_bytes(), false).is_virtual());
    assert!(!ItemFields::decode(&[0xFE], false).is_virtual());
    assert!(!ItemFields::decode(&[], false).is_virtual());
    assert!(!decoded(WEAPON_CODE).1.is_virtual());
}

/// A field that does not fit is absent, and so is every field after it.
#[test]
fn short_streams_end_early() {
    let bytes = Rng(4).bytes(40);
    for is_weapon in [false, true] {
        let mut last_count = 0;
        for len in 0..=bytes.len() {
            let f = ItemFields::decode(&bytes[..len], is_weapon);
            let all = [vec![f.set, f.item_type, f.balance, f.manufacturer, f.grade_index, f.game_stage], f.parts.to_vec()].concat();
            let count = all.iter().take_while(|v| v.is_some()).count();
            assert!(all[count..].iter().all(Option::is_none), "{len} bytes: a field after an absent one");
            assert!(count >= last_count);
            last_count = count;
        }
        assert_eq!(last_count, 6 + PART_COUNT);
    }
    // Item: set 8 + type 17 = 25 bits, so 3 bytes hold only the set.
    let f = ItemFields::decode(&[1, 2, 3], false);
    assert_eq!((f.set, f.item_type), (Some(1), None));
    // Weapon: 8 + 13 = 21 bits fit in 3 bytes.
    let f = ItemFields::decode(&[1, 2, 3], true);
    assert_eq!((f.set, f.item_type, f.balance), (Some(1), Some(0x0302 & 0x1FFF), None));
}

#[test]
fn reencoding_is_lossless() {
    for code in [WEAPON_CODE, ITEM_CODE] {
        let (blob, f) = decoded(code);
        assert_eq!(f.encode(blob.is_weapon, Some(&blob.item_bytes)), blob.item_bytes);
    }
    // The virtual item has 33 bytes, more than its fields fill.
    let bytes = virtual_item_bytes();
    assert_eq!(ItemFields::decode(&bytes, false).encode(false, Some(&bytes)), bytes);

    // Any bytes at all, including unused bits and bytes after the last field that fits.
    let mut rng = Rng(21);
    for len in 0..=40 {
        for is_weapon in [false, true] {
            let bytes = rng.bytes(len);
            let f = ItemFields::decode(&bytes, is_weapon);
            assert_eq!(f.encode(is_weapon, Some(&bytes)), bytes, "{len} bytes, weapon: {is_weapon}");
        }
    }

    // Every item in every sample save.
    let mut checked = 0;
    for (name, doc) in sample_docs() {
        for number in [field::ITEMS, field::WEAPONS, field::BANK] {
            for value in doc.message.values(number) {
                let entry = ProtoMessage::decode(value.as_bytes().unwrap()).unwrap();
                let Ok(blob) = ItemBlob::from_raw(entry.bytes(1).unwrap()) else { continue };
                let f = ItemFields::from_blob(&blob);
                assert_eq!(f.encode(blob.is_weapon, Some(&blob.item_bytes)), blob.item_bytes, "{name}");
                checked += 1;
            }
        }
    }
    assert!(checked >= 9);
}

/// Without an original, or when fields were removed, unused bits are zero and the
/// stream is as short as it can be.
#[test]
fn new_items_are_packed_tightly() {
    let (blob, f) = decoded(ITEM_CODE);
    // The spec item's unused bits are zero already.
    assert_eq!(f.encode(false, None), blob.item_bytes);

    let mut bytes = blob.item_bytes.clone();
    *bytes.last_mut().unwrap() |= 0xC0;
    let with_pad = ItemFields::decode(&bytes, false);
    assert_eq!(with_pad, f);
    assert_eq!(with_pad.encode(false, Some(&bytes)), bytes, "pad bits kept with the original");
    assert_eq!(with_pad.encode(false, None), blob.item_bytes, "pad bits cleared without it");

    let mut truncated = f.clone();
    truncated.parts = [None; PART_COUNT];
    let short = truncated.encode(false, Some(&bytes));
    assert_eq!(short.len(), 9, "70 bits");
    assert_eq!(short, truncated.encode(false, None));
    assert_eq!(ItemFields::decode(&short, false), truncated);

    // A gap ends the stream: fields after the first absent one are not written.
    let mut gap = f.clone();
    gap.manufacturer = None;
    assert_eq!(gap.encode(false, None).len(), 6, "45 bits");
}

#[test]
fn values_are_masked_to_their_width() {
    let (blob, mut f) = decoded(ITEM_CODE);
    f.grade_index = Some(200);
    let encoded = f.encode(false, Some(&blob.item_bytes));
    let g = ItemFields::decode(&encoded, false);
    assert_eq!(g.grade_index, Some(200 & 0x7F));
    assert_eq!(g.game_stage, f.game_stage);
    assert_eq!(g.parts, f.parts);

    let (blob, mut f) = decoded(WEAPON_CODE);
    f.item_type = Some(u32::MAX);
    f.parts[0] = Some(u32::MAX);
    let g = ItemFields::decode(&f.encode(true, Some(&blob.item_bytes)), true);
    assert_eq!(g.item_type, Some(0x1FFF));
    assert_eq!(g.parts[0], Some(0x1_FFFF));
    assert_eq!(g.balance, f.balance);
    assert_eq!(g.parts[1..], f.parts[1..]);
}

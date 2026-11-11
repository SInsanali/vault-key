use savekit::{
    field, verify, CharacterKey, EntryRef, Expectation, FallbackGearClassifier, ItemBlob, ProtoField, ProtoMessage,
    SaveDocument, SaveKitError,
};

use crate::support::{corrupt_entry_blob, sample_docs, sub, synthetic_doc};

const FALLBACK: &FallbackGearClassifier = &FallbackGearClassifier;

/// The reason of a verification failure. Anything else fails the test.
fn reason(result: savekit::Result<()>) -> String {
    match result {
        Err(SaveKitError::InvalidValue { field, reason }) if field == "verify" => reason,
        other => panic!("expected a verify error, got {other:?}"),
    }
}

fn file(doc: &SaveDocument) -> Vec<u8> {
    doc.to_file_bytes().unwrap()
}

fn first_gear(doc: &SaveDocument) -> EntryRef {
    doc.inventory().into_iter().find(|e| e.is_duplicable(FALLBACK)).unwrap().entry_ref
}

fn item_entry(seed: u8) -> ProtoField {
    let blob = ItemBlob::new(false, 7, 0, vec![seed; 24]).to_raw();
    ProtoField::new(field::ITEMS, sub(vec![ProtoField::bytes(1, blob)]))
}

#[test]
fn verifies_a_good_duplicate() {
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        doc.set_backpack_slots(39).unwrap();
        let source = first_gear(&doc);
        doc.duplicate(source, FALLBACK).unwrap();
        doc.duplicate(source, FALLBACK).unwrap();
        let expectations = [
            Expectation::Duplicated { source, count: 2 },
            Expectation::CharacterValue(CharacterKey::BackpackSlots, 39),
        ];
        assert_eq!(verify(&original, &file(&doc), &expectations), Ok(()), "{label}");
    }
}

#[test]
fn catches_a_missing_duplicate() {
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        doc.set_backpack_slots(39).unwrap();
        let source = first_gear(&doc);
        doc.duplicate(source, FALLBACK).unwrap();
        let why = reason(verify(&original, &file(&doc), &[Expectation::Duplicated { source, count: 2 }]));
        assert!(why.contains("inventory size"), "{label}: {why}");
        // And one that was never made.
        let why = reason(verify(&original, &file(&original), &[Expectation::Duplicated { source, count: 1 }]));
        assert!(why.contains("inventory size"), "{label}: {why}");
    }
}

/// The right number of new entries is not enough: they must be copies of the source.
#[test]
fn catches_a_duplicate_of_the_wrong_item() {
    let original = synthetic_doc();
    let mut doc = original.clone();
    doc.duplicate(EntryRef::new(54, 1), FALLBACK).unwrap();
    let wrong_source = [Expectation::Duplicated { source: EntryRef::new(54, 0), count: 1 }];
    let why = reason(verify(&original, &file(&doc), &wrong_source));
    assert_eq!(why, "duplicates: expected 1 new copies, found 0");
    assert_eq!(verify(&original, &file(&doc), &[Expectation::Duplicated { source: EntryRef::new(54, 1), count: 1 }]), Ok(()));

    // A source that does not exist, or cannot be decoded, in the original.
    let missing = [Expectation::Duplicated { source: EntryRef::new(54, 9), count: 0 }];
    assert!(reason(verify(&original, &file(&original), &missing)).contains("not found"));
    let mut damaged = original.clone();
    corrupt_entry_blob(&mut damaged, EntryRef::new(54, 0));
    let undecodable = [Expectation::Duplicated { source: EntryRef::new(54, 0), count: 0 }];
    assert!(reason(verify(&damaged, &file(&damaged), &undecodable)).contains("not found"));
}

#[test]
fn duplicated_checks_total_growth() {
    let mut original = SaveDocument::new(ProtoMessage::new(vec![item_entry(1), item_entry(2)]));
    original.set_backpack_slots(20).unwrap();
    let source = EntryRef::new(53, 0);
    let mut good = original.clone();
    good.duplicate(source, FALLBACK).unwrap();
    let expectations = [Expectation::Duplicated { source, count: 1 }];
    assert_eq!(verify(&original, &file(&good), &expectations), Ok(()));

    let mut extra = good.clone();
    extra.message.fields.push(item_entry(9));
    let why = reason(verify(&original, &file(&extra), &expectations));
    assert!(why.contains("expected 3") && why.contains("found 4"), "{why}");

    // Two expectations add up.
    let mut both = good.clone();
    both.duplicate(EntryRef::new(53, 1), FALLBACK).unwrap();
    let two = [Expectation::Duplicated { source, count: 1 }, Expectation::Duplicated { source: EntryRef::new(53, 1), count: 1 }];
    assert_eq!(verify(&original, &file(&both), &two), Ok(()));
    assert!(reason(verify(&original, &file(&both), &expectations)).contains("expected 3"));
}

#[test]
fn catches_a_wrong_character_value() {
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        doc.set_level(31).unwrap();
        let written = file(&doc);
        let why = reason(verify(&original, &written, &[Expectation::CharacterValue(CharacterKey::Level, 30)]));
        assert_eq!(why, "level: expected 30, found 31", "{label}");
        assert_eq!(verify(&original, &written, &[Expectation::CharacterValue(CharacterKey::Level, 31)]), Ok(()));
    }
}

#[test]
fn every_character_key_reads_its_value() {
    let mut doc = synthetic_doc();
    doc.set_level(30).unwrap();
    doc.set_currency(savekit::Currency::Money, 1).unwrap();
    doc.set_currency(savekit::Currency::Eridium, 2).unwrap();
    doc.set_currency(savekit::Currency::Seraph, 3).unwrap();
    doc.set_currency(savekit::Currency::Torgue, 4).unwrap();
    doc.set_backpack_slots(27).unwrap();
    doc.set_bank_slots(16).unwrap();
    doc.set_gun_slots(3).unwrap();
    doc.set_op_level(5).unwrap();
    doc.set_save_game_id(9).unwrap();
    let values = [30, 820_463, 1, 2, 3, 4, 27, 16, 3, 5, 9];
    let expectations: Vec<Expectation> =
        CharacterKey::ALL.into_iter().zip(values).map(|(key, value)| Expectation::CharacterValue(key, value)).collect();
    assert_eq!(verify(&synthetic_doc(), &file(&doc), &expectations), Ok(()));
    for (i, key) in CharacterKey::ALL.into_iter().enumerate() {
        assert_eq!(key.read(&doc), values[i], "{key}");
        let off_by_one = [Expectation::CharacterValue(key, values[i] + 1)];
        let why = reason(verify(&synthetic_doc(), &file(&doc), &off_by_one));
        assert_eq!(why, format!("{key}: expected {}, found {}", values[i] + 1, values[i]));
    }
}

#[test]
fn character_keys_parse_by_name() {
    let names =
        ["level", "experience", "money", "eridium", "seraph", "torgue", "backpackSlots", "bankSlots", "gunSlots", "opLevel", "saveGameID"];
    assert_eq!(CharacterKey::ALL.map(CharacterKey::name), names);
    for key in CharacterKey::ALL {
        assert_eq!(key.name().parse(), Ok(key));
        assert_eq!(key.to_string(), key.name());
    }
    assert_eq!(reason("bogus".parse::<CharacterKey>().map(|_| ())), "unknown key bogus");
    assert!("Level".parse::<CharacterKey>().is_err());
}

#[test]
fn catches_a_corrupted_file() {
    for (label, original) in sample_docs() {
        let mut written = file(&original);
        let middle = written.len() / 2;
        written[middle] ^= 0xFF;
        assert_eq!(verify(&original, &written, &[]), Err(SaveKitError::HashMismatch), "{label}");
        assert!(verify(&original, &written[..middle], &[]).is_err(), "{label}");
        assert!(verify(&original, &[], &[]).is_err(), "{label}");
    }
}

/// Whatever the expectations, an item that decoded before must still be there and decode.
#[test]
fn catches_lost_and_damaged_items() {
    let original = synthetic_doc();

    let mut damaged = original.clone();
    corrupt_entry_blob(&mut damaged, EntryRef::new(54, 0));
    let why = reason(verify(&original, &file(&damaged), &[]));
    assert_eq!(why, "item 54:0 failed its checksum in the written file");

    let mut lost = original.clone();
    let last_weapon = lost.message.fields.iter().rposition(|f| f.number == field::WEAPONS).unwrap();
    lost.message.fields.remove(last_weapon);
    let why = reason(verify(&original, &file(&lost), &[]));
    assert_eq!(why, "item 54:1 is missing from the written file");

    // An item that was already undecodable is not held against the written file.
    assert_eq!(verify(&damaged, &file(&damaged), &[Expectation::Unchanged]), Ok(()));
}

#[test]
fn unchanged_passes_for_a_plain_round_trip() {
    for (label, original) in sample_docs() {
        assert_eq!(verify(&original, &file(&original), &[Expectation::Unchanged]), Ok(()), "{label}");
        assert_eq!(verify(&original, &file(&original), &[]), Ok(()), "{label}");
    }
}

#[test]
fn unchanged_and_name_catch_changes() {
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        doc.set_level(if original.level() == 5 { 6 } else { 5 }).unwrap();
        let written = file(&doc);
        let why = reason(verify(&original, &written, &[Expectation::Unchanged]));
        assert_eq!(why, "expected the save to be unchanged, but its contents differ", "{label}");
        let why = reason(verify(&original, &written, &[Expectation::Name("Nope".into())]));
        assert_eq!(why, format!("name: expected \"Nope\", found \"{}\"", original.name()), "{label}");
        assert_eq!(verify(&original, &written, &[Expectation::Name(original.name())]), Ok(()), "{label}");
    }
}

#[test]
fn a_moved_item_passes_only_when_the_move_is_expected() {
    let original = synthetic_doc();
    let mut moved = original.clone();
    let from: EntryRef = "54:1".parse().unwrap();
    let to = moved.move_item(from, true).unwrap();
    let written = file(&moved);
    assert!(reason(verify(&original, &written, &[])).contains("missing"));
    assert_eq!(verify(&original, &written, &[Expectation::Relocated { from, to }]), Ok(()));
    // Pointing at the wrong place fails.
    let wrong = Expectation::Relocated { from, to: EntryRef::new(field::BANK, 9) };
    assert!(reason(verify(&original, &written, &[wrong])).contains("missing"));
}

use std::collections::HashMap;

use savekit::{
    field, EntryRef, FallbackGearClassifier, GearClass, GearClassifier, InventoryEntry, ItemBlob, ItemFields,
    ItemLocation, ProtoField, ProtoValue, SaveDocument, SaveKitError, PART_COUNT,
};

use crate::support::{
    corrupt_entry_blob, edit_entry, raw_blobs, round_trip, sample_docs, sub, synthetic_doc, virtual_entry, ECHO_CODE,
    GEAR_WEAPON_CODE, ITEM_CODE, ITEM_CODE_KEY_0, WEAPON_CODE, WEAPON_CODE_KEY_0,
};

const FALLBACK: &FallbackGearClassifier = &FallbackGearClassifier;

/// A classifier that gives every item the same answer.
struct Fixed(GearClass);

impl GearClassifier for Fixed {
    fn gear_class(&self, _fields: &ItemFields, _is_weapon: bool) -> GearClass {
        self.0
    }
}

fn entry(doc: &SaveDocument, at: EntryRef) -> InventoryEntry {
    doc.inventory().into_iter().find(|e| e.entry_ref == at).unwrap_or_else(|| panic!("no entry {at}"))
}

fn by_ref(doc: &SaveDocument) -> HashMap<EntryRef, InventoryEntry> {
    doc.inventory().into_iter().map(|e| (e.entry_ref, e)).collect()
}

/// An empty save with a 12-slot backpack.
fn empty_doc() -> SaveDocument {
    let mut doc = SaveDocument::default();
    doc.set_backpack_slots(12).unwrap();
    doc
}

fn expect_not_duplicable(doc: &mut SaveDocument, at: EntryRef, classifier: &dyn GearClassifier) -> String {
    let before = doc.clone();
    let result = doc.duplicate(at, classifier);
    assert_eq!(*doc, before, "a failed duplicate changed the save");
    match result {
        Err(SaveKitError::NotDuplicable(reason)) => reason,
        other => panic!("expected NotDuplicable, got {other:?}"),
    }
}

// Listing

#[test]
fn lists_the_synthetic_save() {
    let doc = synthetic_doc();
    let inventory = doc.inventory();
    let summary: Vec<(String, ItemLocation, bool, bool)> =
        inventory.iter().map(|e| (e.entry_ref.to_string(), e.location, e.is_weapon, e.is_virtual)).collect();
    use ItemLocation::{Backpack, Bank, Equipped};
    let expected = [
        ("41:0", Bank, true, false),
        ("53:0", Backpack, false, false),
        ("53:1", Equipped, false, false),
        ("53:2", Backpack, false, false),
        ("53:3", Backpack, false, true),
        ("53:4", Backpack, false, true),
        ("53:5", Backpack, false, true),
        ("54:0", Equipped, true, false),
        ("54:1", Backpack, true, false),
    ];
    assert_eq!(summary, expected.map(|(r, l, w, v)| (r.to_string(), l, w, v)));
    assert_eq!(doc.backpack_used(), 3, "virtual items take no space");

    let bank = &inventory[0];
    assert_eq!((bank.quick_slot, bank.mark, bank.quantity, bank.is_equipped), (None, None, None, false));
    let shield = &inventory[2];
    assert_eq!((shield.quantity, shield.mark, shield.is_equipped, shield.quick_slot), (Some(1), Some(1), true, None));
    let stack = &inventory[3];
    assert_eq!((stack.quantity, stack.mark), (Some(3), Some(2)));
    assert_eq!(stack.fields.as_ref().unwrap().grade_index, Some(20));
    let record = &inventory[5];
    assert_eq!(record.quantity, None, "a virtual item's subfield 2 is not a quantity");
    let equipped = &inventory[7];
    assert_eq!((equipped.quick_slot, equipped.mark, equipped.quantity), (Some(1), Some(1), None));
    assert_eq!(equipped.blob.as_ref().unwrap().key, 1_105_057_183);
    assert_eq!(inventory[8].quick_slot, Some(0));
}

#[test]
fn entry_refs_print_and_parse() {
    let at = EntryRef::new(54, 3);
    assert_eq!(at.to_string(), "54:3");
    assert_eq!("54:3".parse(), Ok(at));
    for bad in ["", "54", "54:", ":3", "54:3:1", "a:b", "-1:0", "54:-1", "54: 3"] {
        assert!(bad.parse::<EntryRef>().is_err(), "{bad:?}");
    }
}

#[test]
fn entries_with_the_wrong_wire_type_keep_their_index() {
    let mut doc = empty_doc();
    doc.message.fields.push(ProtoField::varint(field::WEAPONS, 9));
    let added = doc.import_codes(&[WEAPON_CODE], false).unwrap();
    assert_eq!(added, [EntryRef::new(field::WEAPONS, 1)]);
    let inventory = doc.inventory();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].entry_ref, added[0]);
    assert!(doc.duplicate(added[0], FALLBACK).is_ok());
}

#[test]
fn undecodable_entries_are_listed_without_a_blob() {
    let mut doc = empty_doc();
    doc.message.fields.push(ProtoField::bytes(field::WEAPONS, vec![0xFF, 0xFF]));
    doc.message.fields.push(ProtoField::new(field::ITEMS, sub(vec![ProtoField::varint(2, 1)])));
    doc.message.fields.push(ProtoField::new(field::BANK, sub(vec![ProtoField::bytes(1, vec![7, 0, 0])])));
    let inventory = doc.inventory();
    assert_eq!(inventory.len(), 3);
    for e in &inventory {
        assert!(e.blob.is_none() && e.fields.is_none() && !e.is_virtual && !e.is_exportable());
        assert!(!e.is_duplicable(FALLBACK));
        assert_eq!(e.is_weapon, e.entry_ref.field_number == field::WEAPONS, "guessed from the field");
    }
    assert!(doc.export_codes(true).is_empty());
    assert_eq!(doc.backpack_used(), 2, "an unreadable entry still takes a slot");
}

// Export and import

#[test]
fn export_lists_bank_then_items_then_weapons() {
    let doc = synthetic_doc();
    let echo = ItemBlob::from_code(ECHO_CODE).unwrap().code();
    let level_20 = crate::support::level_20_item().code();
    let gear = ItemBlob::from_code(GEAR_WEAPON_CODE).unwrap().code();
    assert_eq!(
        doc.export_codes(true),
        [WEAPON_CODE_KEY_0, echo.as_str(), ITEM_CODE_KEY_0, level_20.as_str(), WEAPON_CODE_KEY_0, gear.as_str()],
        "virtual items are not exported"
    );
    assert_eq!(doc.export_codes(false), doc.export_codes(true)[1..]);
}

#[test]
fn export_import_round_trip() {
    for (label, mut doc) in sample_docs() {
        let codes = doc.export_codes(false);
        assert!(!codes.is_empty(), "{label}");
        let before = doc.message.count(field::ITEMS) + doc.message.count(field::WEAPONS);
        let added = doc.import_codes(&codes, false).unwrap();
        assert_eq!(added.len(), codes.len(), "{label}");
        assert_eq!(doc.message.count(field::ITEMS) + doc.message.count(field::WEAPONS), before + codes.len());
        let entries = by_ref(&doc);
        let imported: Vec<String> = added.iter().map(|at| entries[at].blob.as_ref().unwrap().code()).collect();
        assert_eq!(imported, codes, "{label}");
        for at in &added {
            assert_eq!(entries[at].location, ItemLocation::Backpack, "{label}");
        }
        // It survives a file round trip.
        let again = round_trip(&doc);
        assert_eq!(again.inventory(), doc.inventory(), "{label}");
    }
}

/// Format doc 5.21.
#[test]
fn import_defaults_and_errors() {
    let mut doc = SaveDocument::default();
    let added = doc.import_codes(&[WEAPON_CODE, ITEM_CODE], false).unwrap();
    assert_eq!(added, [EntryRef::new(54, 0), EntryRef::new(53, 0)]);
    let inventory = doc.inventory();
    assert_eq!(inventory.len(), 2);
    let weapon = inventory.iter().find(|e| e.is_weapon).unwrap();
    assert_eq!((weapon.quick_slot, weapon.mark, weapon.location), (Some(0), Some(1), ItemLocation::Backpack));
    let item = inventory.iter().find(|e| !e.is_weapon).unwrap();
    assert_eq!((item.quantity, item.mark, item.is_equipped), (Some(1), Some(1), false));
    assert_eq!(doc.backpack_used(), 2);
    // Fields are written in number order even though the weapon came first.
    assert_eq!(doc.message.fields.iter().map(|f| f.number).collect::<Vec<_>>(), [53, 54]);
    // The submessages are exactly the reference editor's.
    let weapon_entry = crate::support::submessage(&doc, field::WEAPONS);
    assert_eq!(weapon_entry.fields[1..], [ProtoField::varint(2, 0), ProtoField::varint(3, 1)]);
    let item_entry = crate::support::submessage(&doc, field::ITEMS);
    assert_eq!(item_entry.fields[1..], [ProtoField::varint(2, 1), ProtoField::varint(3, 0), ProtoField::varint(4, 1)]);

    let banked = doc.import_codes(&[ITEM_CODE, WEAPON_CODE], true).unwrap();
    assert_eq!(banked, [EntryRef::new(41, 0), EntryRef::new(41, 1)]);
    let bank_entry = crate::support::submessage(&doc, field::BANK);
    assert_eq!(bank_entry.fields.len(), 1, "a bank entry is only the blob");
    let in_bank: Vec<bool> =
        doc.inventory().iter().filter(|e| e.location == ItemLocation::Bank).map(|e| e.is_weapon).collect();
    assert_eq!(in_bank, [false, true], "the bank holds items and weapons, told apart by the header bit");

    // One bad code and nothing is imported.
    let before = doc.clone();
    assert!(doc.import_codes(&[ITEM_CODE, "nonsense"], false).is_err());
    assert_eq!(doc, before);
    assert_eq!(doc.import_codes::<&str>(&[], false), Ok(Vec::new()));
    assert_eq!(doc, before);
}

#[test]
fn imports_take_their_keys_from_the_key_source() {
    let mut doc = SaveDocument::default();
    let mut next = 100;
    let mut keys = || {
        next += 1;
        next
    };
    let added = doc.import_codes_with(&[WEAPON_CODE.to_string(), ITEM_CODE.to_string()], false, &mut keys).unwrap();
    let entries = by_ref(&doc);
    assert_eq!(entries[&added[0]].blob.as_ref().unwrap().key, 101);
    assert_eq!(entries[&added[1]].blob.as_ref().unwrap().key, 102);
    assert_eq!(raw_blobs(&doc, field::WEAPONS)[0][1..5], 101i32.to_be_bytes());

    // The default source gives the two copies of one code different keys.
    let twice = doc.import_codes(&[WEAPON_CODE; 8], true).unwrap();
    let entries = by_ref(&doc);
    let keys: std::collections::HashSet<i32> = twice.iter().map(|at| entries[at].blob.as_ref().unwrap().key).collect();
    assert!(keys.len() > 1);
}

// Item levels

#[test]
fn item_levels_change_only_levelled_gear_outside_the_bank() {
    let mut any_levelled = false;
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        doc.set_item_levels(25, false).unwrap();
        let old = by_ref(&original);
        for e in doc.inventory() {
            let o = &old[&e.entry_ref];
            let grade = o.fields.as_ref().and_then(|f| f.grade_index).unwrap_or(0);
            if e.location == ItemLocation::Bank || o.is_virtual || o.fields.is_none() || grade <= 1 {
                assert_eq!(&e, o, "{label}: {} should not change", e.entry_ref);
                continue;
            }
            let (fields, old_fields) = (e.fields.as_ref().unwrap(), o.fields.as_ref().unwrap());
            assert_eq!((fields.grade_index, fields.game_stage), (Some(25), Some(25)), "{label}: {}", e.entry_ref);
            assert_eq!(e.blob.as_ref().unwrap().key, o.blob.as_ref().unwrap().key, "{label}: key kept");
            // Nothing but the level changes.
            let mut expected = old_fields.clone();
            expected.grade_index = Some(25);
            expected.game_stage = Some(25);
            assert_eq!(fields, &expected, "{label}: {}", e.entry_ref);
            assert_eq!(InventoryEntry { blob: None, fields: None, ..e.clone() }, InventoryEntry { blob: None, fields: None, ..o.clone() });
            any_levelled = true;
        }
        // Every blob still passes its checksum after a file round trip.
        assert_eq!(round_trip(&doc).inventory(), doc.inventory(), "{label}");
    }
    assert!(any_levelled);

    let doc = {
        let mut doc = synthetic_doc();
        doc.set_item_levels(25, false).unwrap();
        doc
    };
    let grades: Vec<Option<u32>> =
        doc.inventory().iter().map(|e| e.fields.as_ref().and_then(|f| f.grade_index)).collect();
    assert_eq!(
        grades,
        [50, 0, 1, 25, 0, 0, 0, 25, 1].map(Some),
        "unchanged: the bank (50), the ECHO (0), the virtual items (0) and the level-1 shield and pistol"
    );
}

#[test]
fn forced_item_levels_skip_virtual_items_and_the_bank() {
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        doc.set_item_levels(25, true).unwrap();
        let old = by_ref(&original);
        for e in doc.inventory() {
            let o = &old[&e.entry_ref];
            if o.is_virtual || o.blob.is_none() || o.location == ItemLocation::Bank {
                assert_eq!(&e, o, "{label}: {}", e.entry_ref);
            } else if o.fields.as_ref().unwrap().grade_index.is_some() {
                assert_eq!(e.fields.as_ref().unwrap().grade_index, Some(25), "{label}: {}", e.entry_ref);
            }
        }
        for number in [field::BANK, field::ITEMS] {
            let (before, after) = (raw_blobs(&original, number), raw_blobs(&doc, number));
            for e in original.inventory().iter().filter(|e| e.entry_ref.field_number == number) {
                if e.is_virtual || number == field::BANK {
                    assert_eq!(before[e.entry_ref.index], after[e.entry_ref.index], "{label}: {}", e.entry_ref);
                }
            }
        }
    }
}

#[test]
fn item_level_zero_means_the_character_level() {
    let mut doc = synthetic_doc();
    doc.set_level(31).unwrap();
    doc.set_item_levels(0, false).unwrap();
    assert_eq!(entry(&doc, EntryRef::new(54, 0)).fields.unwrap().grade_index, Some(31));
    assert_eq!(entry(&doc, EntryRef::new(53, 1)).fields.unwrap().grade_index, Some(1), "level 1 needs force");
    doc.set_item_levels(0, true).unwrap();
    assert_eq!(entry(&doc, EntryRef::new(53, 1)).fields.unwrap().grade_index, Some(31));

    for bad in [-1, 128] {
        let before = doc.clone();
        assert!(matches!(doc.set_item_levels(bad, false), Err(SaveKitError::InvalidValue { .. })));
        assert_eq!(doc, before);
    }
    // A character level that does not fit in 7 bits is refused, not masked.
    doc.message.set_single(field::LEVEL, ProtoValue::Varint(200));
    assert!(doc.set_item_levels(0, true).is_err());
    doc.set_item_levels(127, true).unwrap();
    assert_eq!(entry(&doc, EntryRef::new(54, 1)).fields.unwrap().game_stage, Some(127));
}

// OP level

#[test]
fn op_level_read_back() {
    for (label, mut doc) in sample_docs() {
        let count = doc.inventory().len();
        doc.set_op_level(3).unwrap();
        assert_eq!(doc.op_level(), 3, "{label}");
        assert!(doc.playthroughs_unlocked() >= 2, "{label}: OP levels need UVHM");
        assert_eq!(round_trip(&doc).op_level(), 3, "{label}");
        doc.set_op_level(10).unwrap();
        assert_eq!(doc.op_level(), 10, "{label}");
        doc.set_op_level(0).unwrap();
        assert_eq!(doc.op_level(), 0, "{label}");
        assert!(doc.inventory().len() <= count + 1, "{label}: at most one record is created");
        for bad in [-1, 11] {
            let before = doc.clone();
            assert!(matches!(doc.set_op_level(bad), Err(SaveKitError::InvalidValue { .. })), "{label}");
            assert_eq!(doc, before, "{label}");
        }
    }
}

/// Format doc 5.13: the payload is `-(4 | level << 8)` as a 10-byte varint, in the first
/// virtual item with id 4. The other virtual items are left alone.
#[test]
fn op_level_updates_the_existing_record() {
    let original = synthetic_doc();
    let mut doc = original.clone();
    assert_eq!(doc.op_level(), 0);
    doc.set_op_level(3).unwrap();
    assert_eq!(doc.inventory().len(), original.inventory().len());
    let record = |d: &SaveDocument, index: usize| {
        let value = d.message.values(field::ITEMS).nth(index).unwrap();
        savekit::ProtoMessage::decode(value.as_bytes().unwrap()).unwrap()
    };
    assert_eq!(record(&doc, 4).varint(2), Some(-(4i64 | (3 << 8)) as u64));
    assert_eq!(record(&doc, 4).varint(2), Some(0xFFFF_FFFF_FFFF_FCFC));
    assert_eq!(record(&doc, 3), record(&original, 3));
    assert_eq!(record(&doc, 5), record(&original, 5));
    assert_eq!(raw_blobs(&doc, field::ITEMS), raw_blobs(&original, field::ITEMS));
    assert_eq!(doc.playthroughs_unlocked(), 2);

    // Level 0 does not unlock UVHM.
    let mut zero = original.clone();
    zero.set_op_level(0).unwrap();
    assert_eq!(zero, original);
}

#[test]
fn op_level_creates_the_record_when_missing() {
    let mut doc = SaveDocument::default();
    assert_eq!(doc.op_level(), 0);
    doc.set_op_level(3).unwrap();
    assert_eq!(doc.op_level(), 3);
    assert_eq!(doc.playthroughs_unlocked(), 2);
    let inventory = doc.inventory();
    assert_eq!(inventory.len(), 1);
    let record = &inventory[0];
    assert!(record.is_virtual && record.blob.is_some() && !record.is_duplicable(FALLBACK));

    // Exactly the entry the format doc describes: {1: B, 2: v, 3: 0, 4: 0}.
    let mut blob = vec![0x07, 0, 0, 0, 0, 0x39, 0x2A, 0xFF];
    blob.extend_from_slice(&[0; 32]);
    let expected = vec![
        ProtoField::bytes(1, blob),
        ProtoField::varint(2, -(4i64 | (3 << 8)) as u64),
        ProtoField::varint(3, 0),
        ProtoField::varint(4, 0),
    ];
    assert_eq!(crate::support::submessage(&doc, field::ITEMS).fields, expected);

    doc.set_op_level(7).unwrap();
    assert_eq!((doc.op_level(), doc.inventory().len()), (7, 1));
}

#[test]
fn op_level_ignores_other_virtual_items_and_real_items() {
    // Records with ids 3 and 5 only, a real item whose subfield 2 looks like id 4, and a
    // record with id 4 but a large value.
    let mut doc = SaveDocument::default();
    doc.message.fields.push(ProtoField::new(field::ITEMS, virtual_entry(3, 9)));
    doc.message.fields.push(ProtoField::new(field::ITEMS, virtual_entry(5, 9)));
    doc.import_codes(&[ITEM_CODE], false).unwrap();
    edit_entry(&mut doc, EntryRef::new(53, 2), |m| m.set_single(2, ProtoValue::Varint(-(4i64 | (6 << 8)) as u64)));
    assert_eq!(doc.op_level(), 0);
    doc.set_op_level(2).unwrap();
    assert_eq!((doc.op_level(), doc.inventory().len()), (2, 4));

    let mut odd = SaveDocument::default();
    odd.message.fields.push(ProtoField::new(field::ITEMS, virtual_entry(4, 123_456)));
    assert_eq!(odd.op_level(), 123_456);
    // Nonsense payloads read as nonsense numbers, without trapping.
    for payload in [0, 1, u64::MAX, 1 << 63, (1 << 63) + 4] {
        edit_entry(&mut odd, EntryRef::new(53, 0), |m| m.set_single(2, ProtoValue::Varint(payload)));
        let _ = odd.op_level();
        let mut copy = odd.clone();
        copy.set_op_level(1).unwrap();
        assert_eq!(copy.op_level(), 1);
    }
}

// Duplicate Item

#[test]
fn duplicate_weapon_creates_an_identical_copy() {
    for (label, mut doc) in sample_docs() {
        doc.set_backpack_slots(39).unwrap();
        let inventory = doc.inventory();
        let source = inventory.iter().find(|e| e.is_weapon && e.is_duplicable(FALLBACK)).unwrap();
        let source_blob = source.blob.as_ref().unwrap();
        let before = doc.clone();

        let at = doc.duplicate(source.entry_ref, FALLBACK).unwrap();
        assert_eq!(doc.inventory().len(), inventory.len() + 1, "{label}");
        assert_eq!(at, EntryRef::new(field::WEAPONS, before.message.count(field::WEAPONS)), "{label}");
        let copy = entry(&doc, at);
        assert_eq!((copy.location, copy.quick_slot), (ItemLocation::Backpack, Some(0)), "{label}");
        let copy_blob = copy.blob.as_ref().unwrap();
        assert_eq!(copy_blob.item_bytes, source_blob.item_bytes, "{label}");
        assert_ne!(copy_blob.key, source_blob.key, "{label}");
        assert_eq!(copy.fields, source.fields, "{label}");
        assert_eq!(copy.mark, Some(source.mark.unwrap_or(1)), "{label}");

        // Everything that was there is still there, unchanged and in the same place.
        let after = by_ref(&doc);
        for e in &inventory {
            assert_eq!(&after[&e.entry_ref], e, "{label}");
        }
        let mut without_copy = doc.clone();
        let position = without_copy.message.fields.iter().rposition(|f| f.number == field::WEAPONS).unwrap();
        without_copy.message.fields.remove(position);
        assert_eq!(without_copy, before, "{label}: the copy is the last weapon, and the only change");

        let again = round_trip(&doc);
        let (a, b) = (entry(&again, source.entry_ref), entry(&again, at));
        assert_eq!(a.blob.as_ref().unwrap().item_bytes, b.blob.as_ref().unwrap().item_bytes, "{label}");
        assert_ne!(a.blob.unwrap().key, b.blob.unwrap().key, "{label}");
    }
}

#[test]
fn duplicate_uses_the_key_source() {
    let mut doc = synthetic_doc();
    let source = EntryRef::new(54, 0);
    let source_key = entry(&doc, source).blob.unwrap().key;

    let at = doc.duplicate_with(source, FALLBACK, &mut || 7).unwrap();
    assert_eq!(entry(&doc, at).blob.unwrap().key, 7);

    // A source that keeps offering the original's key is not followed, and does not hang.
    let mut calls = 0;
    let mut stubborn = || {
        calls += 1;
        source_key
    };
    let at = doc.duplicate_with(source, FALLBACK, &mut stubborn).unwrap();
    assert!(calls >= 1);
    let copy = entry(&doc, at).blob.unwrap();
    assert_ne!(copy.key, source_key);
    assert_eq!(copy.item_bytes, entry(&doc, source).blob.unwrap().item_bytes);

    // The first key that differs is used.
    let mut offers = [source_key, source_key, 55].into_iter();
    let at = doc.duplicate_with(source, FALLBACK, &mut || offers.next().unwrap()).unwrap();
    assert_eq!(entry(&doc, at).blob.unwrap().key, 55);
}

#[test]
fn duplicate_of_an_equipped_or_bank_item_lands_in_the_backpack() {
    for (label, mut doc) in sample_docs() {
        doc.set_backpack_slots(39).unwrap();
        // A bank item, and a weapon equipped in quick slot 1.
        let bank = doc.import_codes(&[ITEM_CODE], true).unwrap()[0];
        let weapon = doc.import_codes(&[WEAPON_CODE], false).unwrap()[0];
        edit_entry(&mut doc, weapon, |m| m.set_single(2, ProtoValue::Varint(1)));
        let equipped = entry(&doc, weapon);
        assert_eq!(equipped.location, ItemLocation::Equipped, "{label}");
        let banked = entry(&doc, bank);
        assert_eq!(banked.location, ItemLocation::Bank, "{label}");

        for source in [banked, equipped] {
            let at = doc.duplicate(source.entry_ref, FALLBACK).unwrap();
            let copy = entry(&doc, at);
            assert_eq!(copy.location, ItemLocation::Backpack, "{label}");
            assert!(!copy.is_equipped && copy.quick_slot.unwrap_or(0) == 0, "{label}");
            assert_eq!(copy.is_weapon, source.is_weapon, "{label}");
            assert_eq!(copy.blob.as_ref().unwrap().item_bytes, source.blob.as_ref().unwrap().item_bytes, "{label}");
            assert_ne!(copy.blob.as_ref().unwrap().key, source.blob.as_ref().unwrap().key, "{label}");
            assert_eq!(entry(&doc, source.entry_ref), source, "{label}: the source is untouched");
        }
    }
}

#[test]
fn duplicate_into_a_full_backpack_is_refused() {
    for (label, mut doc) in sample_docs() {
        let source = doc.inventory().into_iter().find(|e| e.is_weapon && e.is_duplicable(FALLBACK)).unwrap();
        doc.set_backpack_slots(12).unwrap();
        while (doc.backpack_used() as i64) < doc.backpack_slots() {
            doc.import_codes(&[ITEM_CODE], false).unwrap();
        }
        let used = doc.backpack_used();
        assert!(used >= 12, "{label}");
        let before = doc.clone();
        assert_eq!(doc.duplicate(source.entry_ref, FALLBACK), Err(SaveKitError::BackpackFull), "{label}");
        assert_eq!(doc, before, "{label}");

        // With room it fits.
        assert!(used < 39, "{label}");
        doc.set_backpack_slots(39).unwrap();
        doc.duplicate(source.entry_ref, FALLBACK).unwrap();
        assert_eq!(doc.backpack_used(), used + 1, "{label}");
    }
}

/// A story item has no manufacturer; the game removes copies of some of them.
#[test]
fn story_item_without_a_manufacturer_is_not_duplicable() {
    let mut fields = ItemFields::from_blob(&ItemBlob::from_code(ITEM_CODE).unwrap());
    fields.item_type = Some((98 << 8) | 3);
    fields.balance = Some((114 << 8) | 3);
    fields.manufacturer = Some(2047);
    fields.grade_index = Some(0);
    fields.game_stage = Some(0);
    fields.parts = [None; PART_COUNT];
    let blob = ItemBlob::new(false, 7, 0, fields.encode(false, None));

    let mut doc = empty_doc();
    let at = doc.import_codes(&[blob.code()], false).unwrap()[0];
    let e = entry(&doc, at);
    assert_eq!(e.fields.as_ref().unwrap().manufacturer, Some(2047));
    assert!(e.is_exportable() && !e.is_duplicable(FALLBACK));
    assert_eq!(expect_not_duplicable(&mut doc, at, FALLBACK), "not a gear item (story/mission/usable item)");
}

#[test]
fn fallback_classifier_needs_a_real_manufacturer() {
    let fields = |code: &str| ItemFields::from_blob(&ItemBlob::from_code(code).unwrap());
    assert_eq!(FALLBACK.gear_class(&fields(GEAR_WEAPON_CODE), true), GearClass::Gear);
    assert_eq!(FALLBACK.gear_class(&fields(WEAPON_CODE), true), GearClass::Gear);
    assert_eq!(FALLBACK.gear_class(&fields(ITEM_CODE), false), GearClass::Gear);
    let echo = fields(ECHO_CODE);
    assert_eq!(echo.manufacturer, Some(2047));
    assert_eq!(FALLBACK.gear_class(&echo, false), GearClass::NotGear);
    // Too short to have a manufacturer at all.
    assert_eq!(FALLBACK.gear_class(&ItemFields::decode(&[0, 1, 2], false), false), GearClass::NotGear);
}

/// In every sample save: virtual items, undecodable items and items without a manufacturer
/// cannot be duplicated; everything else can.
#[test]
fn only_gear_is_duplicable() {
    for (label, doc) in sample_docs() {
        let mut gear = 0;
        for e in doc.inventory() {
            let manufacturer = e.fields.as_ref().and_then(|f| f.manufacturer);
            let expected = e.blob.is_some() && !e.is_virtual && manufacturer.is_some_and(|m| m != 2047);
            assert_eq!(e.is_duplicable(FALLBACK), expected, "{label}: {}", e.entry_ref);
            assert!(!e.is_duplicable(&Fixed(GearClass::NotGear)) && !e.is_duplicable(&Fixed(GearClass::Unknown)));
            assert_eq!(e.is_duplicable(&Fixed(GearClass::Gear)), e.is_exportable(), "{label}: {}", e.entry_ref);
            gear += usize::from(expected);
        }
        assert!(gear > 0, "{label}");
    }
    let doc = synthetic_doc();
    let duplicable: Vec<String> =
        doc.inventory().iter().filter(|e| e.is_duplicable(FALLBACK)).map(|e| e.entry_ref.to_string()).collect();
    assert_eq!(duplicable, ["41:0", "53:1", "53:2", "54:0", "54:1"], "not the ECHO (53:0) or the virtual items");
}

#[test]
fn virtual_and_corrupt_items_are_not_duplicable() {
    for (label, mut doc) in sample_docs() {
        doc.set_backpack_slots(39).unwrap();
        for e in doc.inventory().iter().filter(|e| e.is_virtual) {
            assert!(!e.is_duplicable(FALLBACK) && !e.is_duplicable(&Fixed(GearClass::Gear)), "{label}");
            assert_eq!(
                expect_not_duplicable(&mut doc, e.entry_ref, &Fixed(GearClass::Gear)),
                "virtual items cannot be duplicated",
                "{label}"
            );
        }
        // Damage the first weapon: flip a bit after the 5-byte header.
        let target = doc.inventory().into_iter().find(|e| e.entry_ref.field_number == field::WEAPONS && e.blob.is_some());
        let target = target.unwrap().entry_ref;
        corrupt_entry_blob(&mut doc, target);
        let damaged = entry(&doc, target);
        assert!(damaged.blob.is_none() && damaged.is_weapon, "{label}");
        assert!(!damaged.is_duplicable(FALLBACK), "{label}");
        assert_eq!(expect_not_duplicable(&mut doc, target, FALLBACK), "item data could not be decoded", "{label}");
    }
}

#[test]
fn duplicate_of_a_missing_entry_is_refused() {
    let mut doc = empty_doc();
    doc.set_op_level(3).unwrap();
    for at in [EntryRef::new(54, 99), EntryRef::new(53, 1), EntryRef::new(41, 0), EntryRef::new(7, 0)] {
        assert_eq!(expect_not_duplicable(&mut doc, at, FALLBACK), "no such inventory entry");
    }
}

#[test]
fn duplicate_keeps_quantity_and_mark() {
    let mut doc = empty_doc();
    let added = doc.import_codes(&[ITEM_CODE, WEAPON_CODE], false).unwrap();
    edit_entry(&mut doc, added[0], |m| {
        m.set_single(2, ProtoValue::Varint(5));
        m.set_single(3, ProtoValue::Varint(1));
        m.set_single(4, ProtoValue::Varint(2));
    });
    edit_entry(&mut doc, added[1], |m| {
        m.set_single(2, ProtoValue::Varint(3));
        m.set_single(3, ProtoValue::Varint(0));
        m.set_single(4, ProtoValue::Varint(1));
    });

    let item = doc.duplicate(added[0], FALLBACK).unwrap();
    assert_eq!(item, EntryRef::new(53, 1));
    let copy = entry(&doc, item);
    assert_eq!((copy.quantity, copy.mark, copy.is_equipped), (Some(5), Some(2), false));
    assert_eq!(copy.location, ItemLocation::Backpack);

    let weapon = doc.duplicate(added[1], FALLBACK).unwrap();
    assert_eq!(weapon, EntryRef::new(54, 1));
    let copy = entry(&doc, weapon);
    assert_eq!((copy.quick_slot, copy.mark, copy.location), (Some(0), Some(0), ItemLocation::Backpack));
    // The source's unknown subfield 4 is not copied: a new weapon entry has subfields 1 to 3.
    assert_eq!(crate::support::raw_blobs(&doc, field::WEAPONS).len(), 2);
    let copy_message = doc.message.values(field::WEAPONS).nth(1).unwrap();
    let numbers: Vec<u32> =
        savekit::ProtoMessage::decode(copy_message.as_bytes().unwrap()).unwrap().fields.iter().map(|f| f.number).collect();
    assert_eq!(numbers, [1, 2, 3]);
}

#[test]
fn duplicate_appears_in_export_after_reload() {
    for (label, mut doc) in sample_docs() {
        doc.set_backpack_slots(39).unwrap();
        let source = doc.inventory().into_iter().find(|e| e.is_weapon && e.is_duplicable(FALLBACK)).unwrap();
        let code = source.blob.as_ref().unwrap().code();
        let count = |d: &SaveDocument| d.export_codes(true).iter().filter(|c| **c == code).count();
        let before = count(&doc);
        doc.duplicate(source.entry_ref, FALLBACK).unwrap();
        assert_eq!(count(&round_trip(&doc)), before + 1, "{label}");
    }
}

#[test]
fn huge_varint_subfields_do_not_trap() {
    let mut doc = empty_doc();
    doc.set_backpack_slots(39).unwrap();
    let weapon = doc.import_codes(&[WEAPON_CODE], false).unwrap()[0];
    let item = doc.import_codes(&[ITEM_CODE], false).unwrap()[0];
    for at in [weapon, item] {
        edit_entry(&mut doc, at, |m| {
            for number in 2..=4 {
                m.set_single(number, ProtoValue::Varint(u64::MAX));
            }
        });
    }
    let inventory = doc.inventory();
    assert_eq!(inventory.len(), 2);
    assert_eq!(inventory[0].quantity, Some(-1));
    assert_eq!(inventory[1].quick_slot, Some(-1));
    for at in [weapon, item] {
        let copy = doc.duplicate(at, FALLBACK).unwrap();
        // The copy carries the same bits: -1 goes back out as a 10-byte varint.
        assert_eq!(entry(&doc, copy).mark, Some(-1));
    }
    assert_eq!(round_trip(&doc).inventory().len(), 4);
}

#[test]
fn duplicate_respects_the_classifier() {
    for (label, mut doc) in sample_docs() {
        doc.set_backpack_slots(39).unwrap();
        let source = doc.inventory().into_iter().find(|e| e.is_weapon && e.is_exportable()).unwrap().entry_ref;
        assert_eq!(
            expect_not_duplicable(&mut doc, source, &Fixed(GearClass::NotGear)),
            "not a gear item (story/mission/usable item)",
            "{label}"
        );
        assert_eq!(
            expect_not_duplicable(&mut doc, source, &Fixed(GearClass::Unknown)),
            "unrecognized item; import game data or check the item",
            "{label}"
        );
        let before = doc.clone();
        doc.duplicate(source, &Fixed(GearClass::Gear)).unwrap();
        assert_ne!(doc, before, "{label}");
    }
}

/// The classifier sees the item's decoded fields and whether it is a weapon.
#[test]
fn classifier_receives_the_item() {
    struct OnlyWeaponType(u32);
    impl GearClassifier for OnlyWeaponType {
        fn gear_class(&self, fields: &ItemFields, is_weapon: bool) -> GearClass {
            if is_weapon && fields.item_type == Some(self.0) {
                GearClass::Gear
            } else {
                GearClass::Unknown
            }
        }
    }
    let mut doc = synthetic_doc();
    let classifier = OnlyWeaponType(1924);
    let duplicable: Vec<String> =
        doc.inventory().iter().filter(|e| e.is_duplicable(&classifier)).map(|e| e.entry_ref.to_string()).collect();
    assert_eq!(duplicable, ["41:0", "54:0"], "the two copies of the spec weapon");
    doc.duplicate(EntryRef::new(54, 0), &classifier).unwrap();
    expect_not_duplicable(&mut doc, EntryRef::new(54, 1), &classifier);
}

// One item's level

fn level_of(doc: &SaveDocument, at: EntryRef) -> (Option<u32>, Option<u32>) {
    let fields = entry(doc, at).fields.unwrap();
    (fields.grade_index, fields.game_stage)
}

#[test]
fn one_item_level_changes_that_item_wherever_it_is() {
    let doc = synthetic_doc();
    let others = |doc: &SaveDocument, except: EntryRef| -> Vec<InventoryEntry> {
        doc.inventory().into_iter().filter(|e| e.entry_ref != except).collect()
    };
    // Backpack, equipped and bank alike, and a level-1 item too.
    for at in ["53:2", "54:0", "41:0", "54:1"] {
        let at: EntryRef = at.parse().unwrap();
        let mut levelled = doc.clone();
        levelled.set_item_level(at, 61).unwrap();
        let again = round_trip(&levelled);
        assert_eq!(level_of(&again, at), (Some(61), Some(61)), "{at}");
        let (before, after) = (entry(&doc, at), entry(&again, at));
        assert_eq!(after.blob.as_ref().unwrap().key, before.blob.as_ref().unwrap().key, "{at}: the key is kept");
        assert_eq!((after.location, after.mark, after.quantity), (before.location, before.mark, before.quantity), "{at}");
        let mut parts = after.fields.clone().unwrap();
        let mut was = before.fields.clone().unwrap();
        (parts.grade_index, parts.game_stage, was.grade_index, was.game_stage) = (None, None, None, None);
        assert_eq!(parts, was, "{at}: only the level changed");
        assert_eq!(others(&again, at), others(&doc, at), "{at}: nothing else changed");
    }
}

#[test]
fn one_item_level_refusals_change_nothing() {
    let doc = synthetic_doc();
    let refused = |at: &str, level: i64| {
        let mut copy = doc.clone();
        let error = copy.set_item_level(at.parse().unwrap(), level).unwrap_err();
        assert_eq!(copy, doc, "{at} {level}");
        match error {
            SaveKitError::InvalidValue { field, reason } => format!("{field}: {reason}"),
            other => panic!("{other:?}"),
        }
    };
    assert!(refused("53:3", 30).contains("placeholder"));
    assert!(refused("54:9", 30).contains("no item"));
    assert!(refused("13:0", 30).contains("not an inventory entry"));
    assert!(refused("53:2", 0).starts_with("itemLevel"));
    assert!(refused("53:2", 128).starts_with("itemLevel"));
    let mut corrupt = doc.clone();
    corrupt_entry_blob(&mut corrupt, "54:1".parse().unwrap());
    let before = corrupt.clone();
    assert!(corrupt.set_item_level("54:1".parse().unwrap(), 30).is_err());
    assert_eq!(corrupt, before);
}

// Moving between the backpack and the bank

#[test]
fn moving_to_the_bank_and_back_keeps_the_item_byte_for_byte() {
    for (label, doc) in sample_docs() {
        let mut doc = doc;
        let at = doc.import_codes(&[WEAPON_CODE], false).unwrap()[0];
        let raw_before = raw_blobs(&doc, field::WEAPONS)[at.index].clone();
        let banks_before = raw_blobs(&doc, field::BANK).len();

        let in_bank = doc.move_item(at, true).unwrap();
        let again = round_trip(&doc);
        assert_eq!(in_bank, EntryRef::new(field::BANK, banks_before), "{label}: it goes to the end of the bank");
        let banked = entry(&again, in_bank);
        assert_eq!(banked.location, ItemLocation::Bank, "{label}");
        assert_eq!(raw_blobs(&again, field::BANK)[in_bank.index], raw_before, "{label}: the blob is untouched");
        assert!(!raw_blobs(&again, field::WEAPONS).contains(&raw_before), "{label}: it left the backpack");

        let back = doc.move_item(in_bank, false).unwrap();
        let again = round_trip(&doc);
        let out = entry(&again, back);
        assert_eq!((out.location, out.quick_slot, out.mark), (ItemLocation::Backpack, Some(0), Some(1)), "{label}");
        assert_eq!(raw_blobs(&again, field::WEAPONS)[back.index], raw_before, "{label}");
        assert_eq!(raw_blobs(&again, field::BANK).len(), banks_before, "{label}");
    }
}

#[test]
fn moving_an_item_moves_up_the_ones_after_it() {
    let mut doc = synthetic_doc();
    // 53:2 is a backpack item with a stack of 3 and a favourite star; 53:3 to 53:5 are virtual.
    let before = by_ref(&doc);
    let in_bank = doc.move_item("53:2".parse().unwrap(), true).unwrap();
    assert_eq!(in_bank, EntryRef::new(field::BANK, 1));
    let after = by_ref(&doc);
    for (from, to) in [("53:3", "53:2"), ("53:4", "53:3"), ("53:5", "53:4")] {
        let (from, to): (EntryRef, EntryRef) = (from.parse().unwrap(), to.parse().unwrap());
        assert_eq!(after[&to].blob, before[&from].blob, "{from} is now {to}");
    }
    assert!(!after.contains_key(&"53:5".parse().unwrap()));
    // In the bank it has no star or stack; out again it gets the defaults.
    assert_eq!((after[&in_bank].mark, after[&in_bank].quantity), (None, None));
    let out = doc.move_item(in_bank, false).unwrap();
    let out = entry(&doc, out);
    assert_eq!((out.location, out.is_equipped, out.mark, out.quantity), (ItemLocation::Backpack, false, Some(1), Some(1)));
}

#[test]
fn moves_that_cannot_be_made_change_nothing() {
    let doc = synthetic_doc();
    let refused = |at: &str, to_bank: bool| {
        let mut copy = doc.clone();
        let error = copy.move_item(at.parse().unwrap(), to_bank).unwrap_err();
        assert_eq!(copy, doc, "{at}");
        match error {
            SaveKitError::InvalidValue { reason, .. } => reason,
            other => panic!("{other:?}"),
        }
    };
    assert!(refused("54:0", true).contains("equipped"));
    assert!(refused("53:1", true).contains("equipped"));
    assert!(refused("53:3", true).contains("placeholder"));
    assert!(refused("41:0", true).contains("bank already"));
    assert!(refused("54:1", false).contains("backpack already"));
    assert!(refused("54:7", true).contains("no item"));
    let mut corrupt = doc.clone();
    corrupt_entry_blob(&mut corrupt, "54:1".parse().unwrap());
    let before = corrupt.clone();
    assert!(corrupt.move_item("54:1".parse().unwrap(), true).is_err());
    assert_eq!(corrupt, before);
}

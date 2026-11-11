//! Tests on real saves from `fixtures/local`. They run over every `.sav` there and
//! pass without doing anything when there is none.

use savekit::proto::decode_packed_varints;
use savekit::{codec, field, verify, Expectation, FallbackGearClassifier, ItemLocation, ProtoMessage, SaveDocument};

use crate::support::{fixtures, packed, submessage, CLASS_PATH};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Says which fixtures the other tests ran on (shown with `--nocapture`).
#[test]
fn report_fixtures() {
    let found = fixtures();
    eprintln!("savekit fixture tests ran on {} save(s) from fixtures/local", found.len());
    for fixture in &found {
        let known = fixture.bytes.len() > 20 && hex(&fixture.bytes[..20]) == WINDOWS_LEVEL_1_GAIGE;
        let note = if known { " (known answers checked)" } else { "" };
        eprintln!("  {}: {} bytes{note}", fixture.name, fixture.bytes.len());
    }
}

#[test]
fn fixtures_decode_and_round_trip() {
    for fixture in fixtures() {
        let name = &fixture.name;
        let payload = codec::decode(&fixture.bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let doc = SaveDocument::from_file_bytes(&fixture.bytes).unwrap_or_else(|e| panic!("{name}: {e}"));

        // Unedited decode, encode, decode: the payload is identical, byte for byte.
        assert_eq!(doc.message.encode(), payload, "{name}: message re-encoding");
        let written = doc.to_file_bytes().unwrap();
        assert_eq!(codec::decode(&written).unwrap(), payload, "{name}: file round trip");
        assert_eq!(SaveDocument::from_file_bytes(&written).unwrap(), doc, "{name}");
        assert_eq!(verify(&doc, &written, &[Expectation::Unchanged]), Ok(()), "{name}");
    }
}

/// Every item the game wrote passes our checksum, and its code round-trips.
#[test]
fn fixture_items_all_decode() {
    for fixture in fixtures() {
        let name = &fixture.name;
        let doc = SaveDocument::from_file_bytes(&fixture.bytes).unwrap();
        let inventory = doc.inventory();
        let total = [field::BANK, field::ITEMS, field::WEAPONS].map(|n| doc.message.count(n)).iter().sum::<usize>();
        assert_eq!(inventory.len(), total, "{name}: every entry is listed");
        for entry in &inventory {
            let blob = entry.blob.as_ref().unwrap_or_else(|| panic!("{name}: {} does not decode", entry.entry_ref));
            assert_eq!(blob.version, 7, "{name}: {}", entry.entry_ref);
            assert_eq!(blob.is_weapon, entry.entry_ref.field_number == field::WEAPONS || (entry.location == ItemLocation::Bank && blob.is_weapon));
            let again = savekit::ItemBlob::from_code(&blob.code()).unwrap();
            assert_eq!(again.item_bytes, blob.item_bytes, "{name}: {}", entry.entry_ref);
        }
        let exported = doc.export_codes(true).len();
        assert_eq!(exported, inventory.iter().filter(|e| !e.is_virtual).count(), "{name}");
    }
}

/// SHA-1 header of `win-throwaway-Save0001.sav`: a level-1 Gaige written by the Windows
/// game on 2026-11-08. The facts below were established with an independent decoder.
const WINDOWS_LEVEL_1_GAIGE: &str = "5d30e618e5bd457ac712e6fdc3a3f07b0b9e6d13";

#[test]
fn windows_level_1_save_known_answers() {
    for fixture in fixtures().into_iter().filter(|f| f.bytes.len() > 20 && hex(&f.bytes[..20]) == WINDOWS_LEVEL_1_GAIGE) {
        let file = &fixture.bytes;
        assert_eq!(file.len(), 7_679);

        // Container.
        let declared = u32::from_be_bytes(file[20..24].try_into().unwrap()) as usize;
        assert_eq!(declared, 10_621);
        let wsg = lzo1x::decompress(&file[24..], declared).unwrap();
        assert_eq!(u32::from_be_bytes(wsg[..4].try_into().unwrap()) as usize, wsg.len() - 4, "size field");
        assert_eq!(wsg[7..11], [2, 0, 0, 0], "version");
        // Unlike Mac saves and the format doc, the 4 bytes after the Huffman stream are not zero.
        assert_eq!(wsg[wsg.len() - 8..], [0xA1, 0x78, 0x9B, 0x9E, 0xD4, 0x93, 0x9F, 0x1A]);
        let payload = codec::decode(file).unwrap();
        assert_eq!(payload.len(), 15_803);
        assert_eq!(u32::from_le_bytes(wsg[15..19].try_into().unwrap()), 15_803);

        // Character.
        let doc = SaveDocument::from_file_bytes(file).unwrap();
        assert_eq!(doc.class_path(), CLASS_PATH);
        assert_eq!(doc.name(), "Gaige");
        assert_eq!((doc.level(), doc.experience(), doc.save_game_id()), (1, 0, 1));
        assert_eq!((doc.money(), doc.eridium(), doc.seraph(), doc.torgue()), (80, 0, 0, 0));
        assert_eq!(packed(&doc, field::CURRENCY), [80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], "13 entries, not 5");
        assert_eq!((doc.backpack_slots(), doc.gun_slots(), doc.bank_slots()), (12, 2, 6));
        assert_eq!(packed(&doc, field::BLACK_MARKET), [0; 7], "7 entries, not 9");
        assert_eq!(doc.op_level(), 0);

        // Inventory.
        assert_eq!(doc.message.count(field::BANK), 0);
        assert_eq!(doc.message.count(field::ITEMS), 17);
        assert_eq!(doc.message.count(field::WEAPONS), 3);
        let inventory = doc.inventory();
        assert_eq!(inventory.len(), 20);
        let virtual_payloads: Vec<i64> = inventory
            .iter()
            .filter(|e| e.is_virtual)
            .map(|e| {
                let entry = doc.message.values(field::ITEMS).nth(e.entry_ref.index).unwrap();
                ProtoMessage::decode(entry.as_bytes().unwrap()).unwrap().varint(2).unwrap() as i64
            })
            .collect();
        assert_eq!(virtual_payloads, [-3, -4, -5]);
        // Twelve entries are not gear: 9-byte items of set 9 with no manufacturer. With the one
        // weapon that is not equipped they make 13 entries "in the backpack" of a 12-slot
        // character. Whether the game counts them against the 12 slots is not known.
        let no_manufacturer =
            inventory.iter().filter(|e| e.fields.as_ref().is_some_and(|f| f.manufacturer == Some(2047))).count();
        assert_eq!(no_manufacturer, 12);
        assert_eq!(doc.backpack_used(), 13);
        let duplicable: Vec<String> = inventory
            .iter()
            .filter(|e| e.is_duplicable(&FallbackGearClassifier))
            .map(|e| e.entry_ref.to_string())
            .collect();
        assert_eq!(duplicable, ["53:12", "53:13", "54:0", "54:1", "54:2"]);
        // The game sets the unused high bits of an item's last byte to 1 (the format doc's
        // samples, which did not come from the game, have 0).
        for entry in inventory.iter().filter(|e| !e.is_virtual) {
            let blob = entry.blob.as_ref().unwrap();
            let spare_bits = if blob.is_weapon { 3 } else { 2 };
            let last = *blob.item_bytes.last().unwrap();
            assert_eq!(last >> (8 - spare_bits), (1 << spare_bits) - 1, "{}: pad bits", entry.entry_ref);
        }
        let quick_slots: Vec<Option<i64>> =
            inventory.iter().filter(|e| e.entry_ref.field_number == field::WEAPONS).map(|e| e.quick_slot).collect();
        assert_eq!(quick_slots, [Some(1), Some(2), Some(0)]);

        // Top-level fields: in ascending order, and with fields the format doc does not list.
        let numbers: Vec<u32> = doc.message.fields.iter().map(|f| f.number).collect();
        assert!(numbers.windows(2).all(|w| w[0] <= w[1]), "ascending field order");
        for unknown in [5, 31, 42, 44, 45, 48, 50, 51, 58] {
            assert!(numbers.contains(&unknown), "field {unknown}");
        }
        assert!(decode_packed_varints(doc.message.bytes(field::CURRENCY).unwrap()).is_ok());
        assert_eq!(submessage(&doc, field::SIZES).varint(1), Some(12));
    }
}

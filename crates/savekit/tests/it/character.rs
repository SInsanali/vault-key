use savekit::proto::encode_packed_varints;
use savekit::tables::{AMMO_TYPES, REQUIRED_XP};
use savekit::{field, Currency, ProtoField, ProtoMessage, ProtoValue, SaveDocument, SaveKitError};

use crate::support::{packed, round_trip, sample_docs, sub, submessage, synthetic_doc, CLASS_PATH};

type Edit = fn(&mut SaveDocument) -> savekit::Result<()>;

fn doc_of(fields: Vec<ProtoField>) -> SaveDocument {
    SaveDocument::new(ProtoMessage::new(fields))
}

#[test]
fn xp_table_is_sane() {
    assert_eq!(REQUIRED_XP.len(), 80);
    assert_eq!(REQUIRED_XP[0], 0);
    assert!(REQUIRED_XP.windows(2).all(|w| w[1] > w[0]));
    for (i, &xp) in REQUIRED_XP.iter().enumerate() {
        let formula = (60.0 * ((i + 1) as f64).powf(2.8) - 60.0).ceil() as i64;
        assert!((xp - formula).abs() <= 3, "level {}: {xp} vs formula {formula}", i + 1);
    }
    // Spot values: the wiki's level 2, 50, 72 and 80.
    assert_eq!((REQUIRED_XP[1], REQUIRED_XP[49], REQUIRED_XP[71], REQUIRED_XP[79]), (358, 3_429_728, 9_520_932, 12_787_958));
}

#[test]
fn reads_the_synthetic_save() {
    let doc = round_trip(&synthetic_doc());
    assert_eq!(doc.class_path(), CLASS_PATH);
    assert_eq!(doc.name(), "Gaige");
    assert_eq!((doc.level(), doc.experience(), doc.save_game_id()), (5, 6_000, 3));
    assert_eq!((doc.money(), doc.eridium(), doc.seraph(), doc.torgue()), (80, 0, 0, 0));
    assert_eq!((doc.backpack_slots(), doc.gun_slots(), doc.bank_slots()), (12, 2, 6));
    assert_eq!(doc.playthroughs_unlocked(), 0);
    assert!(!doc.slaughterdome_unlocked());
}

#[test]
fn an_empty_save_reads_as_zeros() {
    let doc = SaveDocument::default();
    assert_eq!(doc.name(), "");
    assert_eq!(doc.class_path(), "");
    assert_eq!((doc.level(), doc.experience(), doc.save_game_id(), doc.money()), (0, 0, 0, 0));
    assert_eq!((doc.backpack_slots(), doc.gun_slots(), doc.bank_slots(), doc.op_level()), (0, 0, 0, 0));
    assert!(doc.inventory().is_empty());
}

#[test]
fn rejects_out_of_range_values() {
    let cases: [(&str, Edit); 15] = [
        ("level 0", |d| d.set_level(0)),
        ("level 81", |d| d.set_level(81)),
        ("level -1", |d| d.set_level(-1)),
        ("negative money", |d| d.set_currency(Currency::Money, -1)),
        ("money over i32", |d| d.set_currency(Currency::Money, 2_147_483_648)),
        ("empty name", |d| d.set_name("")),
        ("emoji name", |d| d.set_name("Zer0 \u{1F52B}")),
        ("backpack 40", |d| d.set_backpack_slots(40)),
        ("backpack 11", |d| d.set_backpack_slots(11)),
        ("bank 25", |d| d.set_bank_slots(25)),
        ("bank 5", |d| d.set_bank_slots(5)),
        ("gun slots 5", |d| d.set_gun_slots(5)),
        ("gun slots 1", |d| d.set_gun_slots(1)),
        ("save id 0", |d| d.set_save_game_id(0)),
        ("save id -4", |d| d.set_save_game_id(-4)),
    ];
    for (name, original) in sample_docs() {
        for (label, edit) in cases {
            let mut doc = original.clone();
            let result = edit(&mut doc);
            assert!(matches!(result, Err(SaveKitError::InvalidValue { .. })), "{name}, {label}: {result:?}");
            assert_eq!(doc, original, "{name}, {label}: the save changed");
        }
    }
}

#[test]
fn name_round_trip_preserves_other_subfields() {
    for (label, mut doc) in sample_docs() {
        let before = submessage(&doc, field::APPEARANCE);
        doc.set_name("Z\u{EB}r0").unwrap();
        let again = round_trip(&doc);
        assert_eq!(again.name(), "Z\u{EB}r0", "{label}");
        let after = submessage(&again, field::APPEARANCE);
        // Latin-1: one byte per character.
        assert_eq!(after.bytes(1), Some(&[b'Z', 0xEB, b'r', b'0'][..]), "{label}");
        let others = |m: &ProtoMessage| m.fields.iter().filter(|f| f.number != 1).cloned().collect::<Vec<_>>();
        assert_eq!(others(&after), others(&before), "{label}");
    }
}

#[test]
fn id_and_currency_round_trip() {
    for (label, mut doc) in sample_docs() {
        let before = packed(&doc, field::CURRENCY);
        doc.set_save_game_id(42).unwrap();
        doc.set_currency(Currency::Money, 123_456).unwrap();
        doc.set_currency(Currency::Eridium, 500).unwrap();
        doc.set_currency(Currency::Seraph, 99).unwrap();
        doc.set_currency(Currency::Torgue, 77).unwrap();
        doc.set_currency(Currency::Torgue, 2_147_483_647).unwrap();
        let again = round_trip(&doc);
        assert_eq!(again.save_game_id(), 42, "{label}");
        assert_eq!((again.money(), again.eridium(), again.seraph()), (123_456, 500, 99), "{label}");
        assert_eq!(again.torgue(), 2_147_483_647, "{label}");
        assert_eq!(again.currency(Currency::Torgue), again.torgue());

        // The game's array is longer than the reference editor knows (13 entries in a
        // Windows save). Entries the setters do not own are kept, and none are added.
        let after = packed(&again, field::CURRENCY);
        assert_eq!(after.len(), before.len().max(5), "{label}");
        for (i, value) in before.iter().enumerate().filter(|(i, _)| ![0, 1, 2, 4].contains(i)) {
            assert_eq!(after[i], *value, "{label}: currency entry {i}");
        }
        assert_eq!(again.message.count(field::CURRENCY), 1, "{label}");
    }
}

#[test]
fn currency_array_is_created_with_five_entries() {
    let mut doc = SaveDocument::default();
    doc.set_currency(Currency::Eridium, 3).unwrap();
    assert_eq!(doc.eridium(), 3);
    assert_eq!(packed(&doc, field::CURRENCY), [0, 3, 0, 0, 0]);
}

#[test]
fn level_round_trip_adjusts_xp() {
    for (label, original) in sample_docs() {
        let mut doc = original.clone();
        let skill_points: Vec<_> = doc.message.values(4).cloned().collect();
        for level in [1, 2, 30, 79, 80] {
            doc.set_level(level).unwrap();
            let again = round_trip(&doc);
            assert_eq!(again.level(), level, "{label}");
            let floor = REQUIRED_XP[(level - 1) as usize];
            if level == 80 {
                assert_eq!(again.experience(), floor, "{label}");
            } else {
                let next = REQUIRED_XP[level as usize];
                assert!((floor..next).contains(&again.experience()), "{label}: level {level}");
            }
            assert_eq!(again.message.count(field::LEVEL), 1);
            assert_eq!(again.message.count(field::EXPERIENCE), 1);
        }
        assert_eq!(doc.message.values(4).cloned().collect::<Vec<_>>(), skill_points, "{label}: skill points");

        // Setting the level a save already has changes nothing.
        let mut same = original.clone();
        let level = same.level();
        if (1..80).contains(&level) && (REQUIRED_XP[(level - 1) as usize]..REQUIRED_XP[level as usize]).contains(&same.experience()) {
            same.set_level(level).unwrap();
            assert_eq!(same, original, "{label}");
        }
    }
}

#[test]
fn xp_within_the_level_is_kept() {
    let mut doc = synthetic_doc();
    // Level 5 is 5,376 to 8,996 XP.
    for (xp, expected) in [(5_376, 5_376), (6_000, 6_000), (8_996, 8_996), (8_997, 5_376), (5_375, 5_376), (0, 5_376)] {
        doc.message.set_single(field::EXPERIENCE, ProtoValue::Varint(xp));
        doc.set_level(5).unwrap();
        assert_eq!(doc.experience(), expected, "starting from {xp}");
    }
    // At level 80 the XP is exactly the minimum.
    doc.message.set_single(field::EXPERIENCE, ProtoValue::Varint(99_999_999));
    doc.set_level(80).unwrap();
    assert_eq!(doc.experience(), 12_787_958);
    // A save with no XP field gets one.
    let mut bare = SaveDocument::default();
    bare.set_level(1).unwrap();
    assert_eq!(bare.message.varint(field::EXPERIENCE), Some(0));
    // A nonsense XP varint does not trap.
    doc.message.set_single(field::EXPERIENCE, ProtoValue::Varint(u64::MAX));
    doc.set_level(7).unwrap();
    assert_eq!(doc.experience(), REQUIRED_XP[6]);
}

#[test]
fn slots_round_trip() {
    for (label, mut doc) in sample_docs() {
        let sizes_before = submessage(&doc, field::SIZES);
        let market_before = packed(&doc, field::BLACK_MARKET);
        doc.set_backpack_slots(39).unwrap();
        doc.set_bank_slots(20).unwrap();
        doc.set_gun_slots(4).unwrap();
        let mut again = round_trip(&doc);
        assert_eq!((again.backpack_slots(), again.bank_slots(), again.gun_slots()), (39, 20, 4), "{label}");
        let market = packed(&again, field::BLACK_MARKET);
        assert_eq!(market.len(), 9, "{label}");
        assert_eq!((market[7], market[8]), (9, 7), "{label}: SDU counts");
        for (i, value) in market_before.iter().enumerate().take(7) {
            assert_eq!(market[i], *value, "{label}: ammo SDU {i}");
        }

        // Sizes are rounded up to whole SDUs: 13 slots is 1 SDU (15), a bank of 7 is 8.
        again.set_backpack_slots(13).unwrap();
        again.set_bank_slots(7).unwrap();
        again.set_gun_slots(2).unwrap();
        let third = round_trip(&again);
        assert_eq!((third.backpack_slots(), third.bank_slots(), third.gun_slots()), (15, 8, 2), "{label}");
        let market = packed(&third, field::BLACK_MARKET);
        assert_eq!((market[7], market[8]), (1, 1), "{label}");

        // Unknown subfields of field 13 are kept, and "slots shown" is not above the count.
        let sizes = submessage(&third, field::SIZES);
        for f in sizes_before.fields.iter().filter(|f| f.number > 3) {
            assert!(sizes.fields.contains(f), "{label}: lost subfield {}", f.number);
        }
        assert!(sizes.varint(3).unwrap_or(0) == 0, "{label}");
    }
}

#[test]
fn gun_slots_lower_the_slots_shown() {
    let sizes = |shown: u64| sub(vec![ProtoField::varint(1, 12), ProtoField::varint(2, 4), ProtoField::varint(3, shown)]);
    let mut doc = doc_of(vec![ProtoField::new(field::SIZES, sizes(2))]);
    doc.set_gun_slots(3).unwrap();
    assert_eq!(submessage(&doc, field::SIZES).varint(3), Some(1));
    doc.set_gun_slots(4).unwrap();
    assert_eq!(submessage(&doc, field::SIZES).varint(3), Some(1), "never raised");
    doc.set_gun_slots(2).unwrap();
    assert_eq!(submessage(&doc, field::SIZES).varint(3), Some(0));
    assert_eq!(doc.backpack_slots(), 12);
}

/// The game writes only the black-market entries it needs (7 in a new character). Edits
/// pad no further than they must: 8 for the backpack, 9 for the bank, none for ammo.
#[test]
fn black_market_is_padded_minimally() {
    let seven = || doc_of(vec![ProtoField::bytes(field::BLACK_MARKET, encode_packed_varints(&[1, 2, 3, 4, 5, 6, 7]))]);

    let mut backpack = seven();
    backpack.set_backpack_slots(18).unwrap();
    assert_eq!(packed(&backpack, field::BLACK_MARKET), [1, 2, 3, 4, 5, 6, 7, 2]);

    let mut bank = seven();
    bank.set_bank_slots(10).unwrap();
    assert_eq!(packed(&bank, field::BLACK_MARKET), [1, 2, 3, 4, 5, 6, 7, 0, 2]);
    assert_eq!(bank.bank_slots(), 10);

    let mut ammo = seven();
    ammo.maximize_ammo().unwrap();
    assert_eq!(packed(&ammo, field::BLACK_MARKET), [1, 2, 3, 4, 5, 6, 7]);
    ammo.unlock_ammo().unwrap();
    assert_eq!(packed(&ammo, field::BLACK_MARKET), [7; 7]);

    // A full table keeps its length and its other entries.
    let mut full = doc_of(vec![ProtoField::bytes(field::BLACK_MARKET, encode_packed_varints(&[1, 2, 3, 4, 5, 6, 7, 8, 9]))]);
    full.set_backpack_slots(12).unwrap();
    assert_eq!(packed(&full, field::BLACK_MARKET), [1, 2, 3, 4, 5, 6, 7, 0, 9]);

    // No table at all.
    let mut none = SaveDocument::default();
    none.set_backpack_slots(39).unwrap();
    assert_eq!(packed(&none, field::BLACK_MARKET), [0, 0, 0, 0, 0, 0, 0, 9]);
    assert_eq!(none.backpack_slots(), 39);
}

#[test]
fn unlocks_round_trip() {
    for (label, mut doc) in sample_docs() {
        doc.unlock_tvhm().unwrap();
        assert!(round_trip(&doc).playthroughs_unlocked() >= 1, "{label}");
        doc.unlock_uvhm().unwrap();
        doc.unlock_tvhm().unwrap();
        assert_eq!(round_trip(&doc).playthroughs_unlocked(), 2, "{label}: TVHM never lowers UVHM");

        doc.unlock_slaughterdome().unwrap();
        doc.unlock_slaughterdome().unwrap();
        let again = round_trip(&doc);
        assert!(again.slaughterdome_unlocked(), "{label}");
        for number in [field::UNLOCKS, field::UNLOCK_NOTIFICATIONS] {
            assert_eq!(again.message.count(number), 1, "{label}");
            let ids = again.message.bytes(number).unwrap();
            assert_eq!(ids.iter().filter(|&&id| id == 1).count(), 1, "{label}: field {number}");
        }
    }
}

#[test]
fn slaughterdome_keeps_other_unlocks() {
    let mut doc = doc_of(vec![ProtoField::bytes(field::UNLOCKS, vec![5, 9])]);
    doc.unlock_slaughterdome().unwrap();
    assert_eq!(doc.message.bytes(field::UNLOCKS), Some(&[5, 9, 1][..]));
    assert_eq!(doc.message.bytes(field::UNLOCK_NOTIFICATIONS), Some(&[1][..]));
}

#[test]
fn ammo_round_trip() {
    for (label, mut doc) in sample_docs() {
        let market_len = packed(&doc, field::BLACK_MARKET).len();
        doc.unlock_ammo().unwrap();
        let again = round_trip(&doc);
        let market = packed(&again, field::BLACK_MARKET);
        assert_eq!(market[..7], [7; 7], "{label}");
        assert_eq!(market.len(), market_len.max(7), "{label}: no padding beyond the ammo entries");

        let mut seen = Vec::new();
        for value in again.message.values(field::RESOURCES) {
            let resource = ProtoMessage::decode(value.as_bytes().unwrap()).unwrap();
            let Some(ammo) = AMMO_TYPES.iter().find(|a| resource.bytes(1) == Some(a.resource.as_bytes())) else {
                continue;
            };
            seen.push(ammo.name);
            assert_eq!(resource.varint(4), Some(7), "{label}: {}", ammo.name);
            let amount = (ammo.base + ammo.step * 7) as f32;
            assert_eq!(resource.first(3), Some(&ProtoValue::Fixed32(amount.to_bits())), "{label}: {}", ammo.name);
            assert_eq!(resource.bytes(2), Some(ammo.pool.as_bytes()), "{label}: {}", ammo.name);
        }
        seen.sort_unstable();
        assert_eq!(seen, ["grenade", "launcher", "pistol", "rifle", "shotgun", "smg", "sniper"], "{label}");

        // Doing it again changes nothing.
        let mut twice = again.clone();
        twice.maximize_ammo().unwrap();
        assert_eq!(twice, again, "{label}");
    }
}

/// Format doc 5.12: pool size is base + step x SDU level, with the level capped at 7.
#[test]
fn maximize_ammo_creates_missing_entries_and_keeps_unknown_subfields() {
    let rifle = &AMMO_TYPES[0];
    let existing = sub(vec![
        ProtoField::bytes(1, rifle.resource),
        ProtoField::bytes(2, rifle.pool),
        ProtoField::new(3, ProtoValue::Fixed32(1.0f32.to_bits())),
        ProtoField::varint(4, 0),
        ProtoField::varint(9, 5),
    ]);
    let other = sub(vec![ProtoField::bytes(1, "D_Resources.Something.Else"), ProtoField::varint(4, 3)]);
    let mut doc = doc_of(vec![
        ProtoField::new(field::RESOURCES, existing),
        ProtoField::new(field::RESOURCES, other.clone()),
        ProtoField::bytes(field::BLACK_MARKET, encode_packed_varints(&[0, 1, 2, 3, 4, 5, 60, 7, 8])),
        ProtoField::varint(field::BANK_SIZE, 6),
    ]);
    doc.maximize_ammo().unwrap();

    let entries: Vec<ProtoMessage> =
        doc.message.values(field::RESOURCES).map(|v| ProtoMessage::decode(v.as_bytes().unwrap()).unwrap()).collect();
    assert_eq!(entries.len(), 8, "the two existing entries and six new ones");
    assert_eq!(entries[0].varint(9), Some(5), "unknown subfield kept");
    assert_eq!(entries[0].varint(4), Some(0));
    assert_eq!(entries[0].first(3), Some(&ProtoValue::Fixed32(280.0f32.to_bits())));
    assert_eq!(doc.message.values(field::RESOURCES).nth(1), Some(&other), "unknown resource untouched");

    let amount_of = |name: &str| {
        let ammo = AMMO_TYPES.iter().find(|a| a.name == name).unwrap();
        let entry = entries.iter().find(|e| e.bytes(1) == Some(ammo.resource.as_bytes())).unwrap();
        assert_eq!(entry.bytes(2), Some(ammo.pool.as_bytes()));
        let Some(ProtoValue::Fixed32(bits)) = entry.first(3) else { panic!("{name}: amount is not a float") };
        (entry.varint(4).unwrap(), f32::from_bits(*bits))
    };
    assert_eq!(amount_of("pistol"), (1, 300.0));
    assert_eq!(amount_of("launcher"), (2, 18.0));
    assert_eq!(amount_of("shotgun"), (3, 140.0));
    assert_eq!(amount_of("smg"), (4, 1080.0));
    assert_eq!(amount_of("sniper"), (5, 108.0));
    assert_eq!(amount_of("grenade"), (7, 10.0), "SDU level capped at 7");

    // The fields around the new entries stay in order, and field 36 is not rewritten.
    let numbers: Vec<u32> = doc.message.fields.iter().map(|f| f.number).collect();
    assert_eq!(numbers, [11, 11, 11, 11, 11, 11, 11, 11, 36, 56]);
    assert_eq!(packed(&doc, field::BLACK_MARKET), [0, 1, 2, 3, 4, 5, 60, 7, 8]);

    // A black-market table with more than 9 entries is refused.
    let mut bad = doc_of(vec![ProtoField::bytes(field::BLACK_MARKET, encode_packed_varints(&[1; 10]))]);
    let before = bad.clone();
    let edits: [Edit; 2] = [SaveDocument::maximize_ammo, SaveDocument::unlock_ammo];
    for edit in edits {
        assert!(matches!(edit(&mut bad), Err(SaveKitError::InvalidValue { .. })));
    }
    assert!(bad.set_backpack_slots(20).is_err());
    assert!(bad.set_bank_slots(10).is_err());
    assert_eq!(bad, before);
}

/// An edit refuses a field whose encoding it does not expect, instead of dropping data.
#[test]
fn edits_refuse_unexpected_encodings() {
    let five = encode_packed_varints(&[5, 6, 7, 8, 9]);
    let two_entries = doc_of(vec![ProtoField::bytes(6, five.clone()), ProtoField::bytes(6, five.clone())]);
    let unpacked = doc_of(vec![ProtoField::varint(6, 5), ProtoField::varint(6, 6)]);
    let single = doc_of(vec![ProtoField::varint(6, 5)]);
    let truncated = doc_of(vec![ProtoField::bytes(6, vec![0x80])]);
    for original in [two_entries, unpacked, single, truncated] {
        let mut doc = original.clone();
        assert!(doc.set_currency(Currency::Money, 1).is_err());
        assert_eq!(doc, original);
    }

    let market = doc_of(vec![ProtoField::bytes(6, five), ProtoField::varint(36, 1), ProtoField::varint(36, 2)]);
    let edits: [Edit; 4] = [
        |d| d.set_bank_slots(10),
        |d| d.set_backpack_slots(20),
        |d| d.maximize_ammo(),
        |d| d.unlock_ammo(),
    ];
    for edit in edits {
        let mut doc = market.clone();
        assert!(matches!(edit(&mut doc), Err(SaveKitError::InvalidValue { .. })));
        assert_eq!(doc, market);
    }

    let sizes = doc_of(vec![ProtoField::varint(field::SIZES, 1)]);
    let appearance = doc_of(vec![ProtoField::bytes(field::APPEARANCE, vec![0x0A, 50, 1])]);
    let edits: [(&SaveDocument, Edit); 3] =
        [(&sizes, |d| d.set_gun_slots(3)), (&sizes, |d| d.set_backpack_slots(20)), (&appearance, |d| d.set_name("Sam"))];
    for (original, edit) in edits {
        let mut doc = original.clone();
        assert!(edit(&mut doc).is_err());
        assert_eq!(&doc, original);
    }
}

#[test]
fn nonsense_varints_do_not_trap() {
    let max = encode_packed_varints(&[u64::MAX; 5]);
    let doc = doc_of(vec![ProtoField::bytes(field::CURRENCY, max)]);
    assert_eq!((doc.money(), doc.eridium(), doc.seraph(), doc.torgue()), (-1, -1, -1, -1));
    let big = doc_of(vec![ProtoField::bytes(field::CURRENCY, encode_packed_varints(&[1 << 63]))]);
    assert_eq!(big.money(), 0, "read as the low 32 bits");
    assert_eq!(big.torgue(), 0, "past the end of the array");

    let mut doc = doc_of(vec![
        ProtoField::varint(field::LEVEL, u64::MAX),
        ProtoField::varint(field::EXPERIENCE, u64::MAX),
        ProtoField::varint(field::PLAYTHROUGHS_COMPLETED, u64::MAX),
        ProtoField::varint(field::SAVE_GAME_ID, 1 << 63),
        ProtoField::varint(field::BANK_SIZE, u64::MAX),
        ProtoField::new(field::SIZES, sub(vec![ProtoField::varint(1, u64::MAX), ProtoField::varint(2, u64::MAX), ProtoField::varint(3, u64::MAX)])),
        ProtoField::bytes(field::BLACK_MARKET, encode_packed_varints(&[u64::MAX; 9])),
    ]);
    assert_eq!((doc.level(), doc.experience(), doc.bank_slots()), (-1, -1, -1));
    assert_eq!(doc.save_game_id(), i64::MIN);
    assert_eq!((doc.backpack_slots(), doc.gun_slots()), (-1, -1));
    doc.unlock_tvhm().unwrap();
    doc.set_gun_slots(3).unwrap();
    doc.set_level(80).unwrap();
    doc.maximize_ammo().unwrap();
    doc.set_backpack_slots(39).unwrap();
    doc.set_bank_slots(24).unwrap();
    doc.set_item_levels(0, true).unwrap();
    assert_eq!((doc.level(), doc.backpack_slots(), doc.bank_slots(), doc.gun_slots()), (80, 39, 24, 3));
}

#[test]
fn one_ammo_level_changes_that_type_only() {
    for (label, mut doc) in sample_docs() {
        let before = doc.clone();
        let levels = doc.ammo_levels();
        // Sniper is index 5.
        doc.set_ammo_level(5, 3).unwrap();
        let again = round_trip(&doc);
        let mut expected = levels;
        expected[5] = 3;
        assert_eq!(again.ammo_levels(), expected, "{label}");
        let pool = |doc: &SaveDocument, index: usize| {
            doc.message
                .values(field::RESOURCES)
                .filter_map(|v| ProtoMessage::decode(v.as_bytes()?).ok())
                .find(|r| r.bytes(1) == Some(AMMO_TYPES[index].resource.as_bytes()))
        };
        for other in (0..7).filter(|&i| i != 5) {
            assert_eq!(pool(&again, other), pool(&before, other), "{label}: {} untouched", AMMO_TYPES[other].name);
        }
        let Some(sniper) = pool(&again, 5) else { continue };
        assert_eq!(sniper.varint(4), Some(3), "{label}");
        assert_eq!(sniper.first(3), pool(&before, 5).unwrap().first(3), "{label}: raising keeps the ammo");
        // Full at the top level, then lowered: trimmed to the new size.
        doc.set_ammo_level(5, 7).unwrap();
        doc.maximize_ammo().unwrap();
        doc.set_ammo_level(5, 0).unwrap();
        assert_eq!(pool(&doc, 5).unwrap().first(3), Some(&ProtoValue::Fixed32(48f32.to_bits())), "{label}");
    }
    let mut doc = synthetic_doc();
    let before = doc.clone();
    assert!(matches!(doc.set_ammo_level(7, 1), Err(SaveKitError::InvalidValue { .. })));
    assert!(matches!(doc.set_ammo_level(0, 8), Err(SaveKitError::InvalidValue { .. })));
    assert_eq!(doc, before);
}

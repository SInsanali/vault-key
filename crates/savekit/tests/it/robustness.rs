//! Nothing panics on file data: truncated and corrupted saves go through the whole decode
//! path, and whatever still decodes goes through every read and every edit.

use savekit::checksums::sha1;
use savekit::{
    codec, field, verify, Currency, Expectation, FallbackGearClassifier, ProtoMessage, ProtoValue, SaveDocument,
};

use crate::support::{sample_files, Rng, ITEM_CODE, WEAPON_CODE};

type Edit = fn(&mut SaveDocument) -> savekit::Result<()>;

const EDITS: [Edit; 17] = [
    |d| d.set_name("Fuzz"),
    |d| d.set_save_game_id(7),
    |d| d.set_level(42),
    |d| d.set_currency(Currency::Money, 99),
    |d| d.set_currency(Currency::Torgue, 99),
    |d| d.set_backpack_slots(27),
    |d| d.set_bank_slots(16),
    |d| d.set_gun_slots(3),
    |d| d.unlock_tvhm(),
    |d| d.unlock_uvhm(),
    |d| d.unlock_slaughterdome(),
    |d| d.unlock_ammo(),
    |d| d.maximize_ammo(),
    |d| d.set_item_levels(0, true),
    |d| d.set_item_levels(30, false),
    |d| d.set_op_level(4),
    |d| d.import_codes_with(&[WEAPON_CODE, ITEM_CODE], false, &mut || 5).map(|_| ()),
];

/// Every read and every edit. Results do not matter; not panicking does.
fn exercise(doc: &SaveDocument) {
    let _ = (doc.name(), doc.class_path(), doc.level(), doc.experience(), doc.save_game_id());
    let _ = (doc.money(), doc.eridium(), doc.seraph(), doc.torgue());
    let _ = (doc.backpack_slots(), doc.gun_slots(), doc.bank_slots(), doc.playthroughs_unlocked());
    let _ = (doc.slaughterdome_unlocked(), doc.op_level(), doc.backpack_used(), doc.export_codes(true));
    let inventory = doc.inventory();
    for entry in &inventory {
        let _ = entry.is_duplicable(&FallbackGearClassifier);
    }

    let mut all = doc.clone();
    for edit in EDITS {
        let mut one = doc.clone();
        let _ = edit(&mut one);
        let _ = edit(&mut all);
    }
    for entry in &inventory {
        let _ = all.duplicate_with(entry.entry_ref, &FallbackGearClassifier, &mut || 9);
    }
    if let Ok(written) = all.to_file_bytes() {
        let source = inventory.first().map(|e| e.entry_ref);
        let mut expectations = vec![Expectation::Unchanged, Expectation::Name("Fuzz".into())];
        expectations.extend(source.map(|source| Expectation::Duplicated { source, count: 1 }));
        let _ = verify(doc, &written, &expectations);
        let _ = SaveDocument::from_file_bytes(&written).map(|again| again.inventory());
    }
}

/// Decodes `file`; if it decodes, exercises it. Returns whether it decoded.
fn feed(file: &[u8]) -> bool {
    match SaveDocument::from_file_bytes(file) {
        Ok(doc) => {
            exercise(&doc);
            true
        }
        Err(_) => false,
    }
}

fn rehash(body: &[u8]) -> Vec<u8> {
    let mut file = sha1(body).to_vec();
    file.extend_from_slice(body);
    file
}

fn wrap(wsg: &[u8]) -> Vec<u8> {
    let mut body = (wsg.len() as u32).to_be_bytes().to_vec();
    body.extend_from_slice(&lzo1x::compress_literals(wsg));
    rehash(&body)
}

/// One to three small changes: a flipped bit, a new byte, a cut, an insertion, a run of
/// `00` or `FF`.
fn mutate(rng: &mut Rng, bytes: &mut Vec<u8>) {
    for _ in 0..=rng.below(3) {
        if bytes.is_empty() {
            *bytes = rng.bytes(4);
            continue;
        }
        let at = rng.below(bytes.len());
        match rng.below(6) {
            0 => bytes[at] ^= 1 << rng.below(8),
            1 => bytes[at] = rng.next() as u8,
            2 => bytes.truncate(at),
            3 => {
                let extra_len = 1 + rng.below(8);
                let extra = rng.bytes(extra_len);
                bytes.splice(at..at, extra);
            }
            4 => {
                let end = (at + 1 + rng.below(8)).min(bytes.len());
                bytes[at..end].fill(0xFF);
            }
            _ => {
                let end = (at + 1 + rng.below(8)).min(bytes.len());
                bytes[at..end].fill(0);
            }
        }
    }
}

#[test]
fn the_samples_themselves_survive_every_edit() {
    for (name, file) in sample_files() {
        assert!(feed(&file), "{name}");
    }
}

#[test]
fn every_truncation_is_an_error() {
    for (name, file) in sample_files() {
        for len in 0..file.len() {
            assert!(SaveDocument::from_file_bytes(&file[..len]).is_err(), "{name}: first {len} bytes decoded");
        }
        // The same with a correct hash, so the cut reaches the decompressor.
        for len in 0..file.len() - 20 {
            let cut = rehash(&file[20..20 + len]);
            assert!(SaveDocument::from_file_bytes(&cut).is_err(), "{name}: {len} hashed bytes decoded");
        }
    }
}

#[test]
fn corrupted_files_never_panic() {
    for (name, file) in sample_files() {
        let mut rng = Rng(0xF11E);
        let mut decoded = 0;
        // Raw damage: the hash catches it.
        for _ in 0..500 {
            let mut bad = file.clone();
            mutate(&mut rng, &mut bad);
            decoded += usize::from(bad != file && feed(&bad));
        }
        assert_eq!(decoded, 0, "{name}: a damaged file passed its SHA-1");

        // Damage under a correct hash: the length field and the compressed stream.
        for _ in 0..3_000 {
            let mut body = file[20..].to_vec();
            mutate(&mut rng, &mut body);
            feed(&rehash(&body));
        }
    }
}

#[test]
fn corrupted_wsg_blobs_never_panic() {
    for (name, file) in sample_files() {
        let declared = u32::from_be_bytes(file[20..24].try_into().unwrap()) as usize;
        let wsg = lzo1x::decompress(&file[24..], declared).unwrap();
        assert!(feed(&wrap(&wsg)), "{name}: re-wrapped unchanged");

        let mut rng = Rng(0xB10B);
        let mut failed = 0;
        for round in 0..3_000 {
            let mut bad = wsg.clone();
            if round % 2 == 0 {
                // Aim at the header and the serialised Huffman tree.
                let at = rng.below(bad.len().min(400));
                bad[at] = rng.next() as u8;
            } else {
                mutate(&mut rng, &mut bad);
            }
            failed += usize::from(!feed(&wrap(&bad)));
        }
        assert!(failed > 1_000, "{name}: only {failed} of 3000 damaged blobs were rejected");
    }
}

/// Damage inside a valid container (good hash, good CRC): this is what reaches the
/// protobuf reader, the character edits and the item code.
#[test]
fn corrupted_payloads_never_panic() {
    for (name, file) in sample_files() {
        let payload = codec::decode(&file).unwrap();
        let mut rng = Rng(0xDA7A);
        let (mut decoded, mut failed) = (0, 0);
        for _ in 0..1_500 {
            let mut bad = payload.clone();
            mutate(&mut rng, &mut bad);
            if feed(&codec::encode(&bad).unwrap()) {
                decoded += 1;
            } else {
                failed += 1;
            }
        }
        assert!(decoded > 100 && failed > 100, "{name}: {decoded} decoded, {failed} failed");
    }
}

/// Damage aimed at one top-level field at a time, so that nested messages, packed arrays
/// and item blobs are hit often. The rest of the save stays valid.
#[test]
fn corrupted_fields_never_panic() {
    let item_fields = [field::BANK, field::ITEMS, field::WEAPONS];
    for (name, file) in sample_files() {
        let original = SaveDocument::from_file_bytes(&file).unwrap();
        let targets: Vec<usize> = (0..original.message.fields.len())
            .filter(|&i| matches!(original.message.fields[i].value, ProtoValue::Bytes(_)))
            .collect();
        let items: Vec<usize> =
            targets.iter().copied().filter(|&i| item_fields.contains(&original.message.fields[i].number)).collect();
        assert!(!items.is_empty(), "{name}");

        let mut rng = Rng(0x17E5);
        for round in 0..4_000 {
            let mut doc = original.clone();
            // Half the rounds go to the inventory.
            let pool = if round % 2 == 0 { &items } else { &targets };
            let slot = &mut doc.message.fields[pool[rng.below(pool.len())]];
            let ProtoValue::Bytes(bytes) = &mut slot.value else { unreachable!() };
            if round % 4 == 0 {
                // Go one level down: damage the item blob but keep the entry well-formed.
                if let Ok(mut entry) = ProtoMessage::decode(bytes) {
                    if let Some(ProtoValue::Bytes(blob)) = entry.fields.iter_mut().find(|f| f.number == 1).map(|f| &mut f.value) {
                        mutate(&mut rng, blob);
                    }
                    *bytes = entry.encode();
                }
            } else {
                mutate(&mut rng, bytes);
            }
            // Mostly straight to the document; every eighth round through the file as well.
            exercise(&doc);
            if round % 8 == 0 {
                feed(&doc.to_file_bytes().unwrap());
            }
        }
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x5EED);
    for _ in 0..5_000 {
        let len = rng.below(300);
        let junk = rng.bytes(len);
        feed(&junk);
        feed(&rehash(&junk));
        feed(&wrap(&junk));
        if let Ok(message) = ProtoMessage::decode(&junk) {
            exercise(&SaveDocument::new(message));
        }
        let _ = codec::encode(&junk).map(|file| assert!(feed(&file) || ProtoMessage::decode(&junk).is_err()));
    }
}

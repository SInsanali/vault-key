//! Shared helpers: fixture discovery, a synthetic save that needs no fixture, and a
//! repeatable random number generator.

use std::path::{Path, PathBuf};

use savekit::proto::encode_packed_varints;
use savekit::{field, EntryRef, ItemBlob, ItemFields, ProtoField, ProtoMessage, ProtoValue, SaveDocument};

/// Format doc 6.5: weapon, version 7, key 1105057183, 32 item bytes, level 50.
pub const WEAPON_CODE: &str = "BL2(h0Hd1Z+jY/s2Qy++Zu8Ba9qXoOmjwJ6NhrlsOmhNMX+oJo5CfQns)";
pub const WEAPON_CODE_KEY_0: &str = "BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)";
/// Format doc 6.5: item, version 7, key 1810808715, 25 item bytes, level 1.
pub const ITEM_CODE: &str = "BL2(B2vuv4tz1zSQCf2pqLJCS5XD/tKN4FXpjRJLnn1v85U=)";
pub const ITEM_CODE_KEY_0: &str = "BL2(BwAAAABrxwAYC17YIBGBAABAAMD/////////Pw2AAwA=)";
/// A second weapon with a real manufacturer.
pub const GEAR_WEAPON_CODE: &str = "BL2(hwAAAAC8CQCBxwJABBEI0MJghdENIwHG//////////97w/iGMQzj)";
/// An intro ECHO: a story item with no manufacturer (all ones).
pub const ECHO_CODE: &str = "BL2(BwAAAABsWgADYgaQ4/8AwA==)";

pub const CLASS_PATH: &str = "GD_Tulip_Mechromancer.Character.CharClass_Mechromancer";

/// SplitMix64, so tests are repeatable without a dependency.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// A real save from `fixtures/local` at the repository root.
pub struct Fixture {
    pub name: String,
    pub bytes: Vec<u8>,
}

fn fixture_dir() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    Some(repo.join("Tests").join("Fixtures").join("local"))
}

/// Every `.sav` in the fixture folder, by name. Empty when the folder is missing or empty,
/// so fixture tests then pass without doing anything.
pub fn fixtures() -> Vec<Fixture> {
    let Some(entries) = fixture_dir().and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "sav"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| Fixture {
            name: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            bytes: std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display())),
        })
        .collect()
}

pub fn sub(fields: Vec<ProtoField>) -> ProtoValue {
    ProtoValue::Bytes(ProtoMessage::new(fields).encode())
}

/// The item bytes of a virtual item: `set` = 255, then 32 zero bytes.
pub fn virtual_item_bytes() -> Vec<u8> {
    let mut bytes = vec![0u8; 33];
    bytes[0] = 0xFF;
    bytes
}

/// A field-53 entry holding a virtual item with payload `-(id | value << 8)`.
pub fn virtual_entry(id: i64, value: i64) -> ProtoValue {
    sub(vec![
        ProtoField::bytes(1, ItemBlob::new(false, 7, 0, virtual_item_bytes()).to_raw()),
        ProtoField::varint(2, (-(id | (value << 8))) as u64),
        ProtoField::varint(3, 0),
        ProtoField::varint(4, 0),
    ])
}

fn raw(code: &str, key: i32) -> Vec<u8> {
    ItemBlob::from_code(code).expect("test item code").rekeyed(key).to_raw()
}

/// The 25-byte spec item, raised to level 20 so that item-level edits apply to it.
pub fn level_20_item() -> ItemBlob {
    let blob = ItemBlob::from_code(ITEM_CODE).expect("test item code");
    let mut fields = ItemFields::from_blob(&blob);
    fields.grade_index = Some(20);
    fields.game_stage = Some(20);
    let item_bytes = fields.encode(false, Some(&blob.item_bytes));
    ItemBlob::new(false, 7, 77_777, item_bytes)
}

/// A small save shaped like the Windows game's level-1 saves: 13 currency entries, 7
/// black-market entries, three virtual items, and unknown fields of every wire type. It
/// also has items the level-1 save lacks: an equipped shield, a level-20 item, a level-50
/// equipped weapon and a weapon in the bank.
pub fn synthetic_doc() -> SaveDocument {
    let colour = |r: u64| sub(vec![ProtoField::varint(1, 255), ProtoField::varint(2, r), ProtoField::varint(3, 9)]);
    let fields = vec![
        ProtoField::bytes(field::CLASS, CLASS_PATH),
        ProtoField::varint(field::LEVEL, 5),
        ProtoField::varint(field::EXPERIENCE, 6_000),
        ProtoField::varint(4, 4),
        ProtoField::bytes(5, vec![1, 2, 3]),
        ProtoField::bytes(field::CURRENCY, encode_packed_varints(&[80, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 3])),
        ProtoField::varint(field::PLAYTHROUGHS_COMPLETED, 0),
        ProtoField::new(
            field::SIZES,
            sub(vec![ProtoField::varint(1, 12), ProtoField::varint(2, 2), ProtoField::varint(3, 0), ProtoField::varint(9, 7)]),
        ),
        ProtoField::new(
            field::APPEARANCE,
            sub(vec![
                ProtoField::bytes(1, "Gaige"),
                ProtoField::new(2, colour(1)),
                ProtoField::new(3, colour(2)),
                ProtoField::new(4, colour(3)),
            ]),
        ),
        ProtoField::varint(field::SAVE_GAME_ID, 3),
        ProtoField::new(31, ProtoValue::Fixed32(0xDEAD_BEEF)),
        ProtoField::bytes(field::BLACK_MARKET, encode_packed_varints(&[0; 7])),
        ProtoField::new(field::BANK, sub(vec![ProtoField::bytes(1, raw(WEAPON_CODE, -5))])),
        ProtoField::new(42, ProtoValue::Fixed64(0x0102_0304_0506_0708)),
        ProtoField::new(
            field::ITEMS,
            sub(vec![
                ProtoField::bytes(1, raw(ECHO_CODE, 12_345)),
                ProtoField::varint(2, 1),
                ProtoField::varint(3, 0),
                ProtoField::varint(4, 1),
            ]),
        ),
        ProtoField::new(
            field::ITEMS,
            sub(vec![
                ProtoField::bytes(1, raw(ITEM_CODE, 1_810_808_715)),
                ProtoField::varint(2, 1),
                ProtoField::varint(3, 1),
                ProtoField::varint(4, 1),
            ]),
        ),
        ProtoField::new(
            field::ITEMS,
            sub(vec![
                ProtoField::bytes(1, level_20_item().to_raw()),
                ProtoField::varint(2, 3),
                ProtoField::varint(3, 0),
                ProtoField::varint(4, 2),
            ]),
        ),
        ProtoField::new(field::ITEMS, virtual_entry(3, 0)),
        ProtoField::new(field::ITEMS, virtual_entry(4, 0)),
        ProtoField::new(field::ITEMS, virtual_entry(5, 0)),
        ProtoField::new(
            field::WEAPONS,
            sub(vec![ProtoField::bytes(1, raw(WEAPON_CODE, 1_105_057_183)), ProtoField::varint(2, 1), ProtoField::varint(3, 1)]),
        ),
        ProtoField::new(
            field::WEAPONS,
            sub(vec![ProtoField::bytes(1, raw(GEAR_WEAPON_CODE, -2_000_000_000)), ProtoField::varint(2, 0), ProtoField::varint(3, 2)]),
        ),
        ProtoField::varint(field::BANK_SIZE, 6),
        ProtoField::varint(58, 1),
    ];
    SaveDocument::new(ProtoMessage::new(fields))
}

/// The synthetic save and every fixture, as file bytes.
pub fn sample_files() -> Vec<(String, Vec<u8>)> {
    let synthetic = synthetic_doc().to_file_bytes().expect("synthetic save encodes");
    let mut files = vec![("synthetic".to_string(), synthetic)];
    files.extend(fixtures().into_iter().map(|f| (f.name, f.bytes)));
    files
}

/// The synthetic save and every fixture, decoded.
pub fn sample_docs() -> Vec<(String, SaveDocument)> {
    let mut docs = vec![("synthetic".to_string(), synthetic_doc())];
    for fixture in fixtures() {
        let doc = SaveDocument::from_file_bytes(&fixture.bytes)
            .unwrap_or_else(|e| panic!("fixture {} does not decode: {e}", fixture.name));
        docs.push((fixture.name, doc));
    }
    docs
}

/// Encodes to file bytes and decodes again.
pub fn round_trip(doc: &SaveDocument) -> SaveDocument {
    SaveDocument::from_file_bytes(&doc.to_file_bytes().expect("encodes")).expect("decodes")
}

/// The nested message in the first entry of top-level field `number` (empty if absent).
pub fn submessage(doc: &SaveDocument, number: u32) -> ProtoMessage {
    doc.message.bytes(number).map_or_else(ProtoMessage::default, |b| ProtoMessage::decode(b).expect("nested message"))
}

/// The packed varints in top-level field `number` (empty if absent).
pub fn packed(doc: &SaveDocument, number: u32) -> Vec<u64> {
    doc.message.bytes(number).map_or_else(Vec::new, |b| savekit::proto::decode_packed_varints(b).expect("packed"))
}

/// Rewrites the nested message of one inventory entry in place.
pub fn edit_entry(doc: &mut SaveDocument, entry: EntryRef, body: impl FnOnce(&mut ProtoMessage)) {
    let slot = doc
        .message
        .fields
        .iter_mut()
        .filter(|f| f.number == entry.field_number)
        .nth(entry.index)
        .expect("entry exists");
    let mut message = ProtoMessage::decode(slot.value.as_bytes().expect("entry is bytes")).expect("entry decodes");
    body(&mut message);
    slot.value = ProtoValue::Bytes(message.encode());
}

/// Flips one bit inside the obfuscated body of an entry's item blob, so its checksum fails.
pub fn corrupt_entry_blob(doc: &mut SaveDocument, entry: EntryRef) {
    edit_entry(doc, entry, |message| {
        let slot = message.fields.iter_mut().find(|f| f.number == 1).expect("entry has a blob");
        let ProtoValue::Bytes(blob) = &mut slot.value else { panic!("blob is not bytes") };
        blob[8] ^= 0x01;
    });
}

/// The raw item blob of every entry of a field, in message order.
pub fn raw_blobs(doc: &SaveDocument, number: u32) -> Vec<Vec<u8>> {
    doc.message
        .values(number)
        .filter_map(|v| ProtoMessage::decode(v.as_bytes()?).ok()?.bytes(1).map(<[u8]>::to_vec))
        .collect()
}

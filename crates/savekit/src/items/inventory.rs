//! Inventory listing, item codes, Duplicate Item, item levels and the OP level
//! (format doc sections 5.5, 5.13, 5.20, 5.21 and 6.1).

use std::fmt;
use std::str::FromStr;

use crate::document::SaveDocument;
use crate::error::{Result, SaveKitError};
use crate::field;
use crate::items::blob::ItemBlob;
use crate::items::fields::ItemFields;
use crate::items::keys::{EntropyKeys, KeySource};
use crate::proto::{ProtoField, ProtoMessage, ProtoValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemLocation {
    Backpack,
    Equipped,
    Bank,
}

/// Identifies an inventory entry: the top-level field it is in ([`field::BANK`],
/// [`field::ITEMS`] or [`field::WEAPONS`]) and its position among that field's entries.
/// Written as `field:index`, for example `54:0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryRef {
    pub field_number: u32,
    /// 0-based, in message order.
    pub index: usize,
}

impl EntryRef {
    pub fn new(field_number: u32, index: usize) -> Self {
        EntryRef { field_number, index }
    }
}

impl fmt::Display for EntryRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.field_number, self.index)
    }
}

impl FromStr for EntryRef {
    type Err = SaveKitError;

    fn from_str(s: &str) -> Result<Self> {
        let parsed = s.split_once(':').and_then(|(field, index)| Some((field.parse().ok()?, index.parse().ok()?)));
        let (field_number, index) =
            parsed.ok_or_else(|| SaveKitError::invalid("item", "item reference must look like 54:0"))?;
        Ok(EntryRef { field_number, index })
    }
}

/// One entry of the bank, the items or the weapons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    pub entry_ref: EntryRef,
    pub location: ItemLocation,
    pub is_weapon: bool,
    /// `None` when the blob fails to decode (bad checksum or too short).
    pub blob: Option<ItemBlob>,
    pub fields: Option<ItemFields>,
    /// Weapons outside the bank: 0 = in the backpack, 1 to 4 = equipped in that slot.
    pub quick_slot: Option<i64>,
    pub is_equipped: bool,
    /// The "star": by community knowledge 0 = trash, 1 = standard, 2 = favourite.
    pub mark: Option<i64>,
    /// Stack size, for real items outside the bank.
    pub quantity: Option<i64>,
    /// A placeholder record (such as the OP level), not something the player holds.
    pub is_virtual: bool,
}

impl InventoryEntry {
    /// Decodable and not virtual: what export and import work on.
    pub fn is_exportable(&self) -> bool {
        self.blob.is_some() && !self.is_virtual
    }

    /// Exportable, and gear according to `classifier`.
    pub fn is_duplicable(&self, classifier: &dyn GearClassifier) -> bool {
        match &self.fields {
            Some(fields) if self.is_exportable() => classifier.gear_class(fields, self.is_weapon) == GearClass::Gear,
            _ => false,
        }
    }
}

/// Whether an item counts as gear (weapon, shield, grenade mod, class mod, relic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GearClass {
    Gear,
    /// A story, mission or usable item. The game may remove a copy of one.
    NotGear,
    /// Not in the classifier's data.
    Unknown,
}

/// Decides whether a saved item is gear. Implement this from the game's data; without game
/// data, use [`FallbackGearClassifier`].
pub trait GearClassifier {
    fn gear_class(&self, fields: &ItemFields, is_weapon: bool) -> GearClass;
}

/// The rule for when no game data is available: gear always has a real manufacturer, so an
/// absent or all-ones manufacturer marks a story or mission item (such as the intro ECHO).
#[derive(Debug, Clone, Copy, Default)]
pub struct FallbackGearClassifier;

/// All 11 bits of the manufacturer field set: no manufacturer.
const NO_MANUFACTURER: u32 = 2047;

impl GearClassifier for FallbackGearClassifier {
    fn gear_class(&self, fields: &ItemFields, _is_weapon: bool) -> GearClass {
        match fields.manufacturer {
            Some(manufacturer) if manufacturer != NO_MANUFACTURER => GearClass::Gear,
            _ => GearClass::NotGear,
        }
    }
}

/// The virtual item blob (format doc 5.13): item, version 7, key 0, checksum `39 2A`, then
/// 33 item bytes with `set` = 255 and everything else zero.
const VIRTUAL_ITEM_BLOB: [u8; 40] = {
    let mut blob = [0u8; 40];
    blob[0] = 0x07;
    blob[5] = 0x39;
    blob[6] = 0x2A;
    blob[7] = 0xFF;
    blob
};

/// Virtual item id of the "OP levels unlocked" record.
const OP_LEVEL_ID: i64 = 4;
pub const MAX_OP_LEVEL: i64 = 10;
/// Grade and stage are 7 bits wide.
pub const MAX_ITEM_LEVEL: i64 = 127;

/// How many keys to draw before giving up on the key source and deriving a key instead.
const KEY_ATTEMPTS: usize = 8;

/// An edit aimed at one item could not be made; `why` says what is wrong with the item.
fn item_refused(why: impl Into<String>) -> SaveKitError {
    SaveKitError::invalid("itemTarget", why)
}

/// Position in `message.fields` of the inventory entry `at`.
fn position_of(m: &ProtoMessage, at: EntryRef) -> Option<usize> {
    m.fields.iter().enumerate().filter(|(_, f)| f.number == at.field_number).nth(at.index).map(|(i, _)| i)
}

fn not_duplicable(why: &str) -> SaveKitError {
    SaveKitError::NotDuplicable(why.to_string())
}

fn make_entry(field_number: u32, index: usize, data: &[u8]) -> InventoryEntry {
    let sub = ProtoMessage::decode(data).ok();
    let blob = sub.as_ref().and_then(|s| s.bytes(1)).and_then(|raw| ItemBlob::from_raw(raw).ok());
    let fields = blob.as_ref().map(ItemFields::from_blob);
    let is_weapon = blob.as_ref().map_or(field_number == field::WEAPONS, |b| b.is_weapon);
    let is_virtual = fields.as_ref().is_some_and(ItemFields::is_virtual);
    let int = |number: u32| sub.as_ref().and_then(|s| s.varint(number)).map(|v| v as i64);

    let in_items = field_number == field::ITEMS;
    let in_weapons = field_number == field::WEAPONS;
    let quick_slot = if in_weapons { int(2) } else { None };
    let is_equipped = in_items && int(3).unwrap_or(0) != 0;
    let mark = if in_items {
        int(4)
    } else if in_weapons {
        int(3)
    } else {
        None
    };
    let quantity = if in_items && !is_virtual { int(2) } else { None };
    let location = if field_number == field::BANK {
        ItemLocation::Bank
    } else if (is_weapon && (1..=4).contains(&quick_slot.unwrap_or(0))) || (!is_weapon && is_equipped) {
        ItemLocation::Equipped
    } else {
        ItemLocation::Backpack
    };
    InventoryEntry {
        entry_ref: EntryRef { field_number, index },
        location,
        is_weapon,
        blob,
        fields,
        quick_slot,
        is_equipped,
        mark,
        quantity,
        is_virtual,
    }
}

/// Field number and message for a new inventory entry: unequipped, and with the reference
/// editor's import defaults (mark 1, quantity 1) unless given.
fn new_entry(
    blob: Vec<u8>,
    is_weapon: bool,
    to_bank: bool,
    mark: Option<i64>,
    quantity: Option<i64>,
) -> (u32, ProtoMessage) {
    // Bit-preserving: a negative number becomes a 10-byte two's-complement varint.
    let varint = |number: u32, value: i64| ProtoField::varint(number, value as u64);
    let mut sub = vec![ProtoField::bytes(1, blob)];
    let field_number = if to_bank {
        field::BANK
    } else if is_weapon {
        sub.extend([varint(2, 0), varint(3, mark.unwrap_or(1))]);
        field::WEAPONS
    } else {
        sub.extend([varint(2, quantity.unwrap_or(1)), varint(3, 0), varint(4, mark.unwrap_or(1))]);
        field::ITEMS
    };
    (field_number, ProtoMessage::new(sub))
}

/// A key from `keys` that differs from `taken`.
fn fresh_key(keys: &mut dyn KeySource, taken: i32) -> i32 {
    for _ in 0..KEY_ATTEMPTS {
        let key = keys.next_key();
        if key != taken {
            return key;
        }
    }
    taken.wrapping_add(1)
}

/// Id (low 8 bits) and value of a virtual item's payload in subfield 2, which holds
/// `-(id | value << 8)` as a 64-bit two's-complement varint.
fn virtual_payload(entry: &ProtoMessage) -> Option<(i64, i64)> {
    let n = (entry.varint(2)? as i64).wrapping_neg();
    Some((n & 0xFF, n >> 8))
}

fn is_virtual_entry(entry: &ProtoMessage) -> bool {
    let blob = entry.bytes(1).and_then(|raw| ItemBlob::from_raw(raw).ok());
    blob.is_some_and(|b| ItemFields::from_blob(&b).is_virtual())
}

impl SaveDocument {
    // Listing

    /// Every entry of the bank, the items and the weapons, in message order.
    pub fn inventory(&self) -> Vec<InventoryEntry> {
        let (mut bank, mut items, mut weapons) = (0usize, 0usize, 0usize);
        let mut out = Vec::new();
        for f in &self.message.fields {
            let counter = match f.number {
                field::BANK => &mut bank,
                field::ITEMS => &mut items,
                field::WEAPONS => &mut weapons,
                _ => continue,
            };
            let index = *counter;
            *counter += 1;
            // An entry with the wrong wire type still takes up an index, but is not listed.
            if let ProtoValue::Bytes(data) = &f.value {
                out.push(make_entry(f.number, index, data));
            }
        }
        out
    }

    /// Entries counted against the backpack: everything in the items and weapons fields that
    /// is neither equipped nor virtual. This includes story and usable items and entries that
    /// do not decode, so it can be higher than the count the game shows.
    pub fn backpack_used(&self) -> usize {
        self.inventory().iter().filter(|e| e.location == ItemLocation::Backpack && !e.is_virtual).count()
    }

    // Export and import

    /// Item codes (key 0) for the bank (if asked), then items, then weapons. Virtual and
    /// undecodable entries are skipped.
    pub fn export_codes(&self, include_bank: bool) -> Vec<String> {
        let inventory = self.inventory();
        let order = [(field::BANK, include_bank), (field::ITEMS, true), (field::WEAPONS, true)];
        let mut codes = Vec::new();
        for (number, wanted) in order {
            if !wanted {
                continue;
            }
            let entries = inventory.iter().filter(|e| e.entry_ref.field_number == number && e.is_exportable());
            codes.extend(entries.filter_map(|e| e.blob.as_ref()).map(ItemBlob::code));
        }
        codes
    }

    /// [`SaveDocument::import_codes_with`], with keys from [`EntropyKeys`].
    pub fn import_codes<S: AsRef<str>>(&mut self, codes: &[S], to_bank: bool) -> Result<Vec<EntryRef>> {
        self.import_codes_with(codes, to_bank, &mut EntropyKeys::new())
    }

    /// Adds the items in `codes` to the bank, or to the backpack (unequipped). Each gets a new
    /// key from `keys`. If any code is bad, nothing is added. Capacity is not checked.
    pub fn import_codes_with<S: AsRef<str>>(
        &mut self,
        codes: &[S],
        to_bank: bool,
        keys: &mut dyn KeySource,
    ) -> Result<Vec<EntryRef>> {
        let blobs = codes.iter().map(|code| ItemBlob::from_code(code.as_ref())).collect::<Result<Vec<_>>>()?;
        let mut refs = Vec::with_capacity(blobs.len());
        for blob in blobs {
            let raw = blob.rekeyed(keys.next_key()).to_raw();
            let (field_number, entry) = new_entry(raw, blob.is_weapon, to_bank, None, None);
            let index = self.message.push_repeated(field_number, ProtoValue::Bytes(entry.encode()));
            refs.push(EntryRef { field_number, index });
        }
        Ok(refs)
    }

    // Duplicate Item

    /// [`SaveDocument::duplicate_with`], with keys from [`EntropyKeys`].
    pub fn duplicate(&mut self, source: EntryRef, classifier: &dyn GearClassifier) -> Result<EntryRef> {
        self.duplicate_with(source, classifier, &mut EntropyKeys::new())
    }

    /// Copies a gear item into the backpack, unequipped, under a new key that differs from
    /// the source's. The copy keeps the source's mark and quantity.
    ///
    /// Fails with [`SaveKitError::NotDuplicable`] if the entry does not exist, cannot be
    /// decoded, is virtual, or is not gear according to `classifier`, and with
    /// [`SaveKitError::BackpackFull`] unless [`SaveDocument::backpack_used`] is below
    /// [`SaveDocument::backpack_slots`].
    pub fn duplicate_with(
        &mut self,
        source: EntryRef,
        classifier: &dyn GearClassifier,
        keys: &mut dyn KeySource,
    ) -> Result<EntryRef> {
        let inventory = self.inventory();
        let entry =
            inventory.iter().find(|e| e.entry_ref == source).ok_or_else(|| not_duplicable("no such inventory entry"))?;
        if entry.is_virtual {
            return Err(not_duplicable("virtual items cannot be duplicated"));
        }
        let (Some(blob), Some(fields)) = (&entry.blob, &entry.fields) else {
            return Err(not_duplicable("item data could not be decoded"));
        };
        match classifier.gear_class(fields, entry.is_weapon) {
            GearClass::Gear => {}
            GearClass::NotGear => return Err(not_duplicable("not a gear item (story/mission/usable item)")),
            GearClass::Unknown => {
                return Err(not_duplicable("unrecognized item; import game data or check the item"))
            }
        }
        let used = inventory.iter().filter(|e| e.location == ItemLocation::Backpack && !e.is_virtual).count();
        if i64::try_from(used).unwrap_or(i64::MAX) >= self.backpack_slots() {
            return Err(SaveKitError::BackpackFull);
        }

        let raw = blob.rekeyed(fresh_key(keys, blob.key)).to_raw();
        let (field_number, copy) = new_entry(raw, blob.is_weapon, false, entry.mark, entry.quantity);
        let index = self.message.push_repeated(field_number, ProtoValue::Bytes(copy.encode()));
        Ok(EntryRef { field_number, index })
    }

    // Item levels

    /// Sets the level (grade and stage) of items and weapons in the backpack or equipped; the
    /// bank is not touched. `level` 0 means the character's level. Without `force`, only
    /// entries whose level is above 1 change. Virtual and undecodable entries are skipped,
    /// and every item keeps its key.
    pub fn set_item_levels(&mut self, level: i64, force: bool) -> Result<()> {
        let range_error = || SaveKitError::invalid("itemLevel", format!("must be 0 to {MAX_ITEM_LEVEL}"));
        if !(0..=MAX_ITEM_LEVEL).contains(&level) {
            return Err(range_error());
        }
        let target = if level > 0 { level } else { self.level() };
        let target = u32::try_from(target).ok().filter(|&t| i64::from(t) <= MAX_ITEM_LEVEL).ok_or_else(range_error)?;

        self.edit(|m| {
            let levelled = m.fields.iter_mut().filter(|f| f.number == field::ITEMS || f.number == field::WEAPONS);
            for entry in levelled {
                let ProtoValue::Bytes(data) = &entry.value else { continue };
                let Ok(mut sub) = ProtoMessage::decode(data) else { continue };
                let Some(slot) = sub.fields.iter_mut().find(|f| f.number == 1) else { continue };
                let Some(mut blob) = slot.value.as_bytes().and_then(|raw| ItemBlob::from_raw(raw).ok()) else {
                    continue;
                };
                let mut fields = ItemFields::from_blob(&blob);
                if fields.is_virtual() || !fields.grade_index.is_some_and(|grade| force || grade > 1) {
                    continue;
                }
                fields.grade_index = Some(target);
                // A blob too short to hold a stage is left that way.
                if fields.game_stage.is_some() {
                    fields.game_stage = Some(target);
                }
                blob.item_bytes = fields.encode(blob.is_weapon, Some(&blob.item_bytes));
                slot.value = ProtoValue::Bytes(blob.to_raw());
                entry.value = ProtoValue::Bytes(sub.encode());
            }
            Ok(())
        })
    }

    /// Sets the level (grade and stage) of one item or weapon, wherever it is: backpack,
    /// equipped or bank. Unlike [`SaveDocument::set_item_levels`], a level-1 item is changed
    /// too: the user picked this item. It keeps its key and every other part.
    pub fn set_item_level(&mut self, at: EntryRef, level: i64) -> Result<()> {
        let target = u32::try_from(level)
            .ok()
            .filter(|&t| t >= 1 && i64::from(t) <= MAX_ITEM_LEVEL)
            .ok_or_else(|| SaveKitError::invalid("itemLevel", format!("must be 1 to {MAX_ITEM_LEVEL}")))?;
        if ![field::BANK, field::ITEMS, field::WEAPONS].contains(&at.field_number) {
            return Err(item_refused(format!("{at} is not an inventory entry")));
        }
        self.edit(|m| {
            let position = position_of(m, at).ok_or_else(|| item_refused(format!("there is no item {at}")))?;
            let entry = &mut m.fields[position];
            let data = entry.value.as_bytes().ok_or_else(|| item_refused("its data can't be read"))?;
            let mut sub = ProtoMessage::decode(data)?;
            let slot = sub.fields.iter_mut().find(|f| f.number == 1).ok_or_else(|| item_refused("its data can't be read"))?;
            let mut blob = slot
                .value
                .as_bytes()
                .and_then(|raw| ItemBlob::from_raw(raw).ok())
                .ok_or_else(|| item_refused("its data can't be read"))?;
            let mut fields = ItemFields::from_blob(&blob);
            if fields.is_virtual() {
                return Err(item_refused("it is a placeholder record, not an item"));
            }
            if fields.grade_index.is_none() {
                return Err(item_refused("it has no level"));
            }
            fields.grade_index = Some(target);
            // A blob too short to hold a stage is left that way.
            if fields.game_stage.is_some() {
                fields.game_stage = Some(target);
            }
            blob.item_bytes = fields.encode(blob.is_weapon, Some(&blob.item_bytes));
            slot.value = ProtoValue::Bytes(blob.to_raw());
            entry.value = ProtoValue::Bytes(sub.encode());
            Ok(())
        })
    }

    // Bank

    /// Moves an item or weapon from the backpack to the bank (`to_bank`), or back. The item
    /// goes to the end of its new list and keeps its blob byte for byte (key and level too);
    /// the entries after its old place move up one. Returns where it is now.
    ///
    /// A bank entry holds only the item (format doc 4.2, field 41), so its star and stack
    /// size are dropped on the way in, and an item coming out gets the import defaults
    /// (unequipped, standard star, quantity 1). Equipped, virtual and undecodable entries
    /// are refused. Capacity is not checked, and neither is whether the item is gear.
    pub fn move_item(&mut self, at: EntryRef, to_bank: bool) -> Result<EntryRef> {
        let entry = self
            .inventory()
            .into_iter()
            .find(|e| e.entry_ref == at)
            .ok_or_else(|| item_refused(format!("there is no item {at}")))?;
        if entry.is_virtual {
            return Err(item_refused("it is a placeholder record, not an item"));
        }
        if entry.blob.is_none() {
            return Err(item_refused("its data can't be read"));
        }
        match (entry.location, to_bank) {
            (ItemLocation::Equipped, _) => return Err(item_refused("it is equipped")),
            (ItemLocation::Bank, true) => return Err(item_refused("it is in the bank already")),
            (ItemLocation::Backpack, false) => return Err(item_refused("it is in the backpack already")),
            _ => {}
        }
        let mut moved_to = at;
        self.edit(|m| {
            let position = position_of(m, at).ok_or_else(|| item_refused(format!("there is no item {at}")))?;
            let removed = m.fields.remove(position);
            // The blob as it is stored, not re-encoded.
            let raw = removed
                .value
                .as_bytes()
                .and_then(|data| ProtoMessage::decode(data).ok())
                .and_then(|sub| sub.bytes(1).map(<[u8]>::to_vec))
                .ok_or_else(|| item_refused("its data can't be read"))?;
            let (field_number, moved) = new_entry(raw, entry.is_weapon, to_bank, None, None);
            let index = m.push_repeated(field_number, ProtoValue::Bytes(moved.encode()));
            moved_to = EntryRef { field_number, index };
            Ok(())
        })?;
        Ok(moved_to)
    }

    // OP level

    /// Position in `message.fields` and decoded form of the first items entry that is a
    /// virtual item with id 4.
    fn op_level_entry(&self) -> Option<(usize, ProtoMessage)> {
        self.message.fields.iter().enumerate().find_map(|(index, f)| {
            if f.number != field::ITEMS {
                return None;
            }
            let entry = ProtoMessage::decode(f.value.as_bytes()?).ok()?;
            let is_op_level = is_virtual_entry(&entry) && virtual_payload(&entry)?.0 == OP_LEVEL_ID;
            is_op_level.then_some((index, entry))
        })
    }

    /// Overpower levels unlocked (0 if the save has no OP level record).
    pub fn op_level(&self) -> i64 {
        self.op_level_entry().and_then(|(_, entry)| virtual_payload(&entry)).map_or(0, |(_, value)| value)
    }

    /// Sets the Overpower levels unlocked (0 to 10), creating the record if the save has
    /// none. Any level above 0 also unlocks Ultimate Vault Hunter Mode.
    pub fn set_op_level(&mut self, level: i64) -> Result<()> {
        if !(0..=MAX_OP_LEVEL).contains(&level) {
            return Err(SaveKitError::invalid("opLevel", format!("must be 0 to {MAX_OP_LEVEL}")));
        }
        let payload = ProtoValue::Varint((-(OP_LEVEL_ID | (level << 8))) as u64);
        let existing = self.op_level_entry();
        self.edit(|m| {
            match existing {
                Some((index, mut entry)) => {
                    entry.set_single(2, payload);
                    m.fields[index].value = ProtoValue::Bytes(entry.encode());
                }
                None => {
                    let entry = ProtoMessage::new(vec![
                        ProtoField::bytes(1, VIRTUAL_ITEM_BLOB),
                        ProtoField::new(2, payload),
                        ProtoField::varint(3, 0),
                        ProtoField::varint(4, 0),
                    ]);
                    m.push_repeated(field::ITEMS, ProtoValue::Bytes(entry.encode()));
                }
            }
            // An absent field counts as 0; a field that is not a varint is left alone.
            let playthroughs = m.first(field::PLAYTHROUGHS_COMPLETED).map_or(Some(0), ProtoValue::as_varint);
            if level > 0 && playthroughs.is_some_and(|p| p < 2) {
                m.set_single(field::PLAYTHROUGHS_COMPLETED, ProtoValue::Varint(2));
            }
            Ok(())
        })
    }
}

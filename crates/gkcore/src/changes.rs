//! Staged edits: what can be changed, how a list of pending changes is replayed onto the
//! original document, what the written file must then satisfy, and how each change reads
//! to the user.

use std::collections::HashSet;

use savekit::tables::{AMMO_TYPES, MAX_AMMO_SDU_LEVEL};
use savekit::{
    field, CharacterKey, Currency, EntryRef, Expectation, GearClass, GearClassifier, InventoryEntry, ItemBlob,
    ItemFields, ItemLocation, SaveDocument, SaveKitError, AMMO_SDU_COUNT, MAX_BACKPACK_SLOTS, MAX_BANK_SLOTS, MAX_CURRENCY, MAX_ITEM_LEVEL, MAX_LEVEL,
    MAX_OP_LEVEL, MIN_BACKPACK_SLOTS, MIN_BANK_SLOTS, MIN_LEVEL,
};

use crate::error::{fail, CoreError, ErrorKind, Result};
use crate::format;
use crate::writer::Check;

/// Design section 5a: the message shown when a copy has nowhere to go.
pub const BACKPACK_FULL_MESSAGE: &str = "Backpack full — free a slot or increase backpack size";
pub const BANK_FULL_MESSAGE: &str = "Bank full — free a slot or increase bank size";

/// Item struct version of Borderlands 2 items.
const BL2_ITEM_VERSION: u8 = 7;
/// The highest item level offered: level 80 plus ten Overpower levels.
pub const MAX_GEAR_LEVEL: i64 = MAX_LEVEL + MAX_OP_LEVEL;
/// Said when a change was made for an item that is no longer in the working character.
pub const ITEM_GONE_MESSAGE: &str =
    "The item this change was for is no longer there (another change it relied on was undone).";

/// The ammo types by the names the UI uses, in black-market order (see [`AMMO_TYPES`]).
pub const AMMO_NAMES: [&str; AMMO_SDU_COUNT] = ["rifle", "pistol", "launcher", "shotgun", "smg", "sniper", "grenade"];
/// What each ammo type is called on screen, in the same order.
pub const AMMO_LABELS: [&str; AMMO_SDU_COUNT] =
    ["Assault rifle ammo", "Pistol ammo", "Launcher ammo", "Shotgun ammo", "SMG ammo", "Sniper ammo", "Grenades"];
const AMMO_KEYS: [&str; AMMO_SDU_COUNT] =
    ["ammo-rifle", "ammo-pistol", "ammo-launcher", "ammo-shotgun", "ammo-smg", "ammo-sniper", "ammo-grenade"];

/// Rounds a pool of this ammo type holds at an SDU level.
pub fn ammo_capacity(index: usize, level: u64) -> u32 {
    let ammo = &AMMO_TYPES[index];
    ammo.base + ammo.step * u32::try_from(level.min(MAX_AMMO_SDU_LEVEL)).unwrap_or(0)
}

/// What identifies one item while changes are staged: its key and parts, not its level or
/// its place. A change aimed at one item finds it by this, so undoing an earlier change
/// that moved items around can never make it act on a different item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemIdentity {
    is_weapon: bool,
    key: i32,
    fields: ItemFields,
}

impl ItemIdentity {
    /// `None` for placeholder records and items whose data does not decode.
    pub fn of(entry: &InventoryEntry) -> Option<ItemIdentity> {
        let (blob, fields) = (entry.blob.as_ref()?, entry.fields.as_ref()?);
        if entry.is_virtual {
            return None;
        }
        let mut fields = fields.clone();
        fields.grade_index = None;
        fields.game_stage = None;
        Some(ItemIdentity { is_weapon: blob.is_weapon, key: blob.key, fields })
    }
}

/// An item with its level left out. Added items are counted this way, so that a level
/// change on an item that is also copied is not mistaken for one more copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LevelFree {
    is_weapon: bool,
    fields: ItemFields,
}

impl LevelFree {
    pub(crate) fn of_blob(blob: &ItemBlob) -> LevelFree {
        let mut fields = ItemFields::from_blob(blob);
        fields.grade_index = None;
        fields.game_stage = None;
        LevelFree { is_weapon: blob.is_weapon, fields }
    }

    pub(crate) fn of(entry: &InventoryEntry) -> Option<LevelFree> {
        entry.blob.as_ref().map(LevelFree::of_blob)
    }
}

/// Where the item a change was made for is now: at `at` if it is still there, else the one
/// place it moved to.
fn resolve(doc: &SaveDocument, at: EntryRef, identity: Option<&ItemIdentity>) -> Result<EntryRef> {
    let Some(identity) = identity else { return Ok(at) };
    let inventory = doc.inventory();
    let is_it = |e: &&InventoryEntry| ItemIdentity::of(e).as_ref() == Some(identity);
    if inventory.iter().filter(|e| e.entry_ref == at).any(|e| is_it(&e)) {
        return Ok(at);
    }
    let found: Vec<&InventoryEntry> = inventory.iter().filter(is_it).collect();
    match found.as_slice() {
        [one] => Ok(one.entry_ref),
        _ => fail(ErrorKind::Conflict, ITEM_GONE_MESSAGE),
    }
}

/// One edit to a character. Every edit the save engine offers is here, except changing the
/// save-game ID, which only "Save as a new character" does (it must equal the slot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Name(String),
    /// 1 to 80. Experience is raised or lowered to fit.
    Level(i64),
    Currency(Currency, i64),
    /// 12 to 39; rounded up to a whole upgrade (3 slots each).
    BackpackSlots(i64),
    /// 6 to 24; rounded up to a whole upgrade (2 slots each).
    BankSlots(i64),
    /// 2, 3 or 4.
    GunSlots(i64),
    /// Overpower levels unlocked, 0 to 10.
    OpLevel(i64),
    UnlockTvhm,
    UnlockUvhm,
    UnlockSlaughterdome,
    /// Buys every ammo upgrade and fills the ammo pools.
    UnlockAmmo,
    /// Fills the ammo pools to what the current upgrades allow.
    MaximizeAmmo,
    /// Sets the level of every item and weapon outside the bank. `level` 0 means the
    /// character's level. Without `force`, items of level 1 are left alone.
    ItemLevels { level: i64, force: bool },
    /// Adds the items in these item codes to the backpack or the bank.
    ImportCodes { codes: Vec<String>, to_bank: bool },
    /// Duplicate Item: one more copy of a gear item, placed in the backpack.
    Duplicate { source: EntryRef },
    /// The level of one gear item (1 to [`MAX_GEAR_LEVEL`]), wherever it is.
    ItemLevel { item: EntryRef, level: i64 },
    /// Moves a gear item from the backpack to the bank, or back.
    MoveItem { item: EntryRef, to_bank: bool },
    /// One ammo type's upgrade level (0 to 7); `ammo` indexes [`AMMO_NAMES`]. Its pool is
    /// filled to the new size.
    AmmoLevel { ammo: usize, level: i64 },
}

impl Edit {
    /// Edits with the same key replace each other; `None` for edits that always add a new
    /// pending change (each duplicate and each import is its own change).
    pub(crate) fn coalesce_key(&self) -> Option<&'static str> {
        Some(match self {
            Edit::Name(_) => "name",
            Edit::Level(_) => "level",
            Edit::Currency(Currency::Money, _) => "money",
            Edit::Currency(Currency::Eridium, _) => "eridium",
            Edit::Currency(Currency::Seraph, _) => "seraph",
            Edit::Currency(Currency::Torgue, _) => "torgue",
            Edit::BackpackSlots(_) => "backpackSlots",
            Edit::BankSlots(_) => "bankSlots",
            Edit::GunSlots(_) => "gunSlots",
            Edit::OpLevel(_) => "opLevel",
            Edit::UnlockTvhm => "unlockTvhm",
            Edit::UnlockUvhm => "unlockUvhm",
            Edit::UnlockSlaughterdome => "unlockSlaughterdome",
            Edit::UnlockAmmo => "unlockAmmo",
            Edit::MaximizeAmmo => "maximizeAmmo",
            Edit::ItemLevels { .. } => "itemLevels",
            // One per item: the session also compares the item.
            Edit::ItemLevel { .. } => "itemLevel",
            Edit::AmmoLevel { ammo, .. } => AMMO_KEYS.get(*ammo).copied().unwrap_or("ammo"),
            Edit::ImportCodes { .. } | Edit::Duplicate { .. } | Edit::MoveItem { .. } => return None,
        })
    }

    /// The item an edit is aimed at, as it was named when the edit was staged.
    pub(crate) fn item(&self) -> Option<EntryRef> {
        match self {
            Edit::Duplicate { source } => Some(*source),
            Edit::ItemLevel { item, .. } | Edit::MoveItem { item, .. } => Some(*item),
            _ => None,
        }
    }

    /// A few words naming the edit, for messages about it: "Level 30", "Duplicate 54:0".
    pub fn summary(&self) -> String {
        match self {
            Edit::Name(name) => format!("Name {name}"),
            Edit::Level(level) => format!("Level {level}"),
            Edit::Currency(Currency::Money, amount) => format!("Money {}", format::money(*amount)),
            Edit::Currency(Currency::Eridium, amount) => format!("Eridium {}", format::number(*amount)),
            Edit::Currency(Currency::Seraph, amount) => format!("Seraph crystals {}", format::number(*amount)),
            Edit::Currency(Currency::Torgue, amount) => format!("Torgue tokens {}", format::number(*amount)),
            Edit::BackpackSlots(slots) => format!("Backpack {slots} slots"),
            Edit::BankSlots(slots) => format!("Bank {slots} slots"),
            Edit::GunSlots(slots) => format!("Gun slots {slots}"),
            Edit::OpLevel(level) => format!("Overpower level OP {level}"),
            Edit::UnlockTvhm => "Unlock True Vault Hunter Mode".to_string(),
            Edit::UnlockUvhm => "Unlock Ultimate Vault Hunter Mode".to_string(),
            Edit::UnlockSlaughterdome => "Unlock Creature Slaughter Dome".to_string(),
            Edit::UnlockAmmo => "Unlock all ammo upgrades".to_string(),
            Edit::MaximizeAmmo => "Fill ammo".to_string(),
            Edit::ItemLevels { .. } => "Item levels".to_string(),
            Edit::ImportCodes { .. } => "Import codes".to_string(),
            Edit::Duplicate { source } => format!("Duplicate {source}"),
            Edit::ItemLevel { item, level } => format!("Item {item} level {level}"),
            Edit::MoveItem { item, to_bank: true } => format!("Move {item} to the bank"),
            Edit::MoveItem { item, to_bank: false } => format!("Move {item} to the backpack"),
            Edit::AmmoLevel { ammo, level } => {
                format!("{} upgrade {level}", AMMO_LABELS.get(*ammo).copied().unwrap_or("Ammo"))
            }
        }
    }

    pub(crate) fn adds_items(&self) -> bool {
        matches!(self, Edit::ImportCodes { .. } | Edit::Duplicate { .. })
    }
}

/// An edit waiting to be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    /// Identifies this change within its session; never reused.
    pub id: u64,
    pub edit: Edit,
    /// The obfuscation keys the new items get, drawn once when the change is staged so that
    /// every replay produces the same bytes.
    pub(crate) keys: Vec<i32>,
    /// For an edit aimed at one item: which item, found when the change was staged.
    pub(crate) target: Option<ItemIdentity>,
}

/// One pending change in words. Plain data, ready to send to a front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeLine {
    pub change_id: u64,
    /// What is changed: "Level", "Money", "Duplicate".
    pub label: String,
    /// The value before, where there is one: "1".
    pub from: Option<String>,
    /// The value after, or what happens: "30", "+1 copy in the backpack".
    pub to: String,
    /// The whole line: "Level 1 → 30".
    pub text: String,
    /// For a duplicate: the item being copied, as `field:index`, so the front end can show
    /// its name.
    pub item_ref: Option<String>,
}

/// The result of replaying pending changes onto the original document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Replay {
    /// The working document.
    pub doc: SaveDocument,
    /// Inventory entries each change created, by change id.
    pub created: Vec<(u64, Vec<EntryRef>)>,
    /// The source of each staged duplicate, by change id.
    pub duplicate_sources: Vec<(u64, EntryRef)>,
    /// Item bytes of every added item, with how many were added.
    pub added: Vec<(LevelFree, usize)>,
    /// Entries an item-level change touches, with the level they get.
    pub item_levels: Vec<(EntryRef, u32)>,
    /// For each change aimed at one item: where that item is now, and its level before.
    pub targets: Vec<(u64, EntryRef, Option<u32>)>,
    /// Once anything has been moved: where each entry that was there before the first move
    /// is now, by its place before the first move (which is its place in the original).
    pub places: Option<Vec<(EntryRef, EntryRef)>>,
}

impl Replay {
    pub(crate) fn unchanged(original: &SaveDocument) -> Replay {
        Replay {
            doc: original.clone(),
            created: Vec::new(),
            duplicate_sources: Vec::new(),
            added: Vec::new(),
            item_levels: Vec::new(),
            targets: Vec::new(),
            places: None,
        }
    }

    /// Updates every place recorded so far after the entry at `from` (in `before`) moved to
    /// `to`: the moved one is at `to`, and the entries after `from` in its field moved up one.
    fn relocate(&mut self, before: &SaveDocument, from: EntryRef, to: EntryRef) {
        let shift = |r: &mut EntryRef| {
            if *r == from {
                *r = to;
            } else if r.field_number == from.field_number && r.index > from.index {
                r.index -= 1;
            }
        };
        let places = self.places.get_or_insert_with(|| before.inventory().iter().map(|e| (e.entry_ref, e.entry_ref)).collect());
        places.iter_mut().for_each(|(_, now)| shift(now));
        self.created.iter_mut().flat_map(|(_, refs)| refs.iter_mut()).for_each(shift);
        self.duplicate_sources.iter_mut().for_each(|(_, r)| shift(r));
        self.item_levels.iter_mut().for_each(|(r, _)| shift(r));
        self.targets.iter_mut().for_each(|(_, r, _)| shift(r));
    }

    fn count_added(&mut self, blob: &ItemBlob) {
        let item = LevelFree::of_blob(blob);
        match self.added.iter_mut().find(|(known, _)| *known == item) {
            Some((_, count)) => *count += 1,
            None => self.added.push((item, 1)),
        }
    }
}

/// Replays `changes` in order onto a copy of `original`.
///
/// Stops at the first change that fails. The error says why and carries that change's id;
/// nothing is ever skipped.
pub(crate) fn replay(
    original: &SaveDocument,
    changes: &[PendingChange],
    classifier: &dyn GearClassifier,
) -> Result<Replay> {
    let mut out = Replay::unchanged(original);
    for change in changes {
        apply(&mut out, change, classifier).map_err(|e| e.for_change(change.id))?;
    }
    Ok(out)
}

fn bank_used(doc: &SaveDocument) -> usize {
    doc.inventory().iter().filter(|e| e.location == ItemLocation::Bank).count()
}

fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn apply(out: &mut Replay, change: &PendingChange, classifier: &dyn GearClassifier) -> Result<()> {
    // Each engine edit is all-or-nothing, and the checks that follow an edit run on a copy,
    // so a failed change leaves `out` as it was.
    let mut doc = out.doc.clone();
    let target = match change.edit.item() {
        Some(at) => {
            let now = resolve(&doc, at, change.target.as_ref())?;
            let level = doc.inventory().into_iter().find(|e| e.entry_ref == now).and_then(|e| e.fields?.grade_index);
            out.targets.push((change.id, now, level));
            Some(now)
        }
        None => None,
    };
    let item = || target.ok_or_else(|| CoreError::new(ErrorKind::InvalidValue, "This change names no item."));
    match &change.edit {
        Edit::Name(name) => {
            if name.chars().any(char::is_control) {
                return fail(ErrorKind::InvalidValue, "Name can't contain line breaks or control characters.");
            }
            doc.set_name(name).map_err(edit_error)?;
        }
        Edit::Level(level) => doc.set_level(*level).map_err(edit_error)?,
        Edit::Currency(currency, value) => doc.set_currency(*currency, *value).map_err(edit_error)?,
        Edit::BackpackSlots(slots) => {
            let before = doc.backpack_slots();
            doc.set_backpack_slots(*slots).map_err(edit_error)?;
            let used = count(doc.backpack_used());
            if doc.backpack_slots() < before && doc.backpack_slots() < used {
                return fail(
                    ErrorKind::InvalidValue,
                    format!("The backpack holds {used} items, so it can't be made smaller than that."),
                );
            }
        }
        Edit::BankSlots(slots) => {
            let before = doc.bank_slots();
            doc.set_bank_slots(*slots).map_err(edit_error)?;
            let used = count(bank_used(&doc));
            if doc.bank_slots() < before && doc.bank_slots() < used {
                return fail(
                    ErrorKind::InvalidValue,
                    format!("The bank holds {used} items, so it can't be made smaller than that."),
                );
            }
        }
        Edit::GunSlots(slots) => doc.set_gun_slots(*slots).map_err(edit_error)?,
        Edit::OpLevel(level) => doc.set_op_level(*level).map_err(edit_error)?,
        Edit::UnlockTvhm => doc.unlock_tvhm().map_err(edit_error)?,
        Edit::UnlockUvhm => doc.unlock_uvhm().map_err(edit_error)?,
        Edit::UnlockSlaughterdome => doc.unlock_slaughterdome().map_err(edit_error)?,
        Edit::UnlockAmmo => doc.unlock_ammo().map_err(edit_error)?,
        Edit::MaximizeAmmo => doc.maximize_ammo().map_err(edit_error)?,
        Edit::ItemLevels { level, force } => {
            let target = if *level > 0 { *level } else { doc.level() };
            let affected: Vec<EntryRef> = doc
                .inventory()
                .iter()
                .filter(|e| e.location != ItemLocation::Bank && e.blob.is_some() && !e.is_virtual)
                .filter(|e| e.fields.as_ref().and_then(|f| f.grade_index).is_some_and(|grade| *force || grade > 1))
                .map(|e| e.entry_ref)
                .collect();
            doc.set_item_levels(*level, *force).map_err(edit_error)?;
            // The engine accepted the level, so it fits in 7 bits.
            let target = u32::try_from(target).unwrap_or(0);
            out.item_levels.retain(|(entry, _)| !affected.contains(entry));
            out.item_levels.extend(affected.into_iter().map(|entry| (entry, target)));
        }
        Edit::ImportCodes { codes, to_bank } => {
            if codes.is_empty() {
                return fail(ErrorKind::InvalidValue, "There are no item codes to import.");
            }
            let mut blobs = Vec::with_capacity(codes.len());
            for (index, code) in codes.iter().enumerate() {
                blobs.push(importable(code).map_err(|why| {
                    CoreError::new(ErrorKind::InvalidValue, format!("Item code {} can't be imported: {why}.", index + 1))
                })?);
            }
            let mut keys = change.keys.iter().copied();
            let mut next_key = move || keys.next().unwrap_or(1);
            let refs = doc.import_codes_with(codes, *to_bank, &mut next_key).map_err(edit_error)?;
            if *to_bank {
                if count(bank_used(&doc)) > doc.bank_slots() {
                    return fail(ErrorKind::BankFull, BANK_FULL_MESSAGE);
                }
            } else if count(doc.backpack_used()) > doc.backpack_slots() {
                return fail(ErrorKind::BackpackFull, BACKPACK_FULL_MESSAGE);
            }
            for blob in &blobs {
                out.count_added(blob);
            }
            out.created.push((change.id, refs));
        }
        Edit::Duplicate { .. } => {
            let source = &item()?;
            let key = change.keys.first().copied().unwrap_or(1);
            let mut next_key = move || key;
            let source_blob = doc.inventory().into_iter().find(|e| e.entry_ref == *source).and_then(|e| e.blob);
            // The engine refuses anything that is not gear, and a full backpack.
            let copy = doc.duplicate_with(*source, classifier, &mut next_key).map_err(edit_error)?;
            let Some(source_blob) = source_blob else {
                return fail(ErrorKind::NotDuplicable, "This item can't be duplicated: its data could not be decoded.");
            };
            out.count_added(&source_blob);
            out.created.push((change.id, vec![copy]));
            out.duplicate_sources.push((change.id, *source));
        }
        Edit::ItemLevel { level, .. } => {
            let at = item()?;
            if !(1..=MAX_GEAR_LEVEL).contains(level) {
                return fail(ErrorKind::InvalidValue, format!("Item level must be between 1 and {MAX_GEAR_LEVEL}."));
            }
            doc.set_item_level(at, *level).map_err(|e| item_error("leveled", e))?;
            let level = u32::try_from(*level).unwrap_or(0);
            out.item_levels.retain(|(entry, _)| *entry != at);
            out.item_levels.push((at, level));
        }
        Edit::MoveItem { to_bank, .. } => {
            let at = item()?;
            let entry = doc.inventory().into_iter().find(|e| e.entry_ref == at);
            if let Some(entry) = entry.as_ref().filter(|e| e.location != ItemLocation::Equipped && !e.is_virtual) {
                if let Some(fields) = &entry.fields {
                    match classifier.gear_class(fields, entry.is_weapon) {
                        GearClass::Gear => {}
                        GearClass::NotGear => {
                            return fail(ErrorKind::InvalidValue, "Only weapons and gear can go in the bank, not story or mission items.")
                        }
                        GearClass::Unknown => {
                            return fail(ErrorKind::InvalidValue, "The game's item data does not say what this is, so it can't be moved.")
                        }
                    }
                }
            }
            let before = doc.clone();
            let to = doc.move_item(at, *to_bank).map_err(|e| item_error("moved", e))?;
            if *to_bank {
                if count(bank_used(&doc)) > doc.bank_slots() {
                    return fail(ErrorKind::BankFull, BANK_FULL_MESSAGE);
                }
            } else if count(doc.backpack_used()) > doc.backpack_slots() {
                return fail(ErrorKind::BackpackFull, BACKPACK_FULL_MESSAGE);
            }
            out.relocate(&before, at, to);
        }
        Edit::AmmoLevel { ammo, level } => {
            let level = u64::try_from(*level)
                .ok()
                .filter(|l| *l <= MAX_AMMO_SDU_LEVEL)
                .ok_or_else(|| CoreError::new(ErrorKind::InvalidValue, format!("Ammo upgrades go from 0 to {MAX_AMMO_SDU_LEVEL}.")))?;
            doc.set_ammo_level(*ammo, level).map_err(edit_error)?;
        }
    }
    out.doc = doc;
    Ok(())
}

/// An engine refusal of an edit aimed at one item, in words: "This item can't be moved: it
/// is equipped."
fn item_error(done: &str, error: SaveKitError) -> CoreError {
    match error {
        SaveKitError::InvalidValue { field, reason } if field == "itemTarget" => {
            let advice = if reason == "it is equipped" { " Unequip it in the game first." } else { "" };
            CoreError::new(ErrorKind::InvalidValue, format!("This item can't be {done}: {reason}.{advice}"))
        }
        other => edit_error(other),
    }
}

/// The item in an item code, if it is one that may be imported.
fn importable(code: &str) -> std::result::Result<ItemBlob, String> {
    let blob = ItemBlob::from_code(code).map_err(|e| match e {
        SaveKitError::InvalidValue { reason, .. } => reason,
        SaveKitError::ItemChecksumMismatch => "the code is damaged (its checksum does not match)".to_string(),
        other => other.to_string(),
    })?;
    if blob.version != BL2_ITEM_VERSION {
        return Err(format!("it is not a Borderlands 2 item (item version {})", blob.version));
    }
    if ItemFields::from_blob(&blob).is_virtual() {
        return Err("it is a placeholder record, not an item".to_string());
    }
    Ok(blob)
}

/// An engine error in words for the user.
fn edit_error(error: SaveKitError) -> CoreError {
    match error {
        SaveKitError::BackpackFull => CoreError::new(ErrorKind::BackpackFull, BACKPACK_FULL_MESSAGE),
        SaveKitError::NotDuplicable(why) => {
            CoreError::new(ErrorKind::NotDuplicable, format!("This item can't be duplicated: {why}."))
        }
        SaveKitError::InvalidValue { field, reason } => {
            let message = match field.as_str() {
                "name" if reason.contains("empty") => "Name can't be empty.".to_string(),
                "name" => "Name can only use Latin letters (no emoji).".to_string(),
                "level" => format!("Level must be between {MIN_LEVEL} and {MAX_LEVEL}."),
                "currency" => format!("Must be between 0 and {}.", format::number(MAX_CURRENCY)),
                "backpackSlots" => format!("Backpack must be {MIN_BACKPACK_SLOTS}–{MAX_BACKPACK_SLOTS} slots."),
                "bankSlots" => format!("Bank must be {MIN_BANK_SLOTS}–{MAX_BANK_SLOTS} slots."),
                "gunSlots" => "Gun slots must be 2, 3 or 4.".to_string(),
                "opLevel" => format!("Overpower level must be 0–{MAX_OP_LEVEL}."),
                "itemLevel" => {
                    format!("Item level must be between 0 and {MAX_ITEM_LEVEL} (0 means the character's level).")
                }
                "item" => format!("That item code can't be used: {reason}."),
                _ => format!(
                    "This save is laid out in a way Vault Key does not expect ({field}: {reason}), so this change can't be made."
                ),
            };
            CoreError::new(ErrorKind::InvalidValue, message)
        }
        other => CoreError::new(ErrorKind::InvalidValue, format!("This change can't be made: {other}.")),
    }
}

/// What the written file must satisfy, derived from the pending changes: expectations for
/// the engine's verifier, and Vault Key's own checks for what the verifier cannot express.
pub(crate) fn verification(
    original: &SaveDocument,
    changes: &[PendingChange],
    replayed: &Replay,
) -> (Vec<Expectation>, Vec<Check>) {
    let mut expectations = Vec::new();
    let mut checks = Vec::new();
    let value = |key: CharacterKey, n: i64| Expectation::CharacterValue(key, n);
    for change in changes {
        match &change.edit {
            Edit::Name(name) => expectations.push(Expectation::Name(name.clone())),
            Edit::Level(level) => expectations.push(value(CharacterKey::Level, *level)),
            Edit::Currency(currency, amount) => {
                let key = match currency {
                    Currency::Money => CharacterKey::Money,
                    Currency::Eridium => CharacterKey::Eridium,
                    Currency::Seraph => CharacterKey::Seraph,
                    Currency::Torgue => CharacterKey::Torgue,
                };
                expectations.push(value(key, *amount));
            }
            // Sizes are rounded up to a whole upgrade, so expect what was applied, and check
            // separately that it is no less than what was asked for.
            Edit::BackpackSlots(asked) => {
                expectations.push(value(CharacterKey::BackpackSlots, replayed.doc.backpack_slots()));
                checks.push(Check::BackpackAtLeast(*asked));
            }
            Edit::BankSlots(asked) => {
                expectations.push(value(CharacterKey::BankSlots, replayed.doc.bank_slots()));
                checks.push(Check::BankAtLeast(*asked));
            }
            Edit::GunSlots(slots) => expectations.push(value(CharacterKey::GunSlots, *slots)),
            Edit::OpLevel(level) => expectations.push(value(CharacterKey::OpLevel, *level)),
            Edit::UnlockTvhm => checks.push(Check::PlaythroughsAtLeast(1)),
            Edit::UnlockUvhm => checks.push(Check::PlaythroughsAtLeast(2)),
            Edit::UnlockSlaughterdome => checks.push(Check::SlaughterdomeUnlocked),
            // Ammo edits are covered by the whole-document check at the end.
            Edit::UnlockAmmo | Edit::MaximizeAmmo => {}
            Edit::ItemLevels { .. } | Edit::ImportCodes { .. } | Edit::Duplicate { .. } => {}
            // The item's place and level are checked below, from the replay.
            Edit::ItemLevel { .. } | Edit::MoveItem { .. } => {}
            Edit::AmmoLevel { .. } => {}
        }
    }
    checks.extend(replayed.item_levels.iter().map(|(entry, level)| Check::ItemLevel(*entry, *level)));
    let moved = replayed.places.is_some();
    for (from, to) in replayed.places.iter().flatten().filter(|(from, to)| from != to) {
        expectations.push(Expectation::Relocated { from: *from, to: *to });
    }

    if !replayed.added.is_empty() {
        checks.push(Check::InventoryGrowth { added: replayed.added.clone() });
    }
    // The engine's own duplicate check counts every inventory entry, so it applies only when
    // the duplicates are all that was added (no imports, no new Overpower record).
    let duplicates = replayed.duplicate_sources.len();
    let imports = changes.iter().any(|c| matches!(c.edit, Edit::ImportCodes { .. }));
    let original_inventory = original.inventory();
    // The sources are named by where they were when copied, which a move can change, and
    // the engine counts copies by their exact bytes, which a level change changes. In those
    // cases the level-free count above and the exact comparison below cover the copies.
    let leveled = !replayed.item_levels.is_empty();
    if duplicates > 0
        && !imports
        && !moved
        && !leveled
        && replayed.doc.inventory().len() == original_inventory.len() + duplicates
    {
        // One expectation per distinct item: two sources that hold identical items count together.
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        for (_, source) in &replayed.duplicate_sources {
            let bytes_of = |entry: EntryRef| {
                original_inventory.iter().find(|e| e.entry_ref == entry).and_then(|e| e.blob.as_ref()).map(|b| &b.item_bytes)
            };
            let Some(bytes) = bytes_of(*source) else { continue };
            if !seen.insert(bytes.clone()) {
                continue;
            }
            let copies = replayed.duplicate_sources.iter().filter(|(_, other)| bytes_of(*other) == Some(bytes)).count();
            expectations.push(Expectation::Duplicated { source: *source, count: copies });
        }
    }

    checks.push(Check::Equals(replayed.doc.clone()));
    (expectations, checks)
}

fn line(change_id: u64, label: &str, from: Option<String>, to: String, item_ref: Option<String>) -> ChangeLine {
    let text = match &from {
        Some(from) => format!("{label} {from} → {to}"),
        None => format!("{label}: {to}"),
    };
    ChangeLine { change_id, label: label.to_string(), from, to, text, item_ref }
}

/// The pending changes in words, in order.
pub(crate) fn change_lines(original: &SaveDocument, changes: &[PendingChange], replayed: &Replay) -> Vec<ChangeLine> {
    let n = format::number;
    changes
        .iter()
        .map(|change| {
            let id = change.id;
            let plain = |label: &str, from: String, to: String| line(id, label, Some(from), to, None);
            let does = |label: &str, to: &str| line(id, label, None, to.to_string(), None);
            match &change.edit {
                Edit::Name(name) => plain("Name", original.name(), name.clone()),
                Edit::Level(level) => plain("Level", n(original.level()), n(*level)),
                Edit::Currency(currency, amount) => match currency {
                    Currency::Money => plain("Money", format::money(original.money()), format::money(*amount)),
                    Currency::Eridium => plain("Eridium", n(original.eridium()), n(*amount)),
                    Currency::Seraph => plain("Seraph crystals", n(original.seraph()), n(*amount)),
                    Currency::Torgue => plain("Torgue tokens", n(original.torgue()), n(*amount)),
                },
                Edit::BackpackSlots(_) => {
                    plain("Backpack", n(original.backpack_slots()), format!("{} slots", n(replayed.doc.backpack_slots())))
                }
                Edit::BankSlots(_) => {
                    plain("Bank", n(original.bank_slots()), format!("{} slots", n(replayed.doc.bank_slots())))
                }
                Edit::GunSlots(slots) => plain("Gun slots", n(original.gun_slots()), n(*slots)),
                Edit::OpLevel(level) => {
                    plain("Overpower level", format!("OP {}", original.op_level()), format!("OP {level}"))
                }
                Edit::UnlockTvhm => does("Unlock", "True Vault Hunter Mode"),
                Edit::UnlockUvhm => does("Unlock", "Ultimate Vault Hunter Mode"),
                Edit::UnlockSlaughterdome => does("Unlock", "Creature Slaughter Dome"),
                Edit::UnlockAmmo => does("Unlock", "all ammo upgrades"),
                Edit::MaximizeAmmo => does("Ammo", "fill to max"),
                Edit::ItemLevels { level, force } => {
                    let target = if *level > 0 { format!("level {level}") } else { "the character's level".to_string() };
                    let forced = if *force { " (including level 1 items)" } else { "" };
                    does("Item levels", &format!("all gear → {target}{forced}"))
                }
                Edit::ImportCodes { codes, to_bank } => {
                    let items = if codes.len() == 1 { "item" } else { "items" };
                    let place = if *to_bank { "bank" } else { "backpack" };
                    does("Import codes", &format!("+{} {items} in the {place}", codes.len()))
                }
                Edit::ItemLevel { item, level } => {
                    let (at, before) = target_of(replayed, id, *item);
                    let from = before.map_or_else(|| "?".to_string(), |l| l.to_string());
                    line(id, "Item level", Some(from), level.to_string(), Some(at.to_string()))
                }
                Edit::MoveItem { item, to_bank } => {
                    let (at, _) = target_of(replayed, id, *item);
                    let place = if *to_bank { "to the bank" } else { "to the backpack" };
                    line(id, "Move", None, place.to_string(), Some(at.to_string()))
                }
                Edit::AmmoLevel { ammo, level } => {
                    let label = AMMO_LABELS.get(*ammo).copied().unwrap_or("Ammo");
                    let was = original.ammo_levels().get(*ammo).copied().unwrap_or(0);
                    let level = u64::try_from(*level).unwrap_or(0);
                    let at = |l: u64| format!("upgrade {l} ({} max)", n(i64::from(ammo_capacity(*ammo, l))));
                    plain(label, at(was), at(level))
                }
                Edit::Duplicate { source } => {
                    let (source, _) = target_of(replayed, id, *source);
                    let source = &source;
                    let kind = if source.field_number == field::WEAPONS { "weapon" } else { "item" };
                    line(
                        id,
                        "Duplicate",
                        None,
                        format!("+1 copy of {kind} {source} in the backpack"),
                        Some(source.to_string()),
                    )
                }
            }
        })
        .collect()
}

/// Where the item of change `id` is now, and its level before the change.
fn target_of(replayed: &Replay, id: u64, named: EntryRef) -> (EntryRef, Option<u32>) {
    replayed.targets.iter().find(|(change, _, _)| *change == id).map_or((named, None), |(_, at, level)| (*at, *level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{echo_ref, sample_doc, weapon_ref, ITEM_CODE, WEAPON_CODE};
    use savekit::FallbackGearClassifier;

    fn change(id: u64, edit: Edit) -> PendingChange {
        let keys = match &edit {
            Edit::ImportCodes { codes, .. } => (0..codes.len()).map(|i| 5_000 + i as i32).collect(),
            Edit::Duplicate { .. } => vec![4_242],
            _ => Vec::new(),
        };
        PendingChange { id, edit, keys, target: None }
    }

    fn replayed(changes: &[PendingChange]) -> Result<Replay> {
        replay(&sample_doc(3), changes, &FallbackGearClassifier)
    }

    #[test]
    fn replay_equals_direct_application() {
        let changes = [
            change(1, Edit::Name("Kai".to_string())),
            change(2, Edit::Level(30)),
            change(3, Edit::Currency(Currency::Money, 123_456)),
            change(4, Edit::Currency(Currency::Eridium, 99)),
            change(5, Edit::Currency(Currency::Seraph, 7)),
            change(6, Edit::Currency(Currency::Torgue, 8)),
            change(7, Edit::BackpackSlots(38)),
            change(8, Edit::BankSlots(11)),
            change(9, Edit::GunSlots(4)),
            change(10, Edit::OpLevel(3)),
            change(11, Edit::UnlockTvhm),
            change(12, Edit::UnlockUvhm),
            change(13, Edit::UnlockSlaughterdome),
            change(14, Edit::UnlockAmmo),
            change(15, Edit::MaximizeAmmo),
            change(16, Edit::ImportCodes { codes: vec![WEAPON_CODE.to_string(), ITEM_CODE.to_string()], to_bank: true }),
            change(17, Edit::Duplicate { source: weapon_ref() }),
        ];
        let got = replayed(&changes).unwrap();

        let mut direct = sample_doc(3);
        direct.set_name("Kai").unwrap();
        direct.set_level(30).unwrap();
        direct.set_currency(Currency::Money, 123_456).unwrap();
        direct.set_currency(Currency::Eridium, 99).unwrap();
        direct.set_currency(Currency::Seraph, 7).unwrap();
        direct.set_currency(Currency::Torgue, 8).unwrap();
        direct.set_backpack_slots(38).unwrap();
        direct.set_bank_slots(11).unwrap();
        direct.set_gun_slots(4).unwrap();
        direct.set_op_level(3).unwrap();
        direct.unlock_tvhm().unwrap();
        direct.unlock_uvhm().unwrap();
        direct.unlock_slaughterdome().unwrap();
        direct.unlock_ammo().unwrap();
        direct.maximize_ammo().unwrap();
        let mut import_keys = [5_000, 5_001].into_iter();
        direct.import_codes_with(&[WEAPON_CODE, ITEM_CODE], true, &mut || import_keys.next().unwrap()).unwrap();
        direct.duplicate_with(weapon_ref(), &FallbackGearClassifier, &mut || 4_242).unwrap();

        assert_eq!(got.doc, direct);
        assert_eq!(got.doc.to_file_bytes().unwrap(), direct.to_file_bytes().unwrap());
        // And a second replay gives the same bytes again: the keys are part of the change.
        assert_eq!(replayed(&changes).unwrap(), got);
        assert_eq!(got.doc.backpack_slots(), 39);
        assert_eq!(got.doc.bank_slots(), 12);
        assert_eq!(got.created.len(), 2);
        assert_eq!(got.created[1], (17, vec![EntryRef::new(field::WEAPONS, 1)]));
        assert_eq!(got.duplicate_sources, [(17, weapon_ref())]);
        assert_eq!(got.added.iter().map(|(_, n)| n).sum::<usize>(), 3);
    }

    #[test]
    fn item_levels_replay_like_the_engine_and_record_what_they_touch() {
        // With force, every item and weapon outside the bank is touched, level 1 or not.
        let forced = replayed(&[change(1, Edit::ItemLevels { level: 20, force: true })]).unwrap();
        let mut direct = sample_doc(3);
        direct.set_item_levels(20, true).unwrap();
        assert_eq!(forced.doc, direct);
        assert!(!forced.item_levels.is_empty());
        for (entry, level) in &forced.item_levels {
            let item = forced.doc.inventory().into_iter().find(|e| e.entry_ref == *entry).unwrap();
            assert_eq!((item.fields.unwrap().grade_index, *level), (Some(20), 20), "{entry}");
        }
        // Level 0 means the character's level at that point in the list.
        let by_level =
            replayed(&[change(1, Edit::Level(33)), change(2, Edit::ItemLevels { level: 0, force: true })]).unwrap();
        assert!(by_level.item_levels.iter().all(|(_, level)| *level == 33));
        assert!(!by_level.item_levels.is_empty());
    }

    #[test]
    fn a_failing_change_is_reported_by_id_and_nothing_is_skipped() {
        let error = replayed(&[
            change(1, Edit::Level(30)),
            change(2, Edit::Level(81)),
            change(3, Edit::Currency(Currency::Money, 5)),
        ])
        .unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::InvalidValue, Some(2)));
        assert_eq!(error.message, "Level must be between 1 and 80.");
    }

    #[test]
    fn invalid_values_have_messages_a_person_can_act_on() {
        let cases: Vec<(Edit, &str)> = vec![
            (Edit::Level(0), "Level must be between 1 and 80."),
            (Edit::Currency(Currency::Money, -1), "Must be between 0 and 2,147,483,647."),
            (Edit::Currency(Currency::Torgue, 2_147_483_648), "Must be between 0 and 2,147,483,647."),
            (Edit::Name(String::new()), "Name can't be empty."),
            (Edit::Name("Zer0🔥".to_string()), "Name can only use Latin letters (no emoji)."),
            (Edit::Name("two\nlines".to_string()), "Name can't contain line breaks or control characters."),
            (Edit::BackpackSlots(11), "Backpack must be 12–39 slots."),
            (Edit::BackpackSlots(40), "Backpack must be 12–39 slots."),
            (Edit::BankSlots(25), "Bank must be 6–24 slots."),
            (Edit::GunSlots(5), "Gun slots must be 2, 3 or 4."),
            (Edit::OpLevel(11), "Overpower level must be 0–10."),
            (
                Edit::ItemLevels { level: 128, force: false },
                "Item level must be between 0 and 127 (0 means the character's level).",
            ),
            (Edit::ImportCodes { codes: vec![], to_bank: false }, "There are no item codes to import."),
            (
                Edit::ImportCodes { codes: vec![ITEM_CODE.to_string(), "nonsense".to_string()], to_bank: false },
                "Item code 2 can't be imported: item code must look like BL2(...).",
            ),
        ];
        for (edit, message) in cases {
            let error = replayed(&[change(9, edit.clone())]).unwrap_err();
            assert_eq!((error.kind, error.message.as_str(), error.change_id), (ErrorKind::InvalidValue, message, Some(9)), "{edit:?}");
        }
    }

    #[test]
    fn imports_of_things_that_are_not_bl2_items_are_refused() {
        let pre_sequel = ItemBlob::new(true, 10, 0, vec![1, 2, 3, 4, 5, 6, 7, 8]).code();
        let error = replayed(&[change(1, Edit::ImportCodes { codes: vec![pre_sequel], to_bank: false })]).unwrap_err();
        assert!(error.message.contains("not a Borderlands 2 item"), "{}", error.message);
        let mut record = vec![0u8; 33];
        record[0] = 0xFF;
        let placeholder = ItemBlob::new(false, 7, 0, record).code();
        let error = replayed(&[change(1, Edit::ImportCodes { codes: vec![placeholder], to_bank: false })]).unwrap_err();
        assert!(error.message.contains("placeholder record"), "{}", error.message);
        let damaged = ITEM_CODE.replace("B2vu", "B2vv");
        let error = replayed(&[change(1, Edit::ImportCodes { codes: vec![damaged], to_bank: false })]).unwrap_err();
        assert!(error.message.contains("checksum"), "{}", error.message);
    }

    #[test]
    fn duplicate_is_gear_only() {
        let error = replayed(&[change(1, Edit::Duplicate { source: echo_ref() })]).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotDuplicable);
        assert!(error.message.starts_with("This item can't be duplicated: not a gear item"), "{}", error.message);
        let error = replayed(&[change(1, Edit::Duplicate { source: EntryRef::new(54, 9) })]).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotDuplicable);
        // The Overpower record is a placeholder, not an item.
        let error = replayed(&[change(1, Edit::OpLevel(2)), change(2, Edit::Duplicate { source: EntryRef::new(53, 2) })])
            .unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::NotDuplicable, Some(2)));
    }

    #[test]
    fn a_full_backpack_blocks_duplicates_and_imports_with_the_spec_message() {
        // The sample backpack holds 3 of 12. Nine more copies fill it; the tenth is refused.
        let mut changes: Vec<PendingChange> =
            (1..=9).map(|id| change(id, Edit::Duplicate { source: weapon_ref() })).collect();
        assert_eq!(replayed(&changes).unwrap().doc.backpack_used(), 12);
        changes.push(change(10, Edit::Duplicate { source: weapon_ref() }));
        let error = replayed(&changes).unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::BackpackFull, Some(10)));
        assert_eq!(error.message, "Backpack full — free a slot or increase backpack size");

        // An import that does not fit is refused as a whole.
        changes.pop();
        changes.push(change(11, Edit::ImportCodes { codes: vec![ITEM_CODE.to_string()], to_bank: false }));
        assert_eq!(replayed(&changes).unwrap_err().kind, ErrorKind::BackpackFull);
        // More room first, and both fit.
        changes.insert(0, change(12, Edit::BackpackSlots(15)));
        assert_eq!(replayed(&changes).unwrap().doc.backpack_used(), 13);

        // The bank holds 6.
        let seven = vec![ITEM_CODE.to_string(); 7];
        let error = replayed(&[change(1, Edit::ImportCodes { codes: seven.clone(), to_bank: true })]).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::BankFull, "Bank full — free a slot or increase bank size"));
        assert!(replayed(&[change(1, Edit::BankSlots(8)), change(2, Edit::ImportCodes { codes: seven, to_bank: true })]).is_ok());
    }

    #[test]
    fn a_backpack_or_bank_cannot_shrink_below_what_it_holds() {
        let fill: Vec<PendingChange> = std::iter::once(change(1, Edit::BackpackSlots(18)))
            .chain((2..=14).map(|id| change(id, Edit::Duplicate { source: weapon_ref() })))
            .collect();
        assert_eq!(replayed(&fill).unwrap().doc.backpack_used(), 16);
        let mut shrink = fill.clone();
        shrink.push(change(20, Edit::BackpackSlots(15)));
        let error = replayed(&shrink).unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::InvalidValue, Some(20)));
        assert_eq!(error.message, "The backpack holds 16 items, so it can't be made smaller than that.");
        // Shrinking to a size that still fits is fine.
        let mut fits = fill;
        fits.push(change(21, Edit::BackpackSlots(16)));
        assert_eq!(replayed(&fits).unwrap().doc.backpack_slots(), 18);
    }

    fn describe(e: &Expectation) -> String {
        match e {
            Expectation::Duplicated { source, count } => format!("duplicated:{source}x{count}"),
            Expectation::CharacterValue(key, n) => format!("{key}={n}"),
            Expectation::Name(name) => format!("name:{name}"),
            Expectation::Unchanged => "unchanged".to_string(),
            Expectation::Relocated { from, to } => format!("moved:{from}->{to}"),
        }
    }

    fn verification_of(changes: &[PendingChange]) -> (Vec<String>, Vec<Check>) {
        let original = sample_doc(3);
        let replayed = replay(&original, changes, &FallbackGearClassifier).unwrap();
        let (expectations, checks) = verification(&original, changes, &replayed);
        // Whatever was derived must hold for the document it was derived from.
        let written = replayed.doc.to_file_bytes().unwrap();
        savekit::verify(&original, &written, &expectations).unwrap();
        for check in &checks {
            check.run(&original, &replayed.doc).unwrap();
        }
        (expectations.iter().map(describe).collect(), checks)
    }

    #[test]
    fn expectations_follow_the_pending_changes() {
        let (expectations, checks) = verification_of(&[
            change(1, Edit::Name("Kai".to_string())),
            change(2, Edit::Level(30)),
            change(3, Edit::Currency(Currency::Money, 5)),
            change(4, Edit::BackpackSlots(13)),
            change(5, Edit::BankSlots(7)),
            change(6, Edit::GunSlots(3)),
            change(7, Edit::UnlockUvhm),
            change(8, Edit::UnlockSlaughterdome),
            change(9, Edit::MaximizeAmmo),
        ]);
        assert_eq!(expectations, ["name:Kai", "level=30", "money=5", "backpackSlots=15", "bankSlots=8", "gunSlots=3"]);
        assert!(checks.contains(&Check::BackpackAtLeast(13)));
        assert!(checks.contains(&Check::BankAtLeast(7)));
        assert!(checks.contains(&Check::PlaythroughsAtLeast(2)));
        assert!(checks.contains(&Check::SlaughterdomeUnlocked));
        assert!(matches!(checks.last(), Some(Check::Equals(_))));
    }

    #[test]
    fn duplicates_expect_one_new_entry_per_copy() {
        let twice = [change(1, Edit::Duplicate { source: weapon_ref() }), change(2, Edit::Duplicate { source: weapon_ref() })];
        let (expectations, checks) = verification_of(&twice);
        assert_eq!(expectations, ["duplicated:54:0x2"]);
        assert!(checks.iter().any(|c| matches!(c, Check::InventoryGrowth { added } if added.len() == 1 && added[0].1 == 2)));

        // With an import, or a new Overpower record, alongside: the engine's duplicate check
        // would miscount, so Vault Key's own growth check carries it.
        let with_import = [
            change(1, Edit::Duplicate { source: weapon_ref() }),
            change(2, Edit::ImportCodes { codes: vec![ITEM_CODE.to_string()], to_bank: false }),
        ];
        let (expectations, checks) = verification_of(&with_import);
        assert!(expectations.is_empty(), "{expectations:?}");
        assert!(checks.iter().any(|c| matches!(c, Check::InventoryGrowth { added } if added.len() == 2)));

        let with_op = [change(1, Edit::OpLevel(4)), change(2, Edit::Duplicate { source: weapon_ref() })];
        let (expectations, checks) = verification_of(&with_op);
        assert_eq!(expectations, ["opLevel=4"]);
        assert!(checks.iter().any(|c| matches!(c, Check::InventoryGrowth { .. })));
    }

    #[test]
    fn two_sources_holding_identical_items_are_counted_together() {
        // A character that already has two identical weapons (an earlier duplicate).
        let mut original = sample_doc(3);
        original.duplicate_with(weapon_ref(), &FallbackGearClassifier, &mut || 77).unwrap();
        let twin = EntryRef::new(field::WEAPONS, 1);
        let changes = [change(1, Edit::Duplicate { source: weapon_ref() }), change(2, Edit::Duplicate { source: twin })];
        let replayed = replay(&original, &changes, &FallbackGearClassifier).unwrap();
        let (expectations, checks) = verification(&original, &changes, &replayed);
        assert_eq!(expectations.iter().map(describe).collect::<Vec<_>>(), ["duplicated:54:0x2"]);
        savekit::verify(&original, &replayed.doc.to_file_bytes().unwrap(), &expectations).unwrap();
        for check in &checks {
            check.run(&original, &replayed.doc).unwrap();
        }
    }

    #[test]
    fn change_lines_read_like_sentences() {
        let changes = [
            change(1, Edit::Level(30)),
            change(2, Edit::Currency(Currency::Money, 123_456)),
            change(3, Edit::BackpackSlots(38)),
            change(4, Edit::Name("Kai".to_string())),
            change(5, Edit::OpLevel(10)),
            change(6, Edit::UnlockTvhm),
            change(7, Edit::MaximizeAmmo),
            change(8, Edit::Duplicate { source: weapon_ref() }),
            change(9, Edit::ImportCodes { codes: vec![ITEM_CODE.to_string(), WEAPON_CODE.to_string()], to_bank: true }),
            change(10, Edit::Currency(Currency::Eridium, 1_500)),
            change(11, Edit::GunSlots(4)),
            change(12, Edit::BankSlots(24)),
        ];
        let original = sample_doc(3);
        let replayed = replay(&original, &changes, &FallbackGearClassifier).unwrap();
        let lines = change_lines(&original, &changes, &replayed);
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            text,
            [
                "Level 5 → 30",
                "Money $80 → $123,456",
                "Backpack 12 → 39 slots",
                "Name Gaige → Kai",
                "Overpower level OP 0 → OP 10",
                "Unlock: True Vault Hunter Mode",
                "Ammo: fill to max",
                "Duplicate: +1 copy of weapon 54:0 in the backpack",
                "Import codes: +2 items in the bank",
                "Eridium 0 → 1,500",
                "Gun slots 2 → 4",
                "Bank 6 → 24 slots",
            ]
        );
        assert_eq!(
            lines[0],
            ChangeLine {
                change_id: 1,
                label: "Level".to_string(),
                from: Some("5".to_string()),
                to: "30".to_string(),
                text: "Level 5 → 30".to_string(),
                item_ref: None,
            }
        );
        assert_eq!(lines[7].item_ref.as_deref(), Some("54:0"));
        assert_eq!(lines.iter().map(|l| l.change_id).collect::<Vec<_>>(), (1..=12).collect::<Vec<_>>());

        let levels = [change(1, Edit::ItemLevels { level: 30, force: false })];
        let replayed = replay(&original, &levels, &FallbackGearClassifier).unwrap();
        assert_eq!(change_lines(&original, &levels, &replayed)[0].text, "Item levels: all gear → level 30");
        let levels = [change(1, Edit::ItemLevels { level: 0, force: true })];
        let replayed = replay(&original, &levels, &FallbackGearClassifier).unwrap();
        assert_eq!(
            change_lines(&original, &levels, &replayed)[0].text,
            "Item levels: all gear → the character's level (including level 1 items)"
        );
    }
}

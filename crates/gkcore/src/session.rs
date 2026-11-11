//! A character opened for editing: the original bytes, the original document and an
//! ordered list of pending changes. Nothing here touches the disk.
//!
//! The working document is always the original with the pending changes replayed in order.
//! Staging and removing are all-or-nothing: if the resulting list would not replay cleanly,
//! the list stays as it was and the error says which change failed.

use std::fmt;
use std::sync::Arc;

use savekit::{
    EntropyKeys, EntryRef, Expectation, GearClassifier, ItemFields, ItemLocation, KeySource, SaveDocument,
};

use crate::changes::{self, ChangeLine, Edit, ItemIdentity, PendingChange, Replay};
use crate::error::{fail, CoreError, ErrorKind, Result};
use crate::library::{self, CharacterClass};
use crate::slot::SlotId;
use crate::writer::Check;

/// Decides which items are gear and so may be duplicated. Supplied by the caller: built
/// from the game's data when that has been imported, [`savekit::FallbackGearClassifier`]
/// otherwise.
pub type SharedClassifier = Arc<dyn GearClassifier + Send + Sync>;

pub const ITEM_LEVELS_AFTER_NEW_ITEMS_MESSAGE: &str = "Save your new copies first, then change item levels.";
pub const NEW_ITEMS_AFTER_ITEM_LEVELS_MESSAGE: &str = "Save the item-level change first, then add items.";
pub const DUPLICATE_OF_IMPORT_MESSAGE: &str = "Save the imported item first, then duplicate it.";

/// What staging an edit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOutcome {
    /// The pending change that now holds the edit. `None` when the edit changes nothing
    /// (the value is already what was asked for), in which case nothing is pending for it.
    pub change_id: Option<u64>,
    /// Inventory entries the change added to the working document.
    pub new_entries: Vec<EntryRef>,
}

impl StageOutcome {
    pub fn staged(&self) -> bool {
        self.change_id.is_some()
    }
}

/// The character as it would be saved now. Plain data, ready to send to a front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterSnapshot {
    pub name: String,
    pub class_path: String,
    pub class: Option<CharacterClass>,
    pub class_label: String,
    pub level: i64,
    pub experience: i64,
    pub save_game_id: i64,
    pub money: i64,
    pub eridium: i64,
    pub seraph: i64,
    pub torgue: i64,
    pub backpack_slots: i64,
    pub backpack_used: usize,
    pub bank_slots: i64,
    pub bank_used: usize,
    pub gun_slots: i64,
    pub op_level: i64,
    /// 0 = Normal only, 1 = True Vault Hunter Mode unlocked, 2 = Ultimate unlocked.
    pub playthroughs_unlocked: i64,
    pub slaughterdome_unlocked: bool,
    /// Ammo upgrade level (0 to 7) of each type, in [`crate::changes::AMMO_NAMES`] order.
    pub ammo_levels: [u64; savekit::AMMO_SDU_COUNT],
}

impl CharacterSnapshot {
    pub fn of(doc: &SaveDocument) -> CharacterSnapshot {
        let class_path = doc.class_path();
        CharacterSnapshot {
            name: doc.name(),
            class: CharacterClass::from_class_path(&class_path),
            class_label: library::class_label(&class_path),
            class_path,
            level: doc.level(),
            experience: doc.experience(),
            save_game_id: doc.save_game_id(),
            money: doc.money(),
            eridium: doc.eridium(),
            seraph: doc.seraph(),
            torgue: doc.torgue(),
            backpack_slots: doc.backpack_slots(),
            backpack_used: doc.backpack_used(),
            bank_slots: doc.bank_slots(),
            bank_used: doc.inventory().iter().filter(|e| e.location == ItemLocation::Bank).count(),
            gun_slots: doc.gun_slots(),
            op_level: doc.op_level(),
            playthroughs_unlocked: doc.playthroughs_unlocked(),
            slaughterdome_unlocked: doc.slaughterdome_unlocked(),
            ammo_levels: doc.ammo_levels(),
        }
    }
}

/// One item or weapon of the working document. Placeholder records (such as the Overpower
/// level) are not listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    pub entry_ref: EntryRef,
    pub location: ItemLocation,
    pub is_weapon: bool,
    /// The item level shown in game, if the item decodes.
    pub level: Option<u32>,
    /// The decoded item, for looking up its name and rarity in the game data. `None` when
    /// the item's data does not decode.
    pub fields: Option<ItemFields>,
    /// The item's code (`BL2(...)`), if it decodes.
    pub code: Option<String>,
    /// Whether Duplicate Item is offered: gear only, according to the classifier.
    pub can_duplicate: bool,
    /// True for an entry added by a pending change (a staged copy or import).
    pub is_new: bool,
    /// The pending change that added it; removing that change removes the entry.
    pub pending_change_id: Option<u64>,
    /// For a staged copy: the item it copies.
    pub copy_of: Option<EntryRef>,
    /// For an equipped weapon: its weapon slot, 1 to 4.
    pub quick_slot: Option<i64>,
    /// Names this item as long as the character is open, wherever moves put it: from its
    /// key, with a count for items that share one.
    pub uid: String,
}

/// Where an open character came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSource {
    /// A save file in the user's SaveData folder.
    Slot(SlotId),
    /// A save file opened from anywhere else ("Open a save file…"). Vault Key never writes
    /// to it: its only outputs are an export to another file and "Save as a new character".
    External(std::path::PathBuf),
}

/// A character opened for editing.
pub struct CharacterSession {
    source: SessionSource,
    original_bytes: Vec<u8>,
    original: SaveDocument,
    changes: Vec<PendingChange>,
    replayed: Replay,
    classifier: SharedClassifier,
    next_id: u64,
}

impl fmt::Debug for CharacterSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CharacterSession")
            .field("source", &self.source)
            .field("pending", &self.changes)
            .finish_non_exhaustive()
    }
}

impl CharacterSession {
    /// Opens a session on the bytes of a save file. Fails if they do not decode.
    pub fn from_bytes(slot_id: SlotId, bytes: Vec<u8>, classifier: SharedClassifier) -> Result<CharacterSession> {
        Self::from_source(SessionSource::Slot(slot_id), bytes, classifier)
    }

    /// Opens a session on the bytes of a save file from outside the save folder.
    pub fn from_external(path: std::path::PathBuf, bytes: Vec<u8>, classifier: SharedClassifier) -> Result<CharacterSession> {
        Self::from_source(SessionSource::External(path), bytes, classifier)
    }

    fn from_source(source: SessionSource, bytes: Vec<u8>, classifier: SharedClassifier) -> Result<CharacterSession> {
        let file_name = match &source {
            SessionSource::Slot(slot_id) => slot_id.file_name().to_string(),
            SessionSource::External(path) => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
        };
        let original = SaveDocument::from_file_bytes(&bytes).map_err(|e| {
            CoreError::new(ErrorKind::Unreadable, format!("{file_name} can't be opened. {}", library::unreadable_reason(&e)))
        })?;
        Ok(CharacterSession {
            source,
            original_bytes: bytes,
            replayed: Replay::unchanged(&original),
            original,
            changes: Vec::new(),
            classifier,
            next_id: 1,
        })
    }

    /// The slot this character was opened from; `None` for a file opened from elsewhere.
    pub fn slot_id(&self) -> Option<&SlotId> {
        match &self.source {
            SessionSource::Slot(slot_id) => Some(slot_id),
            SessionSource::External(_) => None,
        }
    }

    pub fn source(&self) -> &SessionSource {
        &self.source
    }

    /// The name of the file this character was opened from, such as `Save0002.sav`.
    pub fn file_name(&self) -> String {
        match &self.source {
            SessionSource::Slot(slot_id) => slot_id.file_name().to_string(),
            SessionSource::External(path) => path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        }
    }

    /// The bytes of the file as it was when the session was opened (or last saved).
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn original(&self) -> &SaveDocument {
        &self.original
    }

    /// The original with every pending change applied.
    pub fn working(&self) -> &SaveDocument {
        &self.replayed.doc
    }

    /// The pending changes, in the order they are applied.
    pub fn pending(&self) -> &[PendingChange] {
        &self.changes
    }

    pub fn pending_count(&self) -> usize {
        self.changes.len()
    }

    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// The character as it would be saved now.
    pub fn snapshot(&self) -> CharacterSnapshot {
        CharacterSnapshot::of(&self.replayed.doc)
    }

    /// The character as it is on disk.
    pub fn original_snapshot(&self) -> CharacterSnapshot {
        CharacterSnapshot::of(&self.original)
    }

    /// The pending changes in words, in order.
    pub fn change_lines(&self) -> Vec<ChangeLine> {
        changes::change_lines(&self.original, &self.changes, &self.replayed)
    }

    /// Every item and weapon of the working document, staged copies and imports included.
    pub fn items(&self) -> Vec<ItemRow> {
        let mut seen: Vec<String> = Vec::new();
        self.replayed
            .doc
            .inventory()
            .into_iter()
            .filter(|entry| !entry.is_virtual)
            .map(|entry| {
                let base = match &entry.blob {
                    Some(blob) => format!("{}{:08x}", if blob.is_weapon { "w" } else { "i" }, blob.key as u32),
                    None => format!("x{}", entry.entry_ref),
                };
                let repeats = seen.iter().filter(|known| **known == base).count();
                seen.push(base.clone());
                let uid = if repeats == 0 { base } else { format!("{base}-{}", repeats + 1) };
                let entry_ref = entry.entry_ref;
                let pending_change_id =
                    self.replayed.created.iter().find(|(_, refs)| refs.contains(&entry_ref)).map(|(id, _)| *id);
                let copy_of = pending_change_id
                    .and_then(|id| self.replayed.duplicate_sources.iter().find(|(change, _)| *change == id))
                    .map(|(_, source)| *source);
                // A staged copy can be duplicated again (that copies its source); a staged
                // import has to be saved first.
                let staged_import = pending_change_id.is_some() && copy_of.is_none();
                ItemRow {
                    entry_ref,
                    location: entry.location,
                    is_weapon: entry.is_weapon,
                    level: entry.fields.as_ref().and_then(|f| f.grade_index),
                    code: entry.blob.as_ref().filter(|_| entry.is_exportable()).map(|b| b.code()),
                    can_duplicate: entry.is_duplicable(&*self.classifier) && !staged_import,
                    is_new: pending_change_id.is_some(),
                    pending_change_id,
                    copy_of,
                    quick_slot: entry.quick_slot.filter(|slot| (1..=4).contains(slot)),
                    uid,
                    fields: entry.fields,
                }
            })
            .collect()
    }

    /// Item codes for everything in the working document that can be exported.
    pub fn export_codes(&self, include_bank: bool) -> Vec<String> {
        self.replayed.doc.export_codes(include_bank)
    }

    /// Stages an edit. Nothing is written until the session is saved.
    ///
    /// An edit of a kind that is already pending replaces it (staging level 30 and then 50
    /// leaves one change). An edit that changes nothing is not staged, and removes the
    /// pending change of its kind if there is one. On an error nothing is staged and the
    /// pending list is as it was.
    pub fn stage(&mut self, edit: Edit) -> Result<StageOutcome> {
        let edit = self.normalized(edit)?;
        if matches!(edit, Edit::ItemLevels { .. }) && self.changes.iter().any(|c| c.edit.adds_items()) {
            return fail(ErrorKind::Conflict, ITEM_LEVELS_AFTER_NEW_ITEMS_MESSAGE);
        }
        if edit.adds_items() && self.changes.iter().any(|c| matches!(c.edit, Edit::ItemLevels { .. })) {
            return fail(ErrorKind::Conflict, NEW_ITEMS_AFTER_ITEM_LEVELS_MESSAGE);
        }

        // New items get their keys now, so that every replay gives the same bytes.
        let mut entropy = EntropyKeys::new();
        let keys: Vec<i32> = match &edit {
            Edit::ImportCodes { codes, .. } => codes.iter().map(|_| entropy.next_key()).collect(),
            Edit::Duplicate { .. } => vec![entropy.next_key()],
            _ => Vec::new(),
        };
        // An edit aimed at one item remembers which item, so that it keeps meaning that item
        // whatever happens to the changes before it. (An item that is not there or does not
        // decode has no identity; the replay then refuses the edit.)
        let target = edit.item().and_then(|at| {
            let entry = self.replayed.doc.inventory().into_iter().find(|e| e.entry_ref == at)?;
            ItemIdentity::of(&entry)
        });
        // Moving an item back where it was undoes the pending move instead of adding another.
        if let Edit::MoveItem { to_bank, .. } = &edit {
            let undone = self.changes.iter().find(|c| {
                matches!(c.edit, Edit::MoveItem { to_bank: earlier, .. } if earlier != *to_bank) && target.is_some() && c.target == target
            });
            if let Some(undone) = undone {
                self.remove_change(undone.id)?;
                return Ok(StageOutcome { change_id: None, new_entries: Vec::new() });
            }
        }
        let kind = edit.coalesce_key();
        let existing = kind.and_then(|key| {
            self.changes.iter().position(|c| c.edit.coalesce_key() == Some(key) && c.target == target)
        });
        let id = existing.map_or(self.next_id, |at| self.changes[at].id);
        let change = PendingChange { id, edit, keys, target };
        let mut candidate = self.changes.clone();
        match existing {
            Some(at) => candidate[at] = change,
            None => candidate.push(change),
        }
        let replayed = changes::replay(&self.original, &candidate, &*self.classifier)
            .map_err(|e| self.explain_failure(e, &candidate, id, "This edit would break another pending change"))?;

        if kind.is_some() {
            // Does it change anything? Compare with the same list without it.
            let without: Vec<PendingChange> = candidate.iter().filter(|c| c.id != id).cloned().collect();
            if let Ok(base) = changes::replay(&self.original, &without, &*self.classifier) {
                if base.doc == replayed.doc {
                    self.changes = without;
                    self.replayed = base;
                    return Ok(StageOutcome { change_id: None, new_entries: Vec::new() });
                }
            }
        }

        if existing.is_none() {
            self.next_id += 1;
        }
        let new_entries =
            replayed.created.iter().find(|(change, _)| *change == id).map(|(_, refs)| refs.clone()).unwrap_or_default();
        self.changes = candidate;
        self.replayed = replayed;
        Ok(StageOutcome { change_id: Some(id), new_entries })
    }

    /// Duplicate Item (design section 5a): stages one more copy of a gear item, placed in
    /// the backpack. Refused with a clear message when the item is not gear or the backpack
    /// is full. Duplicating a staged copy copies the item it was copied from.
    pub fn duplicate_item(&mut self, source: EntryRef) -> Result<StageOutcome> {
        self.stage(Edit::Duplicate { source })
    }

    /// Removes one pending change and keeps the others. Refused if another pending change
    /// depends on it (a staged copy that only fits because of a staged backpack increase).
    pub fn remove_change(&mut self, change_id: u64) -> Result<()> {
        if !self.changes.iter().any(|c| c.id == change_id) {
            return fail(ErrorKind::NoSuchChange, "That change is no longer pending.");
        }
        let candidate: Vec<PendingChange> = self.changes.iter().filter(|c| c.id != change_id).cloned().collect();
        let replayed = changes::replay(&self.original, &candidate, &*self.classifier).map_err(|e| {
            self.explain_failure(e, &candidate, change_id, "This change can't be removed while another pending change depends on it")
        })?;
        self.changes = candidate;
        self.replayed = replayed;
        Ok(())
    }

    /// Drops every pending change. The working document is the original again.
    pub fn discard_all(&mut self) {
        self.changes.clear();
        self.replayed = Replay::unchanged(&self.original);
    }

    /// Switches to another classifier (for example once game data has been imported) and
    /// replays the pending changes with it. If a pending change no longer applies, the
    /// error names it, and the session keeps the classifier it had.
    pub fn set_classifier(&mut self, classifier: SharedClassifier) -> Result<()> {
        let replayed = changes::replay(&self.original, &self.changes, &*classifier).map_err(|e| {
            self.explain_failure(e, &self.changes, 0, "A pending change does not work with the new item data")
        })?;
        self.classifier = classifier;
        self.replayed = replayed;
        Ok(())
    }

    /// A replay failure, worded for the user. When the change that failed is not the one
    /// being staged (`acting_on`), the message says which one it was.
    fn explain_failure(&self, error: CoreError, list: &[PendingChange], acting_on: u64, lead: &str) -> CoreError {
        match error.change_id {
            Some(failed) if failed != acting_on => {
                let which = list.iter().find(|c| c.id == failed).map_or_else(String::new, |c| c.edit.summary());
                CoreError {
                    kind: error.kind,
                    message: format!("{lead} ({which}): {}", error.message),
                    change_id: error.change_id,
                }
            }
            _ => error,
        }
    }

    /// A duplicate always names an item of the original document: a copy of a staged copy
    /// copies that copy's source, so removing one staged change never changes what another
    /// one means.
    fn normalized(&self, edit: Edit) -> Result<Edit> {
        let Edit::Duplicate { source } = edit else { return Ok(edit) };
        let Some((creator, _)) = self.replayed.created.iter().find(|(_, refs)| refs.contains(&source)) else {
            return Ok(edit);
        };
        match self.replayed.duplicate_sources.iter().find(|(change, _)| change == creator) {
            Some((_, original_source)) => Ok(Edit::Duplicate { source: *original_source }),
            None => fail(ErrorKind::Conflict, DUPLICATE_OF_IMPORT_MESSAGE),
        }
    }

    /// What a file written from this session must satisfy.
    pub(crate) fn verification(&self) -> (Vec<Expectation>, Vec<Check>) {
        changes::verification(&self.original, &self.changes, &self.replayed)
    }

    /// After a successful save in place: the written bytes are the new original, and
    /// nothing is pending.
    pub(crate) fn rebase(&mut self, written: Vec<u8>, document: SaveDocument) {
        self.original_bytes = written;
        self.original = document;
        self.discard_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{echo_ref, sample_bytes, sample_doc, weapon_ref, ITEM_CODE, STEAM_ID, WEAPON_CODE};
    use savekit::{field, Currency, FallbackGearClassifier, GearClass};

    fn session() -> CharacterSession {
        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();
        CharacterSession::from_bytes(slot, sample_bytes(3), Arc::new(FallbackGearClassifier)).unwrap()
    }

    fn ids(session: &CharacterSession) -> Vec<u64> {
        session.pending().iter().map(|c| c.id).collect()
    }

    #[test]
    fn opens_with_nothing_pending_and_rejects_garbage() {
        let s = session();
        assert_eq!(s.original_bytes(), sample_bytes(3));
        assert_eq!(s.working(), s.original());
        assert_eq!(*s.original(), sample_doc(3));
        assert!(!s.has_changes() && s.change_lines().is_empty());
        let snapshot = s.snapshot();
        assert_eq!((snapshot.name.as_str(), snapshot.level, snapshot.save_game_id), ("Gaige", 5, 3));
        assert_eq!((snapshot.class_label.as_str(), snapshot.backpack_slots, snapshot.backpack_used), ("Mechromancer", 12, 3));
        assert_eq!((snapshot.money, snapshot.bank_slots, snapshot.bank_used, snapshot.gun_slots), (80, 6, 0, 2));

        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();
        let error = CharacterSession::from_bytes(slot, b"junk".to_vec(), Arc::new(FallbackGearClassifier)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unreadable);
        assert!(error.message.starts_with("Save0003.sav can't be opened."), "{}", error.message);
    }

    #[test]
    fn working_document_is_the_original_with_the_changes_replayed() {
        let mut s = session();
        s.stage(Edit::Level(30)).unwrap();
        s.stage(Edit::Currency(Currency::Money, 123_456)).unwrap();
        s.stage(Edit::BackpackSlots(39)).unwrap();
        s.stage(Edit::Name("Kai".to_string())).unwrap();
        s.stage(Edit::UnlockUvhm).unwrap();

        let mut direct = sample_doc(3);
        direct.set_level(30).unwrap();
        direct.set_currency(Currency::Money, 123_456).unwrap();
        direct.set_backpack_slots(39).unwrap();
        direct.set_name("Kai").unwrap();
        direct.unlock_uvhm().unwrap();
        assert_eq!(*s.working(), direct);
        assert_eq!(*s.original(), sample_doc(3), "the original is never edited");
        assert_eq!(s.original_bytes(), sample_bytes(3));
        assert_eq!(s.pending_count(), 5);
        assert_eq!(s.snapshot().level, 30);
        assert_eq!(s.original_snapshot().level, 5);
        assert_eq!(s.change_lines()[0].text, "Level 5 → 30");
    }

    #[test]
    fn removing_one_change_leaves_the_others() {
        let mut s = session();
        let level = s.stage(Edit::Level(30)).unwrap().change_id.unwrap();
        let money = s.stage(Edit::Currency(Currency::Money, 999)).unwrap().change_id.unwrap();
        let name = s.stage(Edit::Name("Kai".to_string())).unwrap().change_id.unwrap();
        assert_eq!(ids(&s), [level, money, name]);

        s.remove_change(money).unwrap();
        assert_eq!(ids(&s), [level, name]);
        let mut direct = sample_doc(3);
        direct.set_level(30).unwrap();
        direct.set_name("Kai").unwrap();
        assert_eq!(*s.working(), direct);

        assert_eq!(s.remove_change(money).unwrap_err().kind, ErrorKind::NoSuchChange);
        assert_eq!(ids(&s), [level, name]);

        s.discard_all();
        assert!(!s.has_changes());
        assert_eq!(*s.working(), sample_doc(3));
        // Ids are never reused, so a stale id can never remove a newer change.
        let again = s.stage(Edit::Level(31)).unwrap().change_id.unwrap();
        assert!(again > name);
    }

    #[test]
    fn a_duplicate_staged_then_removed_leaves_the_document_identical_to_the_original() {
        let mut s = session();
        let outcome = s.duplicate_item(weapon_ref()).unwrap();
        let copy = EntryRef::new(field::WEAPONS, 1);
        assert_eq!(outcome.new_entries, [copy]);
        assert_eq!(s.snapshot().backpack_used, 4);
        assert_ne!(s.working(), s.original());

        // The copy is the same item under a different key, unequipped, in the backpack.
        let inventory = s.working().inventory();
        let source = inventory.iter().find(|e| e.entry_ref == weapon_ref()).unwrap();
        let new = inventory.iter().find(|e| e.entry_ref == copy).unwrap();
        assert_eq!(new.blob.as_ref().unwrap().item_bytes, source.blob.as_ref().unwrap().item_bytes);
        assert_ne!(new.blob.as_ref().unwrap().key, source.blob.as_ref().unwrap().key);
        assert_eq!(new.location, ItemLocation::Backpack);

        // It shows in the item list as new, and names the change that removes it.
        let row = s.items().into_iter().find(|r| r.entry_ref == copy).unwrap();
        assert!(row.is_new && row.can_duplicate);
        assert_eq!((row.pending_change_id, row.copy_of), (outcome.change_id, Some(weapon_ref())));

        s.remove_change(outcome.change_id.unwrap()).unwrap();
        assert_eq!(s.working(), s.original());
        assert_eq!(s.working().to_file_bytes().unwrap(), s.original().to_file_bytes().unwrap());
        assert!(!s.has_changes());
        assert!(s.items().iter().all(|r| !r.is_new));
    }

    #[test]
    fn one_copy_per_action_and_each_can_be_removed_on_its_own() {
        let mut s = session();
        let first = s.duplicate_item(weapon_ref()).unwrap().change_id.unwrap();
        let second = s.duplicate_item(weapon_ref()).unwrap().change_id.unwrap();
        let third = s.duplicate_item(EntryRef::new(field::ITEMS, 0)).unwrap().change_id.unwrap();
        assert_eq!(s.pending_count(), 3);
        assert_eq!(s.snapshot().backpack_used, 6);
        // The two weapon copies have different keys: each replay keeps its own.
        let keys: Vec<i32> =
            s.working().inventory().iter().filter(|e| e.is_weapon).map(|e| e.blob.as_ref().unwrap().key).collect();
        assert_eq!(keys.len(), 3);
        assert!(keys[0] != keys[1] && keys[1] != keys[2] && keys[0] != keys[2]);

        let before = s.working().clone();
        s.remove_change(second).unwrap();
        assert_eq!(ids(&s), [first, third]);
        assert_eq!(s.snapshot().backpack_used, 5);
        // The copy that stays is byte-for-byte the one that was staged first.
        let kept = |doc: &SaveDocument| doc.inventory().into_iter().find(|e| e.entry_ref == EntryRef::new(field::WEAPONS, 1)).unwrap();
        assert_eq!(kept(s.working()), kept(&before));
    }

    #[test]
    fn duplicating_a_staged_copy_copies_its_source() {
        let mut s = session();
        let first = s.duplicate_item(weapon_ref()).unwrap();
        let copy = first.new_entries[0];
        let second = s.duplicate_item(copy).unwrap();
        assert_eq!(s.pending()[1].edit, Edit::Duplicate { source: weapon_ref() });
        // So removing the first copy does not break, or change the meaning of, the second.
        s.remove_change(first.change_id.unwrap()).unwrap();
        assert_eq!(ids(&s), [second.change_id.unwrap()]);
        assert_eq!(s.working().inventory().iter().filter(|e| e.is_weapon).count(), 2);
    }

    #[test]
    fn only_gear_can_be_duplicated() {
        let mut s = session();
        let error = s.duplicate_item(echo_ref()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotDuplicable);
        assert!(error.message.contains("not a gear item"), "{}", error.message);
        assert_eq!(s.duplicate_item(EntryRef::new(54, 40)).unwrap_err().kind, ErrorKind::NotDuplicable);
        assert!(!s.has_changes());
        let rows = s.items();
        assert_eq!(rows.len(), 3);
        let can: Vec<(String, bool)> = rows.iter().map(|r| (r.entry_ref.to_string(), r.can_duplicate)).collect();
        assert_eq!(can, [("53:0".to_string(), true), ("53:1".to_string(), false), ("54:0".to_string(), true)]);
        assert!(rows.iter().all(|r| r.code.as_ref().is_some_and(|c| c.starts_with("BL2("))));

        // The classifier the caller supplies decides: with one that knows nothing, nothing is gear.
        struct KnowsNothing;
        impl GearClassifier for KnowsNothing {
            fn gear_class(&self, _: &ItemFields, _: bool) -> GearClass {
                GearClass::Unknown
            }
        }
        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();
        let mut strict = CharacterSession::from_bytes(slot, sample_bytes(3), Arc::new(KnowsNothing)).unwrap();
        assert!(strict.items().iter().all(|r| !r.can_duplicate));
        let error = strict.duplicate_item(weapon_ref()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotDuplicable);
        assert!(error.message.contains("unrecognized item"), "{}", error.message);
    }

    #[test]
    fn a_full_backpack_blocks_the_copy_with_the_spec_message() {
        let mut s = session();
        for _ in 0..9 {
            s.duplicate_item(weapon_ref()).unwrap();
        }
        assert_eq!(s.snapshot().backpack_used, 12);
        let before = s.pending().to_vec();
        let error = s.duplicate_item(weapon_ref()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BackpackFull);
        assert_eq!(error.message, "Backpack full — free a slot or increase backpack size");
        assert_eq!(s.pending(), before, "nothing is staged by a refused edit");

        // More room, and it fits.
        s.stage(Edit::BackpackSlots(15)).unwrap();
        s.duplicate_item(weapon_ref()).unwrap();
        assert_eq!(s.snapshot().backpack_used, 13);
    }

    #[test]
    fn a_change_another_depends_on_cannot_be_removed_or_undone_by_restaging() {
        let mut s = session();
        for _ in 0..9 {
            s.duplicate_item(weapon_ref()).unwrap();
        }
        // The size change is staged after the copies, but replays in list order all the
        // same: the tenth copy only fits because of it.
        let bigger = s.stage(Edit::BackpackSlots(15)).unwrap().change_id.unwrap();
        let tenth = s.duplicate_item(weapon_ref()).unwrap().change_id.unwrap();
        let before = s.pending().to_vec();
        let working = s.working().clone();

        let error = s.remove_change(bigger).unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::BackpackFull, Some(tenth)));
        assert!(error.message.starts_with("This change can't be removed while another pending change depends on it (Duplicate 54:0)"), "{}", error.message);
        assert_eq!(s.pending(), before);
        assert_eq!(*s.working(), working);

        // Setting the size back to the original value would have the same effect.
        let error = s.stage(Edit::BackpackSlots(12)).unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::BackpackFull, Some(tenth)));
        assert!(error.message.starts_with("This edit would break another pending change (Duplicate 54:0)"), "{}", error.message);
        assert_eq!(s.pending(), before);

        // Remove the copy first, and then the size change can go.
        s.remove_change(tenth).unwrap();
        s.remove_change(bigger).unwrap();
        assert_eq!(s.pending_count(), 9);
    }

    #[test]
    fn value_edits_replace_each_other_and_no_ops_are_dropped() {
        let mut s = session();
        let first = s.stage(Edit::Currency(Currency::Money, 5)).unwrap().change_id.unwrap();
        s.stage(Edit::Level(30)).unwrap();
        let second = s.stage(Edit::Currency(Currency::Money, 7)).unwrap().change_id.unwrap();
        assert_eq!(first, second, "the pending change keeps its id and its place");
        assert_eq!(s.pending()[0].edit, Edit::Currency(Currency::Money, 7));
        assert_eq!(s.pending_count(), 2);
        // Other currencies are their own changes.
        s.stage(Edit::Currency(Currency::Eridium, 7)).unwrap();
        assert_eq!(s.pending_count(), 3);

        // Back to the value on disk: the change goes away.
        let outcome = s.stage(Edit::Currency(Currency::Money, 80)).unwrap();
        assert!(!outcome.staged());
        assert_eq!(s.pending_count(), 2);
        assert_eq!(s.snapshot().money, 80);

        // A value that is already there is never staged. Sizes compare after rounding.
        assert!(!s.stage(Edit::GunSlots(2)).unwrap().staged());
        assert!(!s.stage(Edit::BackpackSlots(12)).unwrap().staged());
        assert!(!s.stage(Edit::Name("Gaige".to_string())).unwrap().staged());
        assert!(s.stage(Edit::BackpackSlots(13)).unwrap().staged());
        assert_eq!(s.snapshot().backpack_slots, 15);
        assert_eq!(s.pending_count(), 3);
    }

    #[test]
    fn unlocks_already_in_place_are_not_staged() {
        let mut s = session();
        assert!(s.stage(Edit::UnlockUvhm).unwrap().staged());
        assert!(!s.stage(Edit::UnlockTvhm).unwrap().staged(), "Ultimate already includes True Vault Hunter Mode");
        assert!(s.stage(Edit::UnlockSlaughterdome).unwrap().staged());
        assert!(s.stage(Edit::UnlockSlaughterdome).unwrap().staged());
        assert_eq!(s.pending_count(), 2);
        assert!(s.stage(Edit::UnlockAmmo).unwrap().staged());
        assert!(!s.stage(Edit::MaximizeAmmo).unwrap().staged(), "unlocking the upgrades already fills the pools");
        assert_eq!(s.snapshot().playthroughs_unlocked, 2);
        assert!(s.snapshot().slaughterdome_unlocked);
    }

    #[test]
    fn invalid_edits_stage_nothing_and_keep_the_list() {
        let mut s = session();
        s.stage(Edit::Level(30)).unwrap();
        let before = s.pending().to_vec();
        for edit in [
            Edit::Level(81),
            Edit::Currency(Currency::Money, -1),
            Edit::Name(String::new()),
            Edit::Name("Zer0🔥".to_string()),
            Edit::GunSlots(5),
            Edit::OpLevel(11),
            Edit::BackpackSlots(40),
            Edit::ItemLevels { level: 128, force: false },
            Edit::ImportCodes { codes: vec!["BL2(nope)".to_string()], to_bank: false },
        ] {
            let error = s.stage(edit.clone()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidValue, "{edit:?}");
            assert_eq!(s.pending(), before, "{edit:?}");
        }
        assert_eq!(s.snapshot().level, 30);
    }

    #[test]
    fn imports_are_staged_as_one_change_and_can_be_removed() {
        let mut s = session();
        let outcome =
            s.stage(Edit::ImportCodes { codes: vec![WEAPON_CODE.to_string(), ITEM_CODE.to_string()], to_bank: false }).unwrap();
        assert_eq!(outcome.new_entries, [EntryRef::new(field::WEAPONS, 1), EntryRef::new(field::ITEMS, 2)]);
        assert_eq!(s.snapshot().backpack_used, 5);
        let to_bank = s.stage(Edit::ImportCodes { codes: vec![ITEM_CODE.to_string()], to_bank: true }).unwrap();
        assert_eq!(s.snapshot().bank_used, 1);
        assert_eq!(s.pending_count(), 2);

        // A staged import shows as new; it has to be saved before it can be duplicated.
        let row = s.items().into_iter().find(|r| r.entry_ref == outcome.new_entries[0]).unwrap();
        assert!(row.is_new && !row.can_duplicate && row.copy_of.is_none());
        let error = s.duplicate_item(outcome.new_entries[0]).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::Conflict, DUPLICATE_OF_IMPORT_MESSAGE));

        s.remove_change(outcome.change_id.unwrap()).unwrap();
        assert_eq!((s.snapshot().backpack_used, s.snapshot().bank_used), (3, 1));
        s.remove_change(to_bank.change_id.unwrap()).unwrap();
        assert_eq!(s.working(), s.original());
    }

    #[test]
    fn item_levels_and_new_items_are_saved_separately() {
        let mut s = session();
        s.duplicate_item(weapon_ref()).unwrap();
        let error = s.stage(Edit::ItemLevels { level: 30, force: true }).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::Conflict, ITEM_LEVELS_AFTER_NEW_ITEMS_MESSAGE));
        assert_eq!(s.pending_count(), 1);

        s.discard_all();
        assert!(s.stage(Edit::ItemLevels { level: 30, force: true }).unwrap().staged());
        let error = s.duplicate_item(weapon_ref()).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::Conflict, NEW_ITEMS_AFTER_ITEM_LEVELS_MESSAGE));
        let error = s.stage(Edit::ImportCodes { codes: vec![ITEM_CODE.to_string()], to_bank: true }).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        assert_eq!(s.pending_count(), 1);
        assert!(s.items().iter().all(|r| r.level == Some(30)));
    }

    #[test]
    fn swapping_the_classifier_replays_the_pending_changes() {
        struct NothingIsGear;
        impl GearClassifier for NothingIsGear {
            fn gear_class(&self, _: &ItemFields, _: bool) -> GearClass {
                GearClass::NotGear
            }
        }
        let mut s = session();
        s.stage(Edit::Level(30)).unwrap();
        let copy = s.duplicate_item(weapon_ref()).unwrap().change_id.unwrap();
        let working = s.working().clone();

        // The staged copy is no longer allowed: the error names it, and nothing is dropped.
        let error = s.set_classifier(Arc::new(NothingIsGear)).unwrap_err();
        assert_eq!((error.kind, error.change_id), (ErrorKind::NotDuplicable, Some(copy)));
        assert!(error.message.starts_with("A pending change does not work with the new item data (Duplicate 54:0)"), "{}", error.message);
        assert_eq!(s.pending_count(), 2);
        assert_eq!(*s.working(), working);
        assert!(s.items().iter().any(|r| r.can_duplicate), "the old classifier is still in use");

        // Without the copy, the swap goes through.
        s.remove_change(copy).unwrap();
        s.set_classifier(Arc::new(NothingIsGear)).unwrap();
        assert!(s.items().iter().all(|r| !r.can_duplicate));
        assert_eq!(s.snapshot().level, 30);
    }

    #[test]
    fn sessions_can_cross_threads() {
        fn assert_send<T: Send>() {}
        assert_send::<CharacterSession>();
    }

    /// A session on a save with three gear weapons in the backpack (54:0 to 54:2), a gear
    /// item (53:0) and a story item (53:1).
    fn three_weapons() -> CharacterSession {
        let mut doc = sample_doc(3);
        let mut key = 7_000;
        doc.import_codes_with(&[WEAPON_CODE, WEAPON_CODE], false, &mut || {
            key += 1;
            key
        })
        .unwrap();
        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();
        CharacterSession::from_bytes(slot, doc.to_file_bytes().unwrap(), Arc::new(FallbackGearClassifier)).unwrap()
    }

    fn at(text: &str) -> EntryRef {
        text.parse().unwrap()
    }

    fn level_at(s: &CharacterSession, entry: &str) -> Option<u32> {
        s.items().into_iter().find(|i| i.entry_ref == at(entry)).and_then(|i| i.level)
    }

    /// What a file written from the session must satisfy holds for the working document.
    fn assert_verifies(s: &CharacterSession) {
        let (expectations, checks) = s.verification();
        let written = s.working().to_file_bytes().unwrap();
        savekit::verify(s.original(), &written, &expectations).unwrap();
        let back = SaveDocument::from_file_bytes(&written).unwrap();
        for check in &checks {
            check.run(s.original(), &back).unwrap();
        }
    }

    #[test]
    fn one_item_level_is_one_change_per_item() {
        let mut s = three_weapons();
        let was = level_at(&s, "54:1");
        s.stage(Edit::ItemLevel { item: at("54:1"), level: 30 }).unwrap();
        s.stage(Edit::ItemLevel { item: at("54:1"), level: 45 }).unwrap();
        s.stage(Edit::ItemLevel { item: at("54:2"), level: 12 }).unwrap();
        assert_eq!(s.pending().len(), 2, "the second level for 54:1 replaced the first");
        assert_eq!((level_at(&s, "54:1"), level_at(&s, "54:2")), (Some(45), Some(12)));
        assert_verifies(&s);
        let line = &s.change_lines()[0];
        assert_eq!((line.label.as_str(), line.item_ref.as_deref()), ("Item level", Some("54:1")));
        assert_eq!(line.to, "45");
        // Back to where it was: the change goes.
        s.stage(Edit::ItemLevel { item: at("54:1"), level: i64::from(was.unwrap()) }).unwrap();
        assert_eq!(s.pending().len(), 1);
        assert!(s.stage(Edit::ItemLevel { item: at("54:1"), level: 91 }).is_err());
        assert!(s.stage(Edit::ItemLevel { item: at("54:1"), level: 0 }).is_err());
        assert_eq!(s.pending().len(), 1, "refused edits are not staged");
    }

    #[test]
    fn moving_to_the_bank_and_back_again_leaves_nothing_pending() {
        let mut s = three_weapons();
        s.stage(Edit::MoveItem { item: at("54:0"), to_bank: true }).unwrap();
        let banked: Vec<ItemRow> = s.items().into_iter().filter(|i| i.location == ItemLocation::Bank).collect();
        assert_eq!(banked.len(), 1);
        assert_eq!(banked[0].entry_ref, at("41:0"));
        assert_eq!(s.snapshot().bank_used, 1);
        assert_verifies(&s);
        let line = &s.change_lines()[0];
        assert_eq!((line.text.as_str(), line.item_ref.as_deref()), ("Move: to the bank", Some("41:0")));
        // Moving it back undoes the move rather than adding a second one.
        let outcome = s.stage(Edit::MoveItem { item: at("41:0"), to_bank: false }).unwrap();
        assert!(!outcome.staged());
        assert!(s.pending().is_empty());
        assert_eq!(s.working(), s.original());
    }

    #[test]
    fn a_change_keeps_its_item_when_an_earlier_move_is_undone() {
        let mut s = three_weapons();
        let third = s.items().into_iter().find(|i| i.entry_ref == at("54:2")).unwrap();
        let second_level = level_at(&s, "54:1");
        assert_ne!(second_level, Some(33));
        let moved = s.stage(Edit::MoveItem { item: at("54:0"), to_bank: true }).unwrap().change_id.unwrap();
        // The third weapon is now 54:1. Level it, and copy it.
        s.stage(Edit::ItemLevel { item: at("54:1"), level: 33 }).unwrap();
        s.stage(Edit::Duplicate { source: at("54:1") }).unwrap();
        assert_verifies(&s);
        // Undo the move: the third weapon is back at 54:2, and the changes followed it.
        s.remove_change(moved).unwrap();
        assert_eq!(level_at(&s, "54:2"), Some(33));
        assert_eq!(level_at(&s, "54:1"), second_level, "the second weapon was not leveled");
        let copy = s.items().into_iter().find(|i| i.copy_of.is_some()).unwrap();
        assert_eq!(copy.copy_of, Some(at("54:2")));
        assert_eq!(copy.fields.as_ref().map(|f| f.parts), third.fields.as_ref().map(|f| f.parts));
        assert_verifies(&s);
    }

    #[test]
    fn moves_that_cannot_be_made_are_refused() {
        let mut s = three_weapons();
        let story = s.stage(Edit::MoveItem { item: echo_ref(), to_bank: true }).unwrap_err();
        assert!(story.message.contains("story or mission"), "{}", story.message);
        assert!(s.stage(Edit::MoveItem { item: at("54:0"), to_bank: false }).unwrap_err().message.contains("backpack already"));
        // The bank holds 6: the seventh is refused.
        s.stage(Edit::ImportCodes { codes: vec![WEAPON_CODE.to_string(); 6], to_bank: true }).unwrap();
        let full = s.stage(Edit::MoveItem { item: at("54:0"), to_bank: true }).unwrap_err();
        assert_eq!(full.kind, ErrorKind::BankFull);
        assert_eq!(s.pending().len(), 1);
    }

    #[test]
    fn ammo_levels_are_one_change_per_type() {
        let mut s = session();
        s.stage(Edit::AmmoLevel { ammo: 5, level: 3 }).unwrap();
        s.stage(Edit::AmmoLevel { ammo: 5, level: 4 }).unwrap();
        s.stage(Edit::AmmoLevel { ammo: 1, level: 7 }).unwrap();
        assert_eq!(s.pending().len(), 2);
        let levels = s.snapshot().ammo_levels;
        assert_eq!((levels[5], levels[1], levels[0]), (4, 7, 0));
        assert_eq!(s.change_lines()[0].text, "Sniper ammo upgrade 0 (48 max) → upgrade 4 (96 max)");
        assert_verifies(&s);
        assert!(s.stage(Edit::AmmoLevel { ammo: 5, level: 8 }).is_err());
        // Back to 0: the change goes.
        s.stage(Edit::AmmoLevel { ammo: 5, level: 0 }).unwrap();
        assert_eq!(s.pending().len(), 1);
    }
}

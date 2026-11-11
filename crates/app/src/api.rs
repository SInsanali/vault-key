//! The app's commands, as the UI calls them: `POST /api/<command>` with a JSON body.
//!
//! This layer has no window in it, so it is tested directly. It owns the application core
//! ([`gkcore::Core`]), the one character that is open for editing, and the item catalog,
//! which is loaded on another thread and handed over when it is ready.
//!
//! Every command answers with plain JSON. Errors are sentences for the person using the
//! app; the ones that come from `gkcore` are passed through unchanged.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gkcore::session::DUPLICATE_OF_IMPORT_MESSAGE;
use gkcore::app::EXTERNAL_MESSAGE;
use gkcore::{
    BackupInfo, Catalog, CharacterClass, CharacterEntry, CharacterSession, CharacterSnapshot, Core, CoreConfig,
    CoreError, Currency, Edit, EntryRef, ExportReport, GameStatus, ItemLocation, ItemRow, SaveReport, SessionSource,
    SlotId, StageOutcome, WritableBecause, WriteAccess, BACKPACK_FULL_MESSAGE, AMMO_LABELS, AMMO_NAMES,
    MAX_GEAR_LEVEL, ammo_capacity,
};
use serde_json::{json, Value};

/// What a command asks the window to do after answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
}

/// How long a game-running check is reused. Each check starts a process, so the page's
/// regular status polls must not each cause one. Saving always checks again by itself.
const GAME_STATUS_MAX_AGE: Duration = Duration::from_secs(5);

/// Name of the self-test's log file, inside Vault Key's data folder.
pub const SELFTEST_LOG_NAME: &str = "selftest.log";

/// Makes the core's configuration; called at start-up and again by `retry_start`.
pub type ConfigSource = Box<dyn Fn() -> Result<CoreConfig, CoreError>>;

/// The item catalog: names and rarity from the user's game install.
enum ItemData {
    /// Being read on another thread. `percent` is how far along it is, 0 to 100.
    Loading { result: Receiver<Result<Catalog, String>>, percent: Arc<AtomicU32> },
    Ready(Arc<Catalog>),
    /// Not available, with the reason for the user. The app works without it.
    Unavailable(String),
}

pub struct Api {
    config: ConfigSource,
    core: Result<Core, CoreError>,
    session: Option<CharacterSession>,
    item_data: ItemData,
    game: Option<(Instant, GameStatus)>,
    /// Things that happened by themselves and the user should be told once.
    notices: Vec<String>,
    /// `Some` only in self-test mode: the folder the self-test log is written in.
    selftest_dir: Option<PathBuf>,
}

type CommandResult = Result<Value, String>;

fn message(error: CoreError) -> String {
    error.message
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing \"{key}\"."))
}

fn slot_arg(args: &Value) -> Result<SlotId, String> {
    SlotId::parse(text_arg(args, "slotId")?).map_err(message)
}

fn whole_number(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key).and_then(Value::as_i64).ok_or_else(|| "Enter a whole number.".to_string())
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Every `BL2(...)` item code in a piece of pasted text, in order.
fn codes_in(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("BL2(") {
        let tail = &rest[start..];
        match tail.find(')') {
            Some(end) => {
                codes.push(tail[..=end].to_string());
                rest = &tail[end + 1..];
            }
            None => break,
        }
    }
    codes
}

/// The edit a `stage` request asks for.
fn parse_edit(args: &Value) -> Result<Edit, String> {
    let value = || whole_number(args, "value");
    Ok(match text_arg(args, "kind")? {
        "name" => Edit::Name(text_arg(args, "value")?.trim().to_string()),
        "level" => Edit::Level(value()?),
        "money" => Edit::Currency(Currency::Money, value()?),
        "eridium" => Edit::Currency(Currency::Eridium, value()?),
        "seraph" => Edit::Currency(Currency::Seraph, value()?),
        "torgue" => Edit::Currency(Currency::Torgue, value()?),
        "backpackSlots" => Edit::BackpackSlots(value()?),
        "bankSlots" => Edit::BankSlots(value()?),
        "gunSlots" => Edit::GunSlots(value()?),
        "opLevel" => Edit::OpLevel(value()?),
        "unlockTvhm" => Edit::UnlockTvhm,
        "unlockUvhm" => Edit::UnlockUvhm,
        "unlockSlaughterdome" => Edit::UnlockSlaughterdome,
        "unlockAmmo" => Edit::UnlockAmmo,
        "maximizeAmmo" => Edit::MaximizeAmmo,
        "itemLevels" => Edit::ItemLevels { level: whole_number(args, "level")?, force: flag(args, "force") },
        "itemLevel" => Edit::ItemLevel { item: item_arg(args)?, level: whole_number(args, "level")? },
        "moveItem" => Edit::MoveItem { item: item_arg(args)?, to_bank: flag(args, "toBank") },
        "ammoLevel" => {
            let name = text_arg(args, "ammo")?;
            let ammo = AMMO_NAMES.iter().position(|n| *n == name).ok_or_else(|| format!("Unknown ammo type: {name}"))?;
            Edit::AmmoLevel { ammo, level: whole_number(args, "level")? }
        }
        "importCodes" => {
            let codes = codes_in(text_arg(args, "text")?);
            if codes.is_empty() {
                return Err("No item codes found. A code looks like BL2(…).".to_string());
            }
            Edit::ImportCodes { codes, to_bank: flag(args, "toBank") }
        }
        other => return Err(format!("Unknown kind of change: {other}")),
    })
}

/// The name the UI uses for a kind of edit; the same names `stage` accepts.
fn edit_kind(edit: &Edit) -> &'static str {
    match edit {
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
        Edit::ImportCodes { .. } => "importCodes",
        Edit::Duplicate { .. } => "duplicate",
        Edit::ItemLevel { .. } => "itemLevel",
        Edit::MoveItem { .. } => "moveItem",
        Edit::AmmoLevel { .. } => "ammoLevel",
    }
}

/// The item a request names, as `field:index` in `item`.
fn item_arg(args: &Value) -> Result<EntryRef, String> {
    text_arg(args, "item")?.parse().map_err(|_| "That is not an item reference (like 54:0).".to_string())
}

fn access_json(access: &WriteAccess) -> Value {
    let because = match access {
        WriteAccess::Writable(WritableBecause::CreatedByApp) => Some("created"),
        WriteAccess::Writable(WritableBecause::TestSlot) => Some("test-slot"),
        WriteAccess::ReadOnly(_) => None,
    };
    json!({ "writable": access.is_writable(), "because": because, "explanation": access.explanation() })
}

fn snapshot_json(s: &CharacterSnapshot) -> Value {
    json!({
        "name": s.name,
        "classLabel": s.class_label,
        "heroName": s.class.map(CharacterClass::hero_name),
        "level": s.level,
        "experience": s.experience,
        "saveGameId": s.save_game_id,
        "money": s.money,
        "eridium": s.eridium,
        "seraph": s.seraph,
        "torgue": s.torgue,
        "backpackSlots": s.backpack_slots,
        "backpackUsed": s.backpack_used,
        "bankSlots": s.bank_slots,
        "bankUsed": s.bank_used,
        "gunSlots": s.gun_slots,
        "opLevel": s.op_level,
        "playthroughsUnlocked": s.playthroughs_unlocked,
        "slaughterdomeUnlocked": s.slaughterdome_unlocked,
        "ammo": s.ammo_levels.iter().enumerate().map(|(i, &level)| json!({
            "name": AMMO_NAMES[i],
            "label": AMMO_LABELS[i],
            "level": level,
            "capacity": ammo_capacity(i, level),
            "capacities": (0..=7).map(|l| ammo_capacity(i, l)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn entry_json(core: &Core, e: &CharacterEntry) -> Value {
    // Any readable character in a standard slot can be marked; the mark is for that character.
    let standard_slot = e.slot.is_some() && e.is_readable();
    json!({
        "slotId": e.slot_id.to_string(),
        "slot": e.slot,
        "steamId": e.steam_id,
        "fileName": e.file_name,
        "path": e.path,
        "modifiedUnix": e.modified_unix,
        "readable": e.is_readable(),
        "problem": e.problem,
        "name": e.name,
        "classLabel": e.class_label,
        "heroName": e.class.map(CharacterClass::hero_name),
        "level": e.level,
        "saveGameId": e.save_game_id,
        "access": access_json(&e.access),
        "isTestSlot": core.settings().is_test_slot(&e.slot_id),
        "createdByApp": core.settings().is_created_by_app(&e.slot_id),
        "canBeTestSlot": standard_slot && !e.protected,
        "protected": e.protected,
        "steamCloud": e.steam_cloud,
    })
}

fn backup_json(b: &BackupInfo) -> Value {
    json!({
        "id": b.id,
        "slotId": b.slot_id.to_string(),
        "fileName": b.file_name,
        "slot": b.slot,
        "createdUnix": b.created_unix,
        "createdUtc": b.created_utc,
        "reason": b.reason,
        "characterName": b.character_name,
        "level": b.level,
        "size": b.size,
    })
}

fn report_json(r: &SaveReport) -> Value {
    json!({
        "slotId": r.slot_id.to_string(),
        "slot": r.slot_id.slot(),
        "fileName": r.slot_id.file_name(),
        "path": r.path,
        "createdNew": r.created_new,
        "backup": r.backup.as_ref().map(backup_json),
        "warning": r.warning,
    })
}

fn export_json(r: &ExportReport) -> Value {
    json!({
        "path": r.path,
        "fileName": r.file_name,
        "name": r.name,
        "classLabel": r.class_label,
        "level": r.level,
        "saveGameId": r.save_game_id,
        "replacedBackup": r.replaced_backup,
    })
}

/// The id the page knows an open character by: the slot's id, or `file:` and the path for
/// a character opened from a file outside the save folder.
fn open_id(session: &CharacterSession) -> String {
    match session.source() {
        SessionSource::Slot(slot_id) => slot_id.to_string(),
        SessionSource::External(path) => format!("file:{}", path.display()),
    }
}

/// Free slots (2 to 9) in each Steam-ID folder of the save folder.
fn free_slots_by_steam_id(core: &Core) -> serde_json::Map<String, Value> {
    core.steam_ids()
        .unwrap_or_default()
        .into_iter()
        .map(|id| {
            let slots = core.free_slots(&id).unwrap_or_default();
            (id, json!(slots))
        })
        .collect()
}

fn location_name(location: ItemLocation) -> &'static str {
    match location {
        ItemLocation::Backpack => "backpack",
        ItemLocation::Equipped => "equipped",
        ItemLocation::Bank => "bank",
    }
}

/// Why Duplicate Item is not offered on an item, as a sentence for its disabled button.
fn not_duplicable_reason(row: &ItemRow, unverified: bool, category: &str, catalog_ready: bool) -> String {
    if row.is_new && row.copy_of.is_none() {
        return DUPLICATE_OF_IMPORT_MESSAGE.to_string();
    }
    if row.fields.is_none() || row.code.is_none() {
        return "Vault Key can't read this item's data, so it can't be duplicated.".to_string();
    }
    if unverified {
        return "This item is from add-on content that has not been checked in the game yet, so it can't be duplicated."
            .to_string();
    }
    match category {
        "mission" | "customization" | "usable" => {
            "Only weapons and gear (shields, grenade mods, class mods and relics) can be duplicated.".to_string()
        }
        _ if catalog_ready => "The game's item data does not say what this is, so it can't be duplicated.".to_string(),
        _ => "Only weapons and gear can be duplicated.".to_string(),
    }
}

fn item_json(row: &ItemRow, catalog: Option<&Catalog>) -> Value {
    let display = match (&row.fields, catalog) {
        (Some(fields), Some(catalog)) => catalog.describe(fields, row.is_weapon),
        _ => gkcore::ItemDisplay::UNKNOWN,
    };
    let blocked = (!row.can_duplicate)
        .then(|| not_duplicable_reason(row, display.unverified, display.category, catalog.is_some()));
    // The card's rows, computed from the game's data. `null` when there is no card.
    let stats = match (&row.fields, catalog) {
        (Some(fields), Some(catalog)) => catalog.card(fields, row.is_weapon),
        _ => None,
    }
    .map(|card| {
        json!({
            "rows": card.rows.iter().map(|r| json!({ "key": r.key, "label": r.label, "text": r.text, "note": r.note })).collect::<Vec<_>>(),
            "lines": card.lines.iter().map(|l| json!({ "kind": l.kind, "text": l.text, "color": l.color, "note": l.note })).collect::<Vec<_>>(),
            "missingLines": card.missing_lines,
        })
    });
    let raw = row.fields.as_ref().map(|f| {
        json!({ "set": f.set, "itemType": f.item_type, "balance": f.balance, "manufacturer": f.manufacturer })
    });
    json!({
        "ref": row.entry_ref.to_string(),
        "uid": row.uid,
        "location": location_name(row.location),
        "quickSlot": row.quick_slot,
        "isWeapon": row.is_weapon,
        "level": row.level,
        "code": row.code,
        "canDuplicate": row.can_duplicate,
        "duplicateBlocked": blocked,
        "isNew": row.is_new,
        "changeId": row.pending_change_id,
        "copyOf": row.copy_of.map(|source| source.to_string()),
        "name": display.name,
        "tier": display.tier,
        "tierLabel": display.tier_label,
        "color": display.color,
        "manufacturer": display.manufacturer,
        "category": display.category,
        "categoryLabel": display.category_label,
        "unverified": display.unverified,
        "definition": display.definition,
        "stats": stats,
        "raw": raw,
    })
}

/// Everything the UI shows for the open character.
fn character_json(core: &Core, session: &CharacterSession, catalog: Option<&Catalog>) -> Value {
    let slot_id = session.slot_id();
    let by_steam_id = free_slots_by_steam_id(core);
    // Where "Save to new slot" goes: the slot's own Steam-ID folder, or
    // for a file from elsewhere the only one there is (with several, the user picks).
    let home_steam_id = match slot_id {
        Some(slot_id) => Some(slot_id.steam_id().to_string()),
        None if by_steam_id.len() == 1 => by_steam_id.keys().next().cloned(),
        None => None,
    };
    let free_slots = home_steam_id.as_ref().and_then(|id| by_steam_id.get(id).cloned()).unwrap_or_else(|| json!([]));
    let source = match session.source() {
        SessionSource::Slot(_) => json!({ "kind": "slot" }),
        SessionSource::External(path) => json!({
            "kind": "external",
            "path": path.display().to_string(),
            "folder": path.parent().map(|p| p.display().to_string()),
            "fileName": session.file_name(),
        }),
    };
    let (access, is_test_slot) = match slot_id {
        Some(slot_id) => (access_json(&core.access(slot_id)), core.settings().is_test_slot(slot_id)),
        None => (json!({ "writable": false, "because": null, "explanation": EXTERNAL_MESSAGE }), false),
    };
    let current = session.snapshot();
    let items: Vec<Value> = session.items().iter().map(|row| item_json(row, catalog)).collect();
    let changes: Vec<Value> = session
        .change_lines()
        .iter()
        .zip(session.pending())
        .map(|(line, change)| {
            let item = line.item_ref.as_ref().and_then(|wanted| items.iter().find(|item| item["ref"] == **wanted));
            json!({
                "id": line.change_id,
                "kind": edit_kind(&change.edit),
                "label": line.label,
                "from": line.from,
                "to": line.to,
                "text": line.text,
                "itemRef": line.item_ref,
                "itemName": item.map(|item| item["name"].clone()),
                "itemTier": item.map(|item| item["tier"].clone()),
                "itemColor": item.map(|item| item["color"].clone()),
            })
        })
        .collect();
    let backpack_full = i64::try_from(current.backpack_used).unwrap_or(i64::MAX) >= current.backpack_slots;
    json!({
        "slotId": open_id(session),
        "slot": slot_id.and_then(SlotId::slot),
        "steamId": home_steam_id,
        "fileName": session.file_name(),
        "source": source,
        "access": access,
        "isTestSlot": is_test_slot,
        "current": snapshot_json(&current),
        "original": snapshot_json(&session.original_snapshot()),
        "items": items,
        "changes": changes,
        "pendingCount": session.pending_count(),
        "freeSlots": free_slots,
        "freeSlotsBySteamId": by_steam_id,
        "backpackFull": backpack_full,
        "backpackFullMessage": BACKPACK_FULL_MESSAGE,
        "limits": {
            "level": [savekit::MIN_LEVEL, savekit::MAX_LEVEL],
            "backpack": [savekit::MIN_BACKPACK_SLOTS, savekit::MAX_BACKPACK_SLOTS],
            "bank": [savekit::MIN_BANK_SLOTS, savekit::MAX_BANK_SLOTS],
            "opLevel": [0, savekit::MAX_OP_LEVEL],
            "currency": [0, savekit::MAX_CURRENCY],
            "itemLevel": [1, MAX_GEAR_LEVEL],
        },
    })
}

impl Api {
    /// The app as it really runs: folders from the environment, the live process list, and
    /// the item catalog read from the game install on another thread.
    ///
    /// `selftest_dir` is `Some` only in self-test mode (see `main`).
    pub fn live(catalog_cache: PathBuf, selftest_dir: Option<PathBuf>) -> Api {
        let config: ConfigSource = if selftest_dir.is_some() {
            Box::new(sandbox_config)
        } else {
            Box::new(CoreConfig::live)
        };
        let mut api = Api::with_config(config);
        api.selftest_dir = selftest_dir;
        let (sender, result) = mpsc::channel();
        let percent = Arc::new(AtomicU32::new(0));
        let shown = percent.clone();
        std::thread::spawn(move || {
            let loaded = Catalog::load(Some(catalog_cache), &mut |progress| {
                // The fraction is 0 to 1, so the cast cannot overflow or go negative.
                let done = (progress.fraction.clamp(0.0, 1.0) * 100.0) as u32;
                shown.store(done, Ordering::Relaxed);
            });
            // The app may have closed meanwhile; then nobody is listening, which is fine.
            let _ = sender.send(loaded);
        });
        api.expect_item_data(result, percent);
        api
    }

    /// The catalog is being loaded elsewhere and will arrive on `result`.
    pub fn expect_item_data(&mut self, result: Receiver<Result<Catalog, String>>, percent: Arc<AtomicU32>) {
        self.item_data = ItemData::Loading { result, percent };
    }

    /// An `Api` on a given configuration, with no item catalog. Tests use this.
    pub fn with_config(config: ConfigSource) -> Api {
        let core = config().and_then(Core::new);
        Api {
            config,
            core,
            session: None,
            item_data: ItemData::Unavailable("Item data has not been loaded.".to_string()),
            game: None,
            notices: Vec::new(),
            selftest_dir: None,
        }
    }

    /// Run one command. `Err` is a message for the user.
    pub fn handle(&mut self, command: &str, args: &Value) -> (CommandResult, Effect) {
        self.take_item_data();
        if command == "quit" {
            return (Ok(Value::Null), Effect::Quit);
        }
        (self.run(command, args), Effect::None)
    }

    /// If the catalog has finished loading, starts using it: new sessions get it as their
    /// classifier, and so does the open one.
    fn take_item_data(&mut self) {
        let ItemData::Loading { result, .. } = &self.item_data else { return };
        let loaded = match result.try_recv() {
            Ok(loaded) => loaded,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Reading the game's item data stopped unexpectedly.".to_string()),
        };
        match loaded {
            Ok(catalog) => self.use_catalog(Arc::new(catalog)),
            Err(why) => self.item_data = ItemData::Unavailable(why),
        }
    }

    fn use_catalog(&mut self, catalog: Arc<Catalog>) {
        if let Ok(core) = &mut self.core {
            core.set_classifier(catalog.clone());
        }
        if let Some(session) = &mut self.session {
            // A copy staged under the stand-in rule may be of something the game data says
            // is not gear. Such a change is dropped, and the user is told.
            let mut dropped = 0;
            while let Err(error) = session.set_classifier(catalog.clone()) {
                match error.change_id {
                    Some(id) if session.remove_change(id).is_ok() => dropped += 1,
                    _ => {
                        dropped += session.pending_count();
                        session.discard_all();
                    }
                }
            }
            if dropped > 0 {
                let what = if dropped == 1 {
                    "1 pending change was removed because the data does not allow it".to_string()
                } else {
                    format!("{dropped} pending changes were removed because the data does not allow them")
                };
                self.notices.push(format!("The game's item data finished loading, and {what}."));
            }
        }
        self.item_data = ItemData::Ready(catalog);
    }

    fn catalog(&self) -> Option<&Catalog> {
        match &self.item_data {
            ItemData::Ready(catalog) => Some(catalog),
            _ => None,
        }
    }

    fn core(&mut self) -> Result<&mut Core, String> {
        self.core.as_mut().map_err(|e| e.message.clone())
    }

    /// The core and the open character together.
    fn open_parts(&mut self) -> Result<(&mut Core, &mut CharacterSession), String> {
        let core = self.core.as_mut().map_err(|e| e.message.clone())?;
        let session = self.session.as_mut().ok_or("No character is open.")?;
        Ok((core, session))
    }

    fn character(&self) -> CommandResult {
        let core = self.core.as_ref().map_err(|e| e.message.clone())?;
        let session = self.session.as_ref().ok_or("No character is open.")?;
        Ok(character_json(core, session, self.catalog()))
    }

    fn character_or_null(&self) -> Value {
        self.character().unwrap_or(Value::Null)
    }

    fn game_json(&mut self, fresh: bool) -> Value {
        let Ok(core) = &self.core else { return Value::Null };
        let known = self.game.as_ref().filter(|(at, _)| !fresh && at.elapsed() < GAME_STATUS_MAX_AGE);
        let status = match known.map(|(_, status)| status.clone()) {
            Some(status) => status,
            None => {
                let status = core.game_status();
                self.game = Some((Instant::now(), status.clone()));
                status
            }
        };
        match status {
            GameStatus::NotRunning => json!({ "state": "not-running", "message": "Game closed. Safe to save." }),
            GameStatus::Running => {
                json!({ "state": "running", "message": "Borderlands 2 is running. Quit the game before you save." })
            }
            GameStatus::Unknown(why) => json!({
                "state": "unknown",
                "message": format!("Can't tell whether Borderlands 2 is running ({why}). Saving is blocked until that is known."),
            }),
        }
    }

    fn item_data_json(&self) -> Value {
        match &self.item_data {
            ItemData::Loading { percent, .. } => json!({
                "state": "loading",
                "percent": percent.load(Ordering::Relaxed),
                "message": "Reading item names from your game…",
            }),
            ItemData::Ready(_) => json!({ "state": "ready", "message": "Item names come from your game's own files." }),
            ItemData::Unavailable(why) => json!({ "state": "unavailable", "message": why }),
        }
    }

    fn status(&mut self, args: &Value) -> CommandResult {
        let game = self.game_json(flag(args, "fresh"));
        let folder = match &self.core {
            Ok(core) => json!({
                "found": true,
                "path": core.save_data_root().display().to_string(),
                "dataDir": core.data_dir().display().to_string(),
                "problem": null,
            }),
            Err(error) => json!({
                "found": false,
                "path": null,
                "dataDir": null,
                "problem": { "kind": error.kind.code(), "message": error.message },
            }),
        };
        Ok(json!({
            "app": "Vault Key",
            "version": env!("CARGO_PKG_VERSION"),
            "logFile": crate::errlog::path().map(|path| path.display().to_string()),
            "saveFolder": folder,
            "game": game,
            "itemData": self.item_data_json(),
            "open": self.session.as_ref().map(open_id),
            "pendingCount": self.session.as_ref().map_or(0, CharacterSession::pending_count),
            "notices": std::mem::take(&mut self.notices),
            "selftest": self.selftest_dir.is_some(),
            "theme": self.core.as_ref().map_or("yellow", |core| core.settings().theme()),
        }))
    }

    fn list_characters(&mut self) -> CommandResult {
        let core = self.core()?;
        let entries = core.list_characters().map_err(message)?;
        let characters: Vec<Value> = entries.iter().map(|e| entry_json(core, e)).collect();
        let free = free_slots_by_steam_id(core);
        Ok(json!({ "characters": characters, "freeSlots": free }))
    }

    fn stage(&mut self, args: &Value) -> CommandResult {
        let edit = parse_edit(args)?;
        let (_, session) = self.open_parts()?;
        let is_level = matches!(edit, Edit::Level(_));
        let outcome = session.stage(edit).map_err(message)?;
        if is_level {
            keep_item_levels_after_level(session);
        }
        Ok(self.staged_json(&outcome))
    }

    fn staged_json(&self, outcome: &StageOutcome) -> Value {
        json!({
            "staged": outcome.staged(),
            "changeId": outcome.change_id,
            "newItems": outcome.new_entries.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "character": self.character_or_null(),
        })
    }

    fn save(&mut self) -> CommandResult {
        self.game = None;
        let (core, session) = self.open_parts()?;
        let report = core.save_in_place(session).map_err(message)?;
        // Show what is on disk now, not what was meant to be written.
        let written = report.slot_id.clone();
        let mut report = report_json(&report);
        match core.open(&written) {
            Ok(reopened) => *session = reopened,
            Err(error) => {
                report["warning"] = json!(format!(
                    "The save was written and verified, but could not be opened again afterwards: {}",
                    error.message
                ));
            }
        }
        Ok(json!({ "report": report, "character": self.character_or_null() }))
    }

    fn save_as_new(&mut self, args: &Value) -> CommandResult {
        self.game = None;
        let slot = u32::try_from(whole_number(args, "slot")?).map_err(|_| "Choose a free slot.".to_string())?;
        let steam_id = args.get("steamId").and_then(Value::as_str).filter(|id| !id.is_empty());
        let (core, session) = self.open_parts()?;
        let report = match steam_id {
            Some(id) => core.save_as_new_in(session, id, slot),
            None => core.save_as_new(session, slot),
        }
        .map_err(message)?;
        // Carry on in the new character: it holds the changes, and it can be saved in place.
        let mut json = report_json(&report);
        match core.open(&report.slot_id) {
            Ok(opened) => *session = opened,
            Err(error) => {
                json["warning"] = json!(format!(
                    "The new save was written and verified, but could not be opened afterwards: {}",
                    error.message
                ));
            }
        }
        Ok(json!({ "report": json, "character": self.character_or_null() }))
    }

    fn restore_backup(&mut self, args: &Value) -> CommandResult {
        self.game = None;
        let id = text_arg(args, "backupId")?;
        let core = self.core.as_mut().map_err(|e| e.message.clone())?;
        let report = core.restore_backup(id).map_err(message)?;
        let mut json = report_json(&report);
        // The open character is out of date if it was the one restored.
        if self.session.as_ref().is_some_and(|s| s.slot_id() == Some(&report.slot_id)) {
            match core.open(&report.slot_id) {
                Ok(reopened) => self.session = Some(reopened),
                Err(error) => {
                    self.session = None;
                    json["warning"] =
                        json!(format!("The backup was restored, but the character could not be opened: {}", error.message));
                }
            }
        }
        Ok(json!({ "report": json, "character": self.character_or_null() }))
    }

    fn list_backups(&mut self, args: &Value) -> CommandResult {
        let core = self.core.as_ref().map_err(|e| e.message.clone())?;
        let backups = if flag(args, "all") {
            core.list_backups()
        } else {
            let slot = match args.get("slotId") {
                Some(Value::String(text)) => SlotId::parse(text).map_err(message)?,
                _ => match self.session.as_ref().ok_or("No character is open.")?.slot_id() {
                    Some(slot) => slot.clone(),
                    // A character from a file outside the save folder has no backups here.
                    None => return Ok(json!({ "backups": [] })),
                },
            };
            core.list_backups_for(&slot)
        };
        Ok(json!({ "backups": backups.iter().map(backup_json).collect::<Vec<_>>() }))
    }

    fn set_test_slot(&mut self, args: &Value, marked: bool) -> CommandResult {
        let slot = slot_arg(args)?;
        let core = self.core()?;
        if marked {
            core.mark_test_slot(&slot).map_err(message)?;
        } else {
            core.unmark_test_slot(&slot).map_err(message)?;
        }
        let mut answer = self.list_characters()?;
        answer["character"] = self.character_or_null();
        Ok(answer)
    }

    /// The Lock / Unlock button: locked, Vault Key never writes the character; unlocked, it
    /// may be edited in place.
    fn set_locked(&mut self, args: &Value, locked: bool) -> CommandResult {
        let slot = slot_arg(args)?;
        let core = self.core()?;
        if locked {
            core.lock(&slot).map_err(message)?;
        } else {
            core.unlock(&slot).map_err(message)?;
        }
        let mut answer = self.list_characters()?;
        answer["character"] = self.character_or_null();
        Ok(answer)
    }

    fn selftest_log(&self, args: &Value) -> CommandResult {
        let Some(dir) = &self.selftest_dir else {
            return Err("The self-test is not enabled.".to_string());
        };
        let line: String = text_arg(args, "line")?.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let path = dir.join(SELFTEST_LOG_NAME);
        let written = std::fs::create_dir_all(dir).and_then(|()| {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
            writeln!(file, "{line}")
        });
        written.map(|()| Value::Null).map_err(|e| format!("Cannot write {}: {e}", path.display()))
    }

    fn run(&mut self, command: &str, args: &Value) -> CommandResult {
        match command {
            "ping" => Ok(json!({ "app": "Vault Key", "version": env!("CARGO_PKG_VERSION") })),
            "status" => self.status(args),
            "retry_start" => {
                if self.core.is_err() {
                    self.core = (self.config)().and_then(Core::new);
                    if let (Ok(core), ItemData::Ready(catalog)) = (&mut self.core, &self.item_data) {
                        core.set_classifier(catalog.clone());
                    }
                }
                self.status(args)
            }
            "list_characters" => self.list_characters(),
            "open_character" => {
                let slot = slot_arg(args)?;
                let session = self.core()?.open(&slot).map_err(message)?;
                self.session = Some(session);
                self.character()
            }
            "open_file" => {
                let path = PathBuf::from(text_arg(args, "path")?);
                let session = self.core()?.open_file(&path).map_err(message)?;
                self.session = Some(session);
                self.character()
            }
            "export_file" => {
                let path = PathBuf::from(text_arg(args, "path")?);
                let (core, session) = self.open_parts()?;
                let report = core.export(session, &path).map_err(message)?;
                Ok(json!({ "report": export_json(&report) }))
            }
            "character" => self.character(),
            "close_character" => {
                self.session = None;
                Ok(Value::Null)
            }
            "stage" => self.stage(args),
            "duplicate_item" => {
                let source: EntryRef =
                    text_arg(args, "item")?.parse().map_err(|_| "That item is no longer in the list.".to_string())?;
                let (_, session) = self.open_parts()?;
                let outcome = session.duplicate_item(source).map_err(message)?;
                Ok(self.staged_json(&outcome))
            }
            "remove_change" => {
                let id = args.get("changeId").and_then(Value::as_u64).ok_or("Missing \"changeId\".")?;
                let (_, session) = self.open_parts()?;
                session.remove_change(id).map_err(message)?;
                self.character()
            }
            "discard_changes" => {
                let (_, session) = self.open_parts()?;
                session.discard_all();
                self.character()
            }
            "save" => self.save(),
            "save_as_new" => self.save_as_new(args),
            "list_backups" => self.list_backups(args),
            "restore_backup" => self.restore_backup(args),
            "mark_test_slot" => self.set_test_slot(args, true),
            "unmark_test_slot" => self.set_test_slot(args, false),
            "set_theme" => {
                let theme = text_arg(args, "theme")?;
                self.core()?.set_theme(theme).map_err(message)?;
                Ok(json!({ "theme": theme }))
            }
            "lock_character" => self.set_locked(args, true),
            "unlock_character" => self.set_locked(args, false),
            "export_codes" => {
                let (_, session) = self.open_parts()?;
                let codes = session.export_codes(flag(args, "includeBank"));
                Ok(json!({ "text": codes.join("\n"), "codes": codes }))
            }
            "selftest_log" => self.selftest_log(args),
            other => Err(format!("unknown command: {other}")),
        }
    }
}

/// The configuration for a self-test run: the sandbox folders from the environment, and no
/// knowledge of the real save folders.
///
/// Normally the core keeps the real folders in view even when it is pointed at another one:
/// before creating a character it reads their `Save0001.sav` to make sure it is not about to
/// install an exact copy of the main character. A self-test must not read the real save
/// folder at all, so here the core is not told where it is. `main` has already checked that
/// neither sandbox folder is inside a real save folder.
fn sandbox_config() -> Result<CoreConfig, CoreError> {
    let mut config = CoreConfig::live()?;
    config.other_save_roots.clear();
    Ok(config)
}

/// "Item levels: the character's level" uses the level as it is when that change is applied,
/// and changes are applied in the order they were staged. So when a level change is staged
/// after it, the item-level change is staged again, which puts it after the level change.
fn keep_item_levels_after_level(session: &mut CharacterSession) {
    let pending = session.pending();
    let level_at = pending.iter().position(|c| matches!(c.edit, Edit::Level(_)));
    let items = pending.iter().enumerate().find_map(|(at, c)| match c.edit {
        Edit::ItemLevels { level: 0, force } => Some((at, c.id, force)),
        _ => None,
    });
    if let (Some(level_at), Some((items_at, id, force))) = (level_at, items) {
        if items_at < level_at && session.remove_change(id).is_ok() {
            // If this fails the item-level change is gone, and the pending list shows that.
            let _ = session.stage(Edit::ItemLevels { level: 0, force });
        }
    }
}

#[cfg(test)]
mod tests;

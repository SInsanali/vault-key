//! Tests of the command layer. Every one runs on fresh folders under the system temp folder,
//! with a made-up process list; none touches a real save folder or Vault Key's real data.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use gkcore::guards::GkError;
use gkcore::{Catalog, CoreConfig, CoreError, ErrorKind, FallbackGearClassifier, SaveDocument};
use savekit::{field, ProtoValue};
use serde_json::{json, Value};

use super::*;

const STEAM_ID: &str = "76561190000000000";
/// A weapon with a real manufacturer (gear), from the save format's test vectors.
const GEAR_WEAPON_CODE: &str = "BL2(hwAAAAC8CQCBxwJABBEI0MJghdENIwHG//////////97w/iGMQzj)";
/// A second, different weapon.
const WEAPON_CODE: &str = "BL2(h0Hd1Z+jY/s2Qy++Zu8Ba9qXoOmjwJ6NhrlsOmhNMX+oJo5CfQns)";
/// A level-1 gear item.
const ITEM_CODE: &str = "BL2(B2vuv4tz1zSQCf2pqLJCS5XD/tKN4FXpjRJLnn1v85U=)";
/// An intro ECHO: a story item with no manufacturer, so not gear.
const ECHO_CODE: &str = "BL2(BwAAAABsWgADYgaQ4/8AwA==)";
const CLASS_PATH: &str = "GD_Tulip_Mechromancer.Character.CharClass_Mechromancer";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, empty temp folder that is removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("gk-app-test-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A small but complete character: Gaige, level 5, backpack 12 with one gear weapon
/// (`54:0`), one gear item (`53:0`) and one story item (`53:1`).
fn sample_bytes(save_game_id: i64) -> Vec<u8> {
    let mut doc = SaveDocument::default();
    doc.message.set_single(field::CLASS, ProtoValue::Bytes(CLASS_PATH.as_bytes().to_vec()));
    doc.set_level(5).unwrap();
    doc.set_name("Gaige").unwrap();
    doc.set_save_game_id(save_game_id).unwrap();
    doc.set_backpack_slots(12).unwrap();
    doc.set_bank_slots(6).unwrap();
    doc.set_gun_slots(2).unwrap();
    doc.set_currency(Currency::Money, 80).unwrap();
    let mut key = 1000;
    let mut keys = move || {
        key += 1;
        key
    };
    doc.import_codes_with(&[GEAR_WEAPON_CODE, ITEM_CODE, ECHO_CODE], false, &mut keys).unwrap();
    doc.to_file_bytes().unwrap()
}

/// An `Api` on temp folders: `<temp>/SaveData/<steam id>` and `<temp>/VaultKey`.
struct World {
    temp: TempDir,
    dir: PathBuf,
    /// Whether the made-up process list contains the game.
    running: Arc<AtomicBool>,
    /// Whether the made-up process list fails.
    unreadable: Arc<AtomicBool>,
    /// How many times the process list was asked for.
    listed: Arc<AtomicU32>,
    api: Api,
}

impl World {
    fn new() -> World {
        let temp = TempDir::new();
        let dir = temp.path().join("SaveData").join(STEAM_ID);
        fs::create_dir_all(&dir).unwrap();
        let running = Arc::new(AtomicBool::new(false));
        let unreadable = Arc::new(AtomicBool::new(false));
        let listed = Arc::new(AtomicU32::new(0));
        let api = Api::with_config(Self::config(temp.path(), &running, &unreadable, &listed));
        World { temp, dir, running, unreadable, listed, api }
    }

    fn config(root: &Path, running: &Arc<AtomicBool>, unreadable: &Arc<AtomicBool>, listed: &Arc<AtomicU32>) -> ConfigSource {
        let root = root.to_path_buf();
        let (running, unreadable, listed) = (running.clone(), unreadable.clone(), listed.clone());
        Box::new(move || {
            let (running, unreadable, listed) = (running.clone(), unreadable.clone(), listed.clone());
            Ok(CoreConfig {
                save_data_root: root.join("SaveData"),
                data_dir: root.join("VaultKey"),
                other_save_roots: Vec::new(),
                processes: Arc::new(move || {
                    listed.fetch_add(1, Ordering::SeqCst);
                    if unreadable.load(Ordering::SeqCst) {
                        return Err(GkError("refused: cannot list running processes".to_string()));
                    }
                    let mut names = vec!["explorer.exe".to_string(), "steam.exe".to_string()];
                    if running.load(Ordering::SeqCst) {
                        names.push("Borderlands2.exe".to_string());
                    }
                    Ok(names)
                }),
                clock: gkcore::system_clock(),
                classifier: Arc::new(FallbackGearClassifier),
            })
        })
    }

    fn put(&self, file: &str, save_game_id: i64) -> Vec<u8> {
        let bytes = sample_bytes(save_game_id);
        fs::write(self.dir.join(file), &bytes).unwrap();
        bytes
    }

    fn read(&self, file: &str) -> Vec<u8> {
        fs::read(self.dir.join(file)).unwrap()
    }

    fn slot(file: &str) -> String {
        format!("{STEAM_ID}/{file}")
    }

    fn ok(&mut self, command: &str, args: Value) -> Value {
        let (result, effect) = self.api.handle(command, &args);
        assert_eq!(effect, Effect::None);
        result.unwrap_or_else(|e| panic!("{command} failed: {e}"))
    }

    fn err(&mut self, command: &str, args: Value) -> String {
        self.api.handle(command, &args).0.expect_err(command)
    }

    fn open(&mut self, file: &str) -> Value {
        self.ok("open_character", json!({ "slotId": Self::slot(file) }))
    }

    fn stage(&mut self, edit: Value) -> Value {
        self.ok("stage", edit)
    }
}

fn kinds(character: &Value) -> Vec<&str> {
    character["changes"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect()
}

fn item<'a>(character: &'a Value, item_ref: &str) -> &'a Value {
    character["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["ref"] == item_ref)
        .unwrap_or_else(|| panic!("no item {item_ref}"))
}

#[test]
fn ping_quit_and_unknown() {
    let mut world = World::new();
    assert_eq!(world.ok("ping", Value::Null)["app"], "Vault Key");
    assert!(world.err("nope", Value::Null).contains("unknown command"));
    let (result, effect) = world.api.handle("quit", &Value::Null);
    assert_eq!((result, effect), (Ok(Value::Null), Effect::Quit));
}

#[test]
fn finds_item_codes_in_pasted_text() {
    assert!(codes_in("").is_empty());
    assert!(codes_in("nothing here, BL2 (no").is_empty());
    let text = format!("my guns:\n{GEAR_WEAPON_CODE}, and {ITEM_CODE}\r\nBL2(unfinished");
    assert_eq!(codes_in(&text), [GEAR_WEAPON_CODE, ITEM_CODE]);
}

#[test]
fn status_reports_folder_game_and_item_data() {
    let mut world = World::new();
    let status = world.ok("status", Value::Null);
    assert_eq!(status["saveFolder"]["found"], true);
    assert!(status["saveFolder"]["path"].as_str().unwrap().ends_with("SaveData"));
    assert_eq!(status["game"]["state"], "not-running");
    assert_eq!(status["itemData"]["state"], "unavailable");
    assert_eq!((&status["open"], &status["pendingCount"], &status["selftest"]), (&Value::Null, &json!(0), &json!(false)));

    // The process list is asked for once, then reused for a while: polling is cheap.
    for _ in 0..5 {
        world.ok("status", Value::Null);
    }
    assert_eq!(world.listed.load(Ordering::SeqCst), 1);

    // "fresh" asks again.
    world.running.store(true, Ordering::SeqCst);
    assert_eq!(world.ok("status", Value::Null)["game"]["state"], "not-running");
    let status = world.ok("status", json!({ "fresh": true }));
    assert_eq!(status["game"]["state"], "running");
    assert!(status["game"]["message"].as_str().unwrap().contains("Borderlands 2 is running"));
    assert_eq!(world.listed.load(Ordering::SeqCst), 2);

    // A process list that cannot be read is "unknown", never "not running".
    world.unreadable.store(true, Ordering::SeqCst);
    let status = world.ok("status", json!({ "fresh": true }));
    assert_eq!(status["game"]["state"], "unknown");
    assert!(status["game"]["message"].as_str().unwrap().contains("Saving is blocked"));
}

#[test]
fn a_missing_save_folder_is_a_state_not_a_crash() {
    let temp = TempDir::new();
    let found = Arc::new(AtomicBool::new(false));
    let root = temp.path().to_path_buf();
    let seen = found.clone();
    let mut api = Api::with_config(Box::new(move || {
        if !seen.load(Ordering::SeqCst) {
            return Err(CoreError::new(ErrorKind::SaveFolder, "Cannot find the game's SaveData folder."));
        }
        let mut config = (World::config(&root, &Arc::default(), &Arc::default(), &Arc::default()))()?;
        config.save_data_root = root.join("SaveData");
        Ok(config)
    }));
    let status = api.handle("status", &Value::Null).0.unwrap();
    assert_eq!(status["saveFolder"]["found"], false);
    assert_eq!(status["saveFolder"]["problem"]["kind"], "save-folder");
    assert_eq!(status["saveFolder"]["problem"]["message"], "Cannot find the game's SaveData folder.");
    assert_eq!(status["game"], Value::Null);
    // Every command that needs the folder says the same thing.
    for command in ["list_characters", "character", "save", "list_backups"] {
        assert_eq!(api.handle(command, &json!({})).0.unwrap_err(), "Cannot find the game's SaveData folder.", "{command}");
    }

    // The folder turns up (the game was started for the first time): "Try again" finds it.
    fs::create_dir_all(temp.path().join("SaveData").join(STEAM_ID)).unwrap();
    assert_eq!(api.handle("retry_start", &Value::Null).0.unwrap()["saveFolder"]["found"], false);
    found.store(true, Ordering::SeqCst);
    let status = api.handle("retry_start", &Value::Null).0.unwrap();
    assert_eq!(status["saveFolder"]["found"], true);
    assert_eq!(api.handle("list_characters", &Value::Null).0.unwrap()["characters"], json!([]));
}

#[test]
fn lists_characters_with_access_and_unreadable_files() {
    let mut world = World::new();
    assert_eq!(world.ok("list_characters", Value::Null)["characters"], json!([]));

    world.put("Save0001.sav", 1);
    world.put("Save0003.sav", 3);
    fs::write(world.dir.join("Save0005.sav"), b"this is not a save").unwrap();
    let list = world.ok("list_characters", Value::Null);
    let characters = list["characters"].as_array().unwrap();
    assert_eq!(characters.len(), 3);

    let main = &characters[0];
    assert_eq!((&main["slot"], &main["name"], &main["classLabel"], &main["heroName"]), (&json!(1), &json!("Gaige"), &json!("Mechromancer"), &json!("Gaige")));
    assert_eq!((&main["level"], &main["readable"], &main["canBeTestSlot"]), (&json!(5), &json!(true), &json!(true)));
    assert_eq!(main["access"]["writable"], false);
    assert!(main["access"]["explanation"].as_str().unwrap().contains("read-only"));

    let third = &characters[1];
    assert_eq!((&third["slotId"], &third["canBeTestSlot"]), (&json!(World::slot("Save0003.sav")), &json!(true)));
    assert_eq!((&third["access"]["writable"], &third["isTestSlot"], &third["createdByApp"]), (&json!(false), &json!(false), &json!(false)));
    assert!(third["access"]["explanation"].as_str().unwrap().contains("read-only"));

    let junk = &characters[2];
    assert_eq!((&junk["slot"], &junk["readable"], &junk["name"]), (&json!(5), &json!(false), &json!("")));
    assert!(!junk["problem"].as_str().unwrap().is_empty());
    assert!(world.err("open_character", json!({ "slotId": World::slot("Save0005.sav") })).contains("can't be opened"));

    // Slots 1, 3 and 5 hold files; the rest of 2 to 9 are free.
    assert_eq!(list["freeSlots"][STEAM_ID], json!([2, 4, 6, 7, 8, 9]));

    // Nonsense slot ids are refused, not turned into paths.
    assert!(world.err("open_character", json!({ "slotId": "Save0001.sav" })).contains("not a slot id"));
    assert!(world.err("open_character", json!({ "slotId": "../Save0001.sav" })).contains("not a Steam-ID folder name"));
    assert!(world.err("open_character", json!({ "slotId": format!("{STEAM_ID}/..\\x.sav") })).contains("not a save file name"));
    assert!(world.err("open_character", json!({})).contains("slotId"));
}

#[test]
fn opens_a_character_with_everything_the_screens_show() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    assert_eq!(world.err("character", Value::Null), "No character is open.");
    let c = world.open("Save0003.sav");
    assert_eq!((&c["slot"], &c["fileName"], &c["pendingCount"]), (&json!(3), &json!("Save0003.sav"), &json!(0)));
    assert_eq!(c["access"]["writable"], false);
    assert_eq!(c["current"], c["original"]);
    let now = &c["current"];
    assert_eq!((&now["name"], &now["level"], &now["money"], &now["saveGameId"]), (&json!("Gaige"), &json!(5), &json!(80), &json!(3)));
    assert_eq!((&now["backpackSlots"], &now["backpackUsed"], &now["bankSlots"], &now["gunSlots"]), (&json!(12), &json!(3), &json!(6), &json!(2)));
    assert_eq!((&now["opLevel"], &now["playthroughsUnlocked"], &now["slaughterdomeUnlocked"]), (&json!(0), &json!(0), &json!(false)));
    assert_eq!(c["freeSlots"], json!([2, 4, 5, 6, 7, 8, 9]));
    assert_eq!((&c["backpackFull"], &c["backpackFullMessage"]), (&json!(false), &json!(BACKPACK_FULL_MESSAGE)));
    assert_eq!((&c["limits"]["level"], &c["limits"]["backpack"], &c["limits"]["bank"]), (&json!([1, 80]), &json!([12, 39]), &json!([6, 24])));

    // Without game data, items have no name and no rarity; nothing is guessed.
    let items = c["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let weapon = item(&c, "54:0");
    assert_eq!((&weapon["isWeapon"], &weapon["location"], &weapon["code"]), (&json!(true), &json!("backpack"), &json!(GEAR_WEAPON_CODE)));
    assert_eq!((&weapon["name"], &weapon["tier"], &weapon["color"], &weapon["unverified"]), (&Value::Null, &json!("unknown"), &Value::Null, &json!(false)));
    assert_eq!((&weapon["canDuplicate"], &weapon["duplicateBlocked"], &weapon["isNew"]), (&json!(true), &Value::Null, &json!(false)));
    assert!(weapon["raw"]["balance"].is_number() && weapon["level"].is_number());
    // The story item is not gear, and the button says why.
    let echo = item(&c, "53:1");
    assert_eq!(echo["canDuplicate"], false);
    assert_eq!(echo["duplicateBlocked"], "Only weapons and gear can be duplicated.");

    assert_eq!(world.ok("status", Value::Null)["open"], World::slot("Save0003.sav"));
    world.ok("close_character", Value::Null);
    assert_eq!(world.err("character", Value::Null), "No character is open.");
    assert_eq!(world.err("stage", json!({ "kind": "level", "value": 9 })), "No character is open.");
}

#[test]
fn stages_every_kind_of_edit() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.open("Save0003.sav");
    let edits = [
        json!({ "kind": "name", "value": "  Kai " }),
        json!({ "kind": "level", "value": 30 }),
        json!({ "kind": "money", "value": 123_456 }),
        json!({ "kind": "eridium", "value": 99 }),
        json!({ "kind": "seraph", "value": 12 }),
        json!({ "kind": "torgue", "value": 34 }),
        json!({ "kind": "backpackSlots", "value": 39 }),
        json!({ "kind": "bankSlots", "value": 24 }),
        json!({ "kind": "gunSlots", "value": 4 }),
        json!({ "kind": "unlockTvhm" }),
        json!({ "kind": "unlockUvhm" }),
        json!({ "kind": "unlockSlaughterdome" }),
        json!({ "kind": "opLevel", "value": 2 }),
        json!({ "kind": "unlockAmmo" }),
        json!({ "kind": "itemLevels", "level": 20, "force": true }),
    ];
    let mut last = Value::Null;
    for (n, edit) in edits.iter().enumerate() {
        let answer = world.stage(edit.clone());
        assert_eq!(answer["staged"], true, "{edit}");
        assert!(answer["changeId"].is_u64(), "{edit}");
        assert_eq!(answer["character"]["pendingCount"], n + 1, "{edit}");
        last = answer["character"].clone();
    }
    let wanted: Vec<&str> = edits.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds(&last), wanted);
    let now = &last["current"];
    assert_eq!((&now["name"], &now["level"], &now["money"], &now["eridium"]), (&json!("Kai"), &json!(30), &json!(123_456), &json!(99)));
    assert_eq!((&now["seraph"], &now["torgue"], &now["backpackSlots"], &now["bankSlots"]), (&json!(12), &json!(34), &json!(39), &json!(24)));
    assert_eq!((&now["gunSlots"], &now["opLevel"], &now["playthroughsUnlocked"], &now["slaughterdomeUnlocked"]), (&json!(4), &json!(2), &json!(2), &json!(true)));
    assert_eq!(last["original"]["level"], 5, "the original is still what is on disk");
    assert_eq!(item(&last, "54:0")["level"], 20);
    let lines: Vec<&str> = last["changes"].as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap()).collect();
    assert_eq!(&lines[..3], ["Name Gaige → Kai", "Level 5 → 30", "Money $80 → $123,456"]);
    assert_eq!(world.ok("status", Value::Null)["pendingCount"], edits.len());

    // Staging the same kind again replaces it; staging the value on disk un-stages it.
    let answer = world.stage(json!({ "kind": "level", "value": 31 }));
    assert_eq!((&answer["staged"], &answer["character"]["pendingCount"]), (&json!(true), &json!(edits.len())));
    let answer = world.stage(json!({ "kind": "money", "value": 80 }));
    assert_eq!((&answer["staged"], &answer["changeId"]), (&json!(false), &Value::Null));
    assert!(!kinds(&answer["character"]).contains(&"money"));

    // "Fill ammo" is its own kind.
    world.ok("discard_changes", Value::Null);
    let answer = world.stage(json!({ "kind": "maximizeAmmo" }));
    assert_eq!(kinds(&answer["character"]), ["maximizeAmmo"]);
}

#[test]
fn refusals_carry_the_cores_words_and_stage_nothing() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.open("Save0003.sav");
    assert_eq!(world.err("stage", json!({ "kind": "level", "value": 81 })), "Level must be between 1 and 80.");
    assert_eq!(world.err("stage", json!({ "kind": "level", "value": 12.5 })), "Enter a whole number.");
    assert_eq!(world.err("stage", json!({ "kind": "level", "value": "12" })), "Enter a whole number.");
    assert_eq!(world.err("stage", json!({ "kind": "level" })), "Enter a whole number.");
    assert_eq!(world.err("stage", json!({ "kind": "name", "value": "" })), "Name can't be empty.");
    assert_eq!(world.err("stage", json!({ "kind": "gunSlots", "value": 5 })), "Gun slots must be 2, 3 or 4.");
    assert_eq!(world.err("stage", json!({ "kind": "money", "value": -1 })), "Must be between 0 and 2,147,483,647.");
    assert!(world.err("stage", json!({ "kind": "wings", "value": 2 })).contains("Unknown kind of change"));
    assert!(world.err("stage", json!({})).contains("kind"));
    assert_eq!(world.err("duplicate_item", json!({ "item": "junk" })), "That item is no longer in the list.");
    assert!(world.err("duplicate_item", json!({ "item": "53:1" })).starts_with("This item can't be duplicated"));
    assert_eq!(world.err("remove_change", json!({ "changeId": 99 })), "That change is no longer pending.");
    assert_eq!(world.ok("character", Value::Null)["pendingCount"], 0);
}

#[test]
fn removes_one_change_and_discards_all() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.open("Save0003.sav");
    world.stage(json!({ "kind": "level", "value": 30 }));
    let money = world.stage(json!({ "kind": "money", "value": 999 }))["changeId"].clone();
    world.stage(json!({ "kind": "gunSlots", "value": 3 }));
    let c = world.ok("remove_change", json!({ "changeId": money }));
    assert_eq!(kinds(&c), ["level", "gunSlots"]);
    assert_eq!((&c["current"]["money"], &c["current"]["level"]), (&json!(80), &json!(30)));
    let c = world.ok("discard_changes", Value::Null);
    assert_eq!(c["pendingCount"], 0);
    assert_eq!(c["current"], c["original"]);
}

#[test]
fn duplicate_item_stages_a_marked_copy_and_says_when_the_backpack_is_full() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.open("Save0003.sav");
    let answer = world.ok("duplicate_item", json!({ "item": "54:0" }));
    assert_eq!(answer["staged"], true);
    let new_ref = answer["newItems"][0].as_str().unwrap().to_string();
    let c = &answer["character"];
    assert_eq!(c["items"].as_array().unwrap().len(), 4);
    let copy = item(c, &new_ref);
    assert_eq!((&copy["isNew"], &copy["copyOf"], &copy["changeId"]), (&json!(true), &json!("54:0"), &answer["changeId"]));
    assert_eq!((&copy["location"], &copy["code"]), (&json!("backpack"), &item(c, "54:0")["code"]));
    assert_eq!(c["current"]["backpackUsed"], 4);
    let line = &c["changes"][0];
    assert_eq!((&line["kind"], &line["itemRef"], &line["label"]), (&json!("duplicate"), &json!("54:0"), &json!("Duplicate")));

    // Undo on the copy is "remove that change".
    let c = world.ok("remove_change", json!({ "changeId": answer["changeId"] }));
    assert_eq!((c["items"].as_array().unwrap().len(), &c["pendingCount"]), (3, &json!(0)));

    // Fill the backpack: 3 used of 12, so 9 more copies fit and the 10th does not.
    for _ in 0..9 {
        world.ok("duplicate_item", json!({ "item": "54:0" }));
    }
    let c = world.ok("character", Value::Null);
    assert_eq!((&c["current"]["backpackUsed"], &c["backpackFull"]), (&json!(12), &json!(true)));
    assert_eq!(world.err("duplicate_item", json!({ "item": "54:0" })), BACKPACK_FULL_MESSAGE);
    // A bigger backpack makes room again.
    world.stage(json!({ "kind": "backpackSlots", "value": 15 }));
    assert_eq!(world.ok("duplicate_item", json!({ "item": "53:0" }))["character"]["backpackFull"], false);
}

#[test]
fn item_codes_export_and_import() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.open("Save0003.sav");
    let exported = world.ok("export_codes", json!({ "includeBank": true }));
    let codes = exported["codes"].as_array().unwrap();
    // The same codes the items carry ("copy item code"): items first, then weapons.
    let c = world.ok("character", Value::Null);
    let carried: Vec<Value> = ["53:0", "53:1", "54:0"].iter().map(|r| item(&c, r)["code"].clone()).collect();
    assert_eq!(*codes, carried);
    assert_eq!((&codes[1], &codes[2]), (&json!(ECHO_CODE), &json!(GEAR_WEAPON_CODE)));
    assert_eq!(exported["text"].as_str().unwrap().lines().count(), codes.len());

    let pasted = format!("from a friend:\n{WEAPON_CODE}\n{ITEM_CODE}\n");
    let answer = world.stage(json!({ "kind": "importCodes", "text": pasted }));
    assert_eq!(answer["newItems"].as_array().unwrap().len(), 2);
    let c = &answer["character"];
    assert_eq!((c["items"].as_array().unwrap().len(), &c["changes"][0]["kind"]), (5, &json!("importCodes")));
    // An imported item has to be saved before it can be copied, and its button says so.
    let imported = item(c, answer["newItems"][0].as_str().unwrap());
    assert_eq!((&imported["isNew"], &imported["copyOf"], &imported["canDuplicate"]), (&json!(true), &Value::Null, &json!(false)));
    assert_eq!(imported["duplicateBlocked"], DUPLICATE_OF_IMPORT_MESSAGE);

    let bank = world.stage(json!({ "kind": "importCodes", "text": WEAPON_CODE, "toBank": true }));
    assert_eq!(item(&bank["character"], bank["newItems"][0].as_str().unwrap())["location"], "bank");

    assert_eq!(world.err("stage", json!({ "kind": "importCodes", "text": "hello" })), "No item codes found. A code looks like BL2(…).");
    assert!(world.err("stage", json!({ "kind": "importCodes", "text": "BL2(AAAA)" })).starts_with("Item code 1 can't be imported"));
}

#[test]
fn item_levels_follow_a_level_staged_afterwards() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.open("Save0003.sav");
    // "All gear to the character's level" first, then the level: the gear must end at 40.
    world.stage(json!({ "kind": "itemLevels", "level": 0, "force": true }));
    let answer = world.stage(json!({ "kind": "level", "value": 40 }));
    let c = &answer["character"];
    assert_eq!(kinds(c), ["level", "itemLevels"]);
    assert_eq!((&item(c, "54:0")["level"], &item(c, "53:0")["level"]), (&json!(40), &json!(40)));
    // Changing the level again keeps the order, and the gear follows.
    let c = world.stage(json!({ "kind": "level", "value": 50 }))["character"].clone();
    assert_eq!((kinds(&c), &item(&c, "54:0")["level"]), (vec!["level", "itemLevels"], &json!(50)));
}

#[test]
fn a_read_only_character_is_saved_as_a_new_one_then_edited_in_place_and_restored() {
    let mut world = World::new();
    let main = world.put("Save0001.sav", 1);
    world.open("Save0001.sav");
    world.stage(json!({ "kind": "level", "value": 30 }));
    world.stage(json!({ "kind": "backpackSlots", "value": 39 }));
    world.ok("duplicate_item", json!({ "item": "54:0" }));

    // A character Vault Key did not create, in a slot that is not marked, is never written.
    assert!(world.err("save", Value::Null).contains("read-only"));
    assert_eq!(world.read("Save0001.sav"), main);
    assert_eq!(world.ok("character", Value::Null)["pendingCount"], 3, "a refused save keeps the changes");
    // Only free slots 2 to 9 take a new character.
    assert!(world.api.handle("save_as_new", &json!({ "slot": 1 })).0.is_err());
    assert!(world.api.handle("save_as_new", &json!({ "slot": 10 })).0.is_err());
    assert_eq!(world.err("save_as_new", json!({ "slot": -2 })), "Choose a free slot.");
    assert_eq!(world.err("save_as_new", json!({})), "Enter a whole number.");

    let saved = world.ok("save_as_new", json!({ "slot": 4 }));
    let report = &saved["report"];
    assert_eq!((&report["slot"], &report["fileName"], &report["createdNew"], &report["backup"]), (&json!(4), &json!("Save0004.sav"), &json!(true), &Value::Null));
    assert_eq!(world.read("Save0001.sav"), main, "the original is untouched");
    // The app carries on in the new character, read back from disk, which it may edit.
    let c = &saved["character"];
    assert_eq!((&c["slot"], &c["pendingCount"], &c["access"]["writable"], &c["access"]["because"]), (&json!(4), &json!(0), &json!(true), &json!("created")));
    assert_eq!((&c["current"]["level"], &c["current"]["backpackSlots"], &c["current"]["saveGameId"]), (&json!(30), &json!(39), &json!(4)));
    assert_eq!((c["items"].as_array().unwrap().len(), &c["current"], &c["freeSlots"]), (4, &c["original"], &json!([2, 3, 5, 6, 7, 8, 9])));
    assert!(world.err("save_as_new", json!({ "slot": 4 })).contains("Save0004.sav"), "a taken slot is refused");
    assert!(world.err("save", Value::Null).contains("no changes"));

    // Edit the new character in place: a backup is taken first.
    let created = world.read("Save0004.sav");
    world.stage(json!({ "kind": "money", "value": 5_000 }));
    let saved = world.ok("save", Value::Null);
    let backup_id = saved["report"]["backup"]["id"].as_str().unwrap().to_string();
    assert_eq!((&saved["report"]["createdNew"], &saved["report"]["backup"]["reason"]), (&json!(false), &json!("Before saving 1 change")));
    assert_eq!((&saved["character"]["current"]["money"], &saved["character"]["pendingCount"]), (&json!(5_000), &json!(0)));
    assert_ne!(world.read("Save0004.sav"), created);

    let backups = world.ok("list_backups", Value::Null);
    let backups = backups["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!((&backups[0]["id"], &backups[0]["slot"], &backups[0]["characterName"], &backups[0]["level"]), (&json!(backup_id), &json!(4), &json!("Gaige"), &json!(30)));
    assert_eq!(world.ok("list_backups", json!({ "slotId": World::slot("Save0001.sav") }))["backups"], json!([]));
    assert_eq!(world.ok("list_backups", json!({ "all": true }))["backups"].as_array().unwrap().len(), 1);

    // Restore: the file is the backup again, pending edits are dropped, and what was there
    // is backed up in turn.
    world.stage(json!({ "kind": "level", "value": 60 }));
    let restored = world.ok("restore_backup", json!({ "backupId": backup_id }));
    assert_eq!(world.read("Save0004.sav"), created);
    let c = &restored["character"];
    assert_eq!((&c["slot"], &c["current"]["money"], &c["current"]["level"], &c["pendingCount"]), (&json!(4), &json!(80), &json!(30), &json!(0)));
    assert_eq!(world.ok("list_backups", Value::Null)["backups"].as_array().unwrap().len(), 2);
    assert!(world.err("restore_backup", json!({ "backupId": "nope" })).contains("no backup"));
    assert_eq!(world.read("Save0001.sav"), main);
}

#[test]
fn nothing_is_saved_while_the_game_runs() {
    let mut world = World::new();
    world.put("Save0001.sav", 1);
    world.open("Save0001.sav");
    world.stage(json!({ "kind": "level", "value": 30 }));
    world.running.store(true, Ordering::SeqCst);
    assert_eq!(world.err("save_as_new", json!({ "slot": 2 })), "Borderlands 2 is running. Quit the game, then save.");
    assert!(!world.dir.join("Save0002.sav").exists());
    // The next status poll shows it at once, not a stale "closed".
    assert_eq!(world.ok("status", Value::Null)["game"]["state"], "running");
    world.unreadable.store(true, Ordering::SeqCst);
    assert!(world.err("save_as_new", json!({ "slot": 2 })).contains("can't tell whether Borderlands 2 is running"));
    assert!(!world.dir.join("Save0002.sav").exists());

    world.unreadable.store(false, Ordering::SeqCst);
    world.running.store(false, Ordering::SeqCst);
    assert_eq!(world.ok("save_as_new", json!({ "slot": 2 }))["character"]["current"]["level"], 30);
}

#[test]
fn test_slots_are_marked_and_unmarked() {
    let mut world = World::new();
    world.put("Save0001.sav", 1);
    let before = world.put("Save0003.sav", 3);
    let c = world.open("Save0003.sav");
    assert_eq!(c["access"]["writable"], false);
    world.stage(json!({ "kind": "level", "value": 30 }));
    assert!(world.err("save", Value::Null).contains("read-only"));
    assert_eq!(world.read("Save0003.sav"), before);

    // Slot 1 can be marked like any other slot, and unmarked again.
    let first = world.ok("mark_test_slot", json!({ "slotId": World::slot("Save0001.sav") }));
    assert_eq!((&first["characters"][0]["isTestSlot"], &first["characters"][0]["access"]["writable"]), (&json!(true), &json!(true)));
    let first = world.ok("unmark_test_slot", json!({ "slotId": World::slot("Save0001.sav") }));
    assert_eq!((&first["characters"][0]["isTestSlot"], &first["characters"][0]["access"]["writable"]), (&json!(false), &json!(false)));
    assert!(world.err("mark_test_slot", json!({ "slotId": World::slot("Save0007.sav") })).contains("no Save0007.sav"));

    let marked = world.ok("mark_test_slot", json!({ "slotId": World::slot("Save0003.sav") }));
    let entry = &marked["characters"][1];
    assert_eq!((&entry["isTestSlot"], &entry["access"]["writable"], &entry["access"]["because"]), (&json!(true), &json!(true), &json!("test-slot")));
    // The open character sees it too, and its pending change is still there.
    let c = &marked["character"];
    assert_eq!((&c["access"]["writable"], &c["isTestSlot"], &c["pendingCount"]), (&json!(true), &json!(true), &json!(1)));
    let saved = world.ok("save", Value::Null);
    assert_eq!(saved["character"]["current"]["level"], 30);
    assert_ne!(world.read("Save0003.sav"), before);

    let unmarked = world.ok("unmark_test_slot", json!({ "slotId": World::slot("Save0003.sav") }));
    assert_eq!((&unmarked["characters"][1]["isTestSlot"], &unmarked["character"]["access"]["writable"]), (&json!(false), &json!(false)));
    world.stage(json!({ "kind": "level", "value": 31 }));
    assert!(world.err("save", Value::Null).contains("read-only"));
}

#[test]
fn item_data_arrives_later_and_is_handed_to_the_core() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    let (sender, result) = mpsc::channel();
    let percent = Arc::new(AtomicU32::new(0));
    world.api.expect_item_data(result, percent.clone());
    percent.store(40, Ordering::SeqCst);
    let status = world.ok("status", Value::Null);
    assert_eq!((&status["itemData"]["state"], &status["itemData"]["percent"]), (&json!("loading"), &json!(40)));

    // Meanwhile the app is usable, with the stand-in rule for what is gear.
    let c = world.open("Save0003.sav");
    assert_eq!(item(&c, "54:0")["canDuplicate"], true);
    world.stage(json!({ "kind": "level", "value": 30 }));
    world.ok("duplicate_item", json!({ "item": "54:0" }));

    // A catalog that knows nothing arrives: it allows no copies, so the staged copy goes,
    // the user is told once, and the other change stays.
    sender.send(Ok(Catalog::new(gamedata::ItemDatabase::new("test")))).unwrap();
    let status = world.ok("status", Value::Null);
    assert_eq!(status["itemData"]["state"], "ready");
    assert_eq!(status["notices"].as_array().unwrap().len(), 1);
    assert!(status["notices"][0].as_str().unwrap().contains("1 pending change was removed"));
    assert_eq!(world.ok("status", Value::Null)["notices"], json!([]));
    let c = world.ok("character", Value::Null);
    assert_eq!(kinds(&c), ["level"]);
    let weapon = item(&c, "54:0");
    assert_eq!((&weapon["canDuplicate"], &weapon["name"], &weapon["tier"]), (&json!(false), &Value::Null, &json!("unknown")));
    assert_eq!(weapon["duplicateBlocked"], "The game's item data does not say what this is, so it can't be duplicated.");
    // Characters opened from now on use it as well.
    assert_eq!(item(&world.open("Save0003.sav"), "54:0")["canDuplicate"], false);
}

#[test]
fn item_data_that_cannot_be_loaded_says_why() {
    let mut world = World::new();
    let (sender, result) = mpsc::channel();
    world.api.expect_item_data(result, Arc::default());
    sender.send(Err("Borderlands 2 was not found.".to_string())).unwrap();
    let status = world.ok("status", Value::Null);
    assert_eq!((&status["itemData"]["state"], &status["itemData"]["message"]), (&json!("unavailable"), &json!("Borderlands 2 was not found.")));

    // A loader that dies without an answer does not leave "loading" up for ever.
    let (sender, result) = mpsc::channel::<Result<Catalog, String>>();
    world.api.expect_item_data(result, Arc::default());
    drop(sender);
    assert_eq!(world.ok("status", Value::Null)["itemData"]["state"], "unavailable");
}

#[test]
fn the_selftest_log_is_written_only_in_selftest_mode() {
    let mut world = World::new();
    assert_eq!(world.err("selftest_log", json!({ "line": "x" })), "The self-test is not enabled.");
    let dir = world.temp.path().join("VaultKey");
    assert!(!dir.join(SELFTEST_LOG_NAME).exists());

    world.api.selftest_dir = Some(dir.clone());
    assert_eq!(world.ok("status", Value::Null)["selftest"], true);
    world.ok("selftest_log", json!({ "line": "PASS one" }));
    world.ok("selftest_log", json!({ "line": "FAIL two\nwith a break" }));
    let text = fs::read_to_string(dir.join(SELFTEST_LOG_NAME)).unwrap();
    assert_eq!(text.lines().collect::<Vec<_>>(), ["PASS one", "FAIL two with a break"]);
}

/// The fixture save (a real level-1 character), when it is there. It is not in the
/// repository, so tests that use it pass without it.
fn fixture() -> Option<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/local/win-throwaway-Save0001.sav");
    let bytes = fs::read(path).ok();
    if bytes.is_none() {
        eprintln!("skipped: no fixture save");
    }
    bytes
}

#[test]
fn a_real_save_goes_through_the_whole_flow() {
    let Some(bytes) = fixture() else { return };
    let mut world = World::new();
    fs::write(world.dir.join("Save0001.sav"), &bytes).unwrap();
    let c = world.open("Save0001.sav");
    let items = c["items"].as_array().unwrap();
    let weapon = items.iter().find(|i| i["isWeapon"] == true && i["canDuplicate"] == true).expect("a weapon")["ref"].clone();
    let count = items.len();

    world.stage(json!({ "kind": "level", "value": 30 }));
    world.stage(json!({ "kind": "money", "value": 123_456 }));
    world.stage(json!({ "kind": "backpackSlots", "value": 39 }));
    world.ok("duplicate_item", json!({ "item": weapon }));
    let saved = world.ok("save_as_new", json!({ "slot": 2 }));
    let c = &saved["character"];
    assert_eq!((&c["current"]["level"], &c["current"]["money"], &c["current"]["backpackSlots"]), (&json!(30), &json!(123_456), &json!(39)));
    assert_eq!(c["items"].as_array().unwrap().len(), count + 1);
    assert_eq!(world.read("Save0001.sav"), bytes);

    world.stage(json!({ "kind": "eridium", "value": 50 }));
    let saved = world.ok("save", Value::Null);
    let backup = saved["report"]["backup"]["id"].clone();
    assert_eq!(saved["character"]["current"]["eridium"], 50);
    let restored = world.ok("restore_backup", json!({ "backupId": backup }));
    assert_ne!(restored["character"]["current"]["eridium"], 50);
}

#[test]
fn real_game_data_names_a_real_saves_items() {
    let Some(bytes) = fixture() else { return };
    // A temp cache folder: tests never write to Vault Key's real data folder.
    let cache = TempDir::new();
    let Ok(catalog) = Catalog::load(Some(cache.path().to_path_buf()), &mut |_| {}) else {
        eprintln!("skipped: no game install found");
        return;
    };
    let mut world = World::new();
    fs::write(world.dir.join("Save0001.sav"), &bytes).unwrap();
    let (sender, result) = mpsc::channel();
    world.api.expect_item_data(result, Arc::default());
    sender.send(Ok(catalog)).unwrap();
    let c = world.open("Save0001.sav");
    for i in c["items"].as_array().unwrap() {
        if i["isWeapon"] == true && i["raw"]["set"] == 0 {
            assert!(i["name"].is_string() && i["manufacturer"].is_string(), "{i}");
            assert!(i["color"].as_str().is_some_and(|c| c.len() == 7 && c.starts_with('#')), "{i}");
            assert_eq!((&i["canDuplicate"], &i["duplicateBlocked"]), (&json!(true), &Value::Null), "{i}");
            // A base-game weapon has its card: damage first, every row with a label and a value.
            let rows = i["stats"]["rows"].as_array().unwrap_or_else(|| panic!("no card for {i}"));
            assert_eq!(rows[0]["key"], "damage", "{i}");
            assert!(rows.iter().all(|r| r["label"].is_string() && r["text"].is_string()), "{i}");
        }
        // Nothing unverified, and nothing that is not gear, is offered for duplication.
        if i["unverified"] == true || matches!(i["category"].as_str(), Some("customization" | "mission" | "usable" | "unknown")) {
            assert_eq!(i["canDuplicate"], false, "{i}");
            assert!(i["duplicateBlocked"].is_string(), "{i}");
        }
    }
}

#[test]
fn a_file_from_elsewhere_is_opened_exported_and_added_but_never_written() {
    let mut world = World::new();
    world.put("Save0002.sav", 2);
    let outside = world.temp.path().join("Outside");
    fs::create_dir_all(&outside).unwrap();
    let source = outside.join("Friend.sav");
    let bytes = sample_bytes(2);
    fs::write(&source, &bytes).unwrap();

    let character = world.ok("open_file", json!({ "path": source.display().to_string() }));
    assert_eq!(character["source"]["kind"], "external");
    assert_eq!(character["source"]["fileName"], "Friend.sav");
    assert_eq!(character["source"]["folder"], outside.display().to_string());
    assert_eq!(character["slotId"], format!("file:{}", source.display()));
    assert_eq!((character["slot"].clone(), character["access"]["writable"].clone()), (Value::Null, json!(false)));
    assert_eq!(character["access"]["explanation"], gkcore::app::EXTERNAL_MESSAGE);
    // It goes into the only Steam-ID folder there is.
    assert_eq!(character["steamId"], STEAM_ID);
    assert_eq!(character["freeSlots"], json!([3, 4, 5, 6, 7, 8, 9]));
    assert_eq!(world.ok("status", json!({}))["open"], format!("file:{}", source.display()));
    assert_eq!(world.ok("list_backups", json!({}))["backups"], json!([]));

    world.stage(json!({ "kind": "money", "value": 4242 }));
    assert_eq!(world.err("save", json!({})), gkcore::app::EXTERNAL_MESSAGE);
    assert!(world.err("export_file", json!({ "path": world.dir.join("Save0009.sav").display().to_string() })).contains("save folder"));

    let target = outside.join("Save0002.sav");
    let report = world.ok("export_file", json!({ "path": target.display().to_string() }))["report"].clone();
    assert_eq!((report["fileName"].clone(), report["saveGameId"].clone(), report["replacedBackup"].clone()), (json!("Save0002.sav"), json!(2), Value::Null));
    assert_eq!(gkcore::SaveDocument::from_file_bytes(&fs::read(&target).unwrap()).unwrap().money(), 4242);
    // The changes are still pending after the export.
    assert_eq!(world.ok("character", json!({}))["pendingCount"], 1);

    // Re-opened, the export holds the change; added to my characters, it gets slot 3's ID.
    let exported = world.ok("open_file", json!({ "path": target.display().to_string() }));
    assert_eq!(exported["current"]["money"], 4242);
    let answer = world.ok("save_as_new", json!({ "slot": 3 }));
    assert_eq!(answer["report"]["slotId"], World::slot("Save0003.sav"));
    assert_eq!(answer["character"]["slotId"], World::slot("Save0003.sav"));
    assert_eq!(answer["character"]["current"]["saveGameId"], 3);
    assert_eq!(fs::read(&source).unwrap(), bytes, "the opened file is untouched");

    // Exported back over the file it was opened from: replaced, and the old file is kept.
    world.ok("open_file", json!({ "path": source.display().to_string() }));
    world.stage(json!({ "kind": "money", "value": 777 }));
    let report = world.ok("export_file", json!({ "path": source.display().to_string() }))["report"].clone();
    assert_eq!(fs::read(report["replacedBackup"].as_str().expect("the old file is kept")).unwrap(), bytes);
    assert_eq!(gkcore::SaveDocument::from_file_bytes(&fs::read(&source).unwrap()).unwrap().money(), 777);

    // Opening a file that is in the save folder opens that slot.
    let slot = world.ok("open_file", json!({ "path": world.dir.join("Save0002.sav").display().to_string() }));
    assert_eq!((slot["slotId"].clone(), slot["source"]["kind"].clone()), (json!(World::slot("Save0002.sav")), json!("slot")));
    assert!(world.err("open_file", json!({ "path": "relative.sav" })).contains("full path"));
    assert!(world.err("open_file", json!({})).contains("Missing"));
}

#[test]
fn an_item_is_levelled_moved_to_the_bank_and_ammo_upgraded_then_saved() {
    let mut world = World::new();
    world.put("Save0003.sav", 3);
    world.ok("unlock_character", json!({ "slotId": World::slot("Save0003.sav") }));
    let c = world.open("Save0003.sav");
    let weapon = item(&c, "54:0").clone();
    assert_eq!((&weapon["location"], &weapon["quickSlot"]), (&json!("backpack"), &Value::Null));
    assert!(weapon["uid"].as_str().unwrap().starts_with('w'));
    assert_eq!(c["current"]["ammo"].as_array().unwrap().len(), 7);
    assert_eq!(c["current"]["ammo"][5]["name"], "sniper");

    world.stage(json!({ "kind": "itemLevel", "item": "54:0", "level": 33 }));
    let c = world.stage(json!({ "kind": "moveItem", "item": "54:0", "toBank": true }))["character"].clone();
    let banked = item(&c, "41:0");
    assert_eq!((&banked["uid"], &banked["location"], &banked["level"]), (&weapon["uid"], &json!("bank"), &json!(33)));
    assert_eq!(c["current"]["bankUsed"], 1);
    let c = world.stage(json!({ "kind": "ammoLevel", "ammo": "sniper", "level": 2 }))["character"].clone();
    assert_eq!(kinds(&c), ["itemLevel", "moveItem", "ammoLevel"]);
    assert_eq!(c["changes"][1]["itemRef"], "41:0");
    assert_eq!((&c["current"]["ammo"][5]["level"], &c["current"]["ammo"][5]["capacity"]), (&json!(2), &json!(72)));
    assert!(world.err("stage", json!({ "kind": "ammoLevel", "ammo": "laser", "level": 2 })).contains("Unknown ammo"));
    assert!(world.err("stage", json!({ "kind": "moveItem", "item": "54:x" })).contains("item reference"));

    world.ok("save", Value::Null);
    let saved = SaveDocument::from_file_bytes(&world.read("Save0003.sav")).unwrap();
    let in_bank: Vec<_> = saved.inventory().into_iter().filter(|e| e.location == savekit::ItemLocation::Bank).collect();
    assert_eq!(in_bank.len(), 1);
    assert_eq!(in_bank[0].fields.as_ref().unwrap().grade_index, Some(33));
    assert_eq!(saved.ammo_levels()[5], 2);
}

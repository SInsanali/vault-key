//! The `gk` commands. Each returns the text to print, so tests can call them directly.

use std::fs;
use std::path::{Path, PathBuf};

use savekit::{
    verify, CharacterKey, Currency, EntryRef, Expectation, GearClassifier, ItemLocation, SaveDocument,
};

use gkcore::guards::{self, refuse, GkError};

/// Why a command did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Bad arguments: print the message and the usage text, exit code 2.
    Usage(String),
    /// A refusal or an error: print the one-line reason, exit code 1.
    Error(String),
}

impl From<GkError> for Failure {
    fn from(e: GkError) -> Self {
        Failure::Error(e.0)
    }
}

fn usage<T>(message: impl Into<String>) -> Result<T, Failure> {
    Err(Failure::Usage(message.into()))
}

/// Everything a command needs from the outside world, so tests can substitute it.
pub struct Context {
    /// The SaveData folder `install` works in (real, or the `GK_SAVEDATA_DIR` override).
    /// An error when it could not be found; only `install` needs it.
    pub root: Result<PathBuf, GkError>,
    /// Real SaveData folders on this machine. Output is never written inside these.
    pub real_roots: Vec<PathBuf>,
    /// Image names of running processes; an error means "cannot tell".
    pub process_names: Box<dyn Fn() -> Result<Vec<String>, GkError>>,
    /// Decides which inventory entries are gear and so may be duplicated.
    pub classifier: Box<dyn GearClassifier>,
}

impl Context {
    fn protected_roots(&self) -> Vec<PathBuf> {
        let mut roots = self.real_roots.clone();
        if let Ok(r) = &self.root {
            if !roots.contains(r) {
                roots.push(r.clone());
            }
        }
        roots
    }

    fn check_output(&self, out: &Path) -> Result<(), GkError> {
        guards::check_output(out, &self.protected_roots())
    }
}

pub fn load(path: &Path) -> Result<(Vec<u8>, SaveDocument), GkError> {
    let data = fs::read(path).map_err(|e| GkError(format!("cannot read {}: {e}", path.display())))?;
    match SaveDocument::from_file_bytes(&data) {
        Ok(doc) => Ok((data, doc)),
        Err(e) => refuse(format!("not a valid save: {} ({e})", path.display())),
    }
}

fn location_name(l: ItemLocation) -> &'static str {
    match l {
        ItemLocation::Backpack => "backpack",
        ItemLocation::Equipped => "equipped",
        ItemLocation::Bank => "bank",
    }
}

pub fn info(ctx: &Context, path: &Path) -> Result<String, Failure> {
    let (_, doc) = load(path)?;
    let mut out = String::new();
    out += &format!("name: {}  class: {}  save id: {}\n", doc.name(), doc.class_path(), doc.save_game_id());
    out += &format!("level: {}  xp: {}  op level: {}\n", doc.level(), doc.experience(), doc.op_level());
    out += &format!(
        "money: {}  eridium: {}  seraph: {}  torgue: {}\n",
        doc.money(),
        doc.eridium(),
        doc.seraph(),
        doc.torgue()
    );
    out += &format!(
        "slots: backpack {} (used {})  bank {}  guns {}\n",
        doc.backpack_slots(),
        doc.backpack_used(),
        doc.bank_slots(),
        doc.gun_slots()
    );
    for e in doc.inventory() {
        let grade = match e.fields.as_ref().and_then(|f| f.grade_index) {
            Some(g) => g.to_string(),
            None => "?".to_string(),
        };
        let kind = if e.is_virtual {
            "virtual"
        } else if e.is_weapon {
            "weapon"
        } else {
            "item"
        };
        out += &format!(
            "[{}] {} {} lvl {} dup:{}\n",
            e.entry_ref,
            location_name(e.location),
            kind,
            grade,
            if e.is_duplicable(ctx.classifier.as_ref()) { "yes" } else { "no" }
        );
    }
    Ok(out)
}

/// Every inventory entry with what the game data says about it.
pub fn names(path: &Path, catalog: &gkcore::Catalog) -> Result<String, Failure> {
    let (_, doc) = load(path)?;
    let mut out = String::new();
    for e in doc.inventory() {
        if e.is_virtual {
            continue;
        }
        let Some(fields) = &e.fields else {
            out += &format!("[{}] {} (does not decode)\n", e.entry_ref, location_name(e.location));
            continue;
        };
        let d = catalog.describe(fields, e.is_weapon);
        let level = fields.grade_index.map_or("?".to_string(), |g| g.to_string());
        out += &format!(
            "[{}] {} lvl {} {} {} {} {} \"{}\"{} dup:{}\n",
            e.entry_ref,
            location_name(e.location),
            level,
            d.tier,
            d.color.as_deref().unwrap_or("-"),
            d.manufacturer.as_deref().unwrap_or("?"),
            d.category,
            d.name.as_deref().unwrap_or("?"),
            if d.unverified { " (unverified)" } else { "" },
            if e.is_duplicable(catalog) { "yes" } else { "no" }
        );
    }
    Ok(out)
}

/// Item codes for everything exportable, one per line.
pub fn export(path: &Path) -> Result<String, Failure> {
    let (_, doc) = load(path)?;
    let mut out = String::new();
    for code in doc.export_codes(true) {
        out += &code;
        out.push('\n');
    }
    Ok(out)
}

fn encode_verified(
    original: &SaveDocument,
    edited: &SaveDocument,
    expectations: &[Expectation],
) -> Result<Vec<u8>, GkError> {
    let data = edited.to_file_bytes().map_err(|e| GkError(format!("cannot encode the save: {e}")))?;
    verify(original, &data, expectations)
        .map_err(|e| GkError(format!("verification failed, nothing written: {e}")))?;
    Ok(data)
}

pub fn duplicate(ctx: &Context, input: &Path, out: &Path, reference: &str) -> Result<String, Failure> {
    let source: EntryRef = match reference.parse() {
        Ok(r) => r,
        Err(_) => return usage(format!("bad entry reference '{reference}' (expected <field>:<index>, e.g. 54:0)")),
    };
    ctx.check_output(out)?;
    let (_, original) = load(input)?;
    let mut doc = original.clone();
    doc.duplicate(source, ctx.classifier.as_ref())
        .map_err(|e| GkError(format!("duplicate failed: {e}")))?;
    let data = encode_verified(&original, &doc, &[Expectation::Duplicated { source, count: 1 }])?;
    guards::write_new_file(out, &data)?;
    Ok(format!("wrote {} (verified: 1 new copy of {source})\n", out.display()))
}

pub fn import(ctx: &Context, input: &Path, out: &Path, codes: &[String]) -> Result<String, Failure> {
    if codes.is_empty() {
        return usage("import needs at least one item code");
    }
    ctx.check_output(out)?;
    let (_, original) = load(input)?;
    let mut doc = original.clone();
    doc.import_codes(codes, false).map_err(|e| GkError(format!("import failed: {e}")))?;
    let data = encode_verified(&original, &doc, &[])?;
    guards::write_new_file(out, &data)?;
    let n = codes.len();
    Ok(format!(
        "wrote {} (verified: imported {n} item{} into the backpack)\n",
        out.display(),
        if n == 1 { "" } else { "s" }
    ))
}

pub const SET_KEYS: &str = "name savegameid level money eridium seraph torgue backpack bank gunslots oplevel";

pub fn set(ctx: &Context, input: &Path, out: &Path, assignments: &[String]) -> Result<String, Failure> {
    if assignments.is_empty() {
        return usage("set needs at least one key=value");
    }
    ctx.check_output(out)?;
    let (_, original) = load(input)?;
    let mut doc = original.clone();
    let mut expect: Vec<Expectation> = Vec::new();
    for a in assignments {
        let Some((key, val)) = a.split_once('=') else {
            return usage(format!("bad assignment '{a}' (expected key=value)"));
        };
        if key == "name" {
            doc.set_name(val).map_err(|e| GkError(format!("name: {e}")))?;
            expect.push(Expectation::Name(val.to_string()));
            continue;
        }
        let known = matches!(
            key,
            "savegameid" | "level" | "money" | "eridium" | "seraph" | "torgue" | "backpack" | "bank" | "gunslots" | "oplevel"
        );
        if !known {
            return usage(format!("unknown key '{key}' ({SET_KEYS})"));
        }
        let n: i64 = match val.parse() {
            Ok(n) => n,
            Err(_) => return Err(Failure::Error(format!("value for {key} must be a whole number"))),
        };
        let fail = |e: savekit::SaveKitError| GkError(format!("{key}: {e}"));
        let expected = match key {
            "savegameid" => {
                doc.set_save_game_id(n).map_err(fail)?;
                (CharacterKey::SaveGameId, n)
            }
            "level" => {
                doc.set_level(n).map_err(fail)?;
                (CharacterKey::Level, n)
            }
            "money" => {
                doc.set_currency(Currency::Money, n).map_err(fail)?;
                (CharacterKey::Money, n)
            }
            "eridium" => {
                doc.set_currency(Currency::Eridium, n).map_err(fail)?;
                (CharacterKey::Eridium, n)
            }
            "seraph" => {
                doc.set_currency(Currency::Seraph, n).map_err(fail)?;
                (CharacterKey::Seraph, n)
            }
            "torgue" => {
                doc.set_currency(Currency::Torgue, n).map_err(fail)?;
                (CharacterKey::Torgue, n)
            }
            // Backpack and bank sizes round up to a whole upgrade, so expect what was applied.
            "backpack" => {
                doc.set_backpack_slots(n).map_err(fail)?;
                (CharacterKey::BackpackSlots, doc.backpack_slots())
            }
            "bank" => {
                doc.set_bank_slots(n).map_err(fail)?;
                (CharacterKey::BankSlots, doc.bank_slots())
            }
            "gunslots" => {
                doc.set_gun_slots(n).map_err(fail)?;
                (CharacterKey::GunSlots, n)
            }
            _ => {
                doc.set_op_level(n).map_err(fail)?;
                (CharacterKey::OpLevel, n)
            }
        };
        expect.push(Expectation::CharacterValue(expected.0, expected.1));
    }
    let data = encode_verified(&original, &doc, &expect)?;
    guards::write_new_file(out, &data)?;
    Ok(format!("wrote {} (verified: {})\n", out.display(), assignments.join(" ")))
}

/// Copy a verified save into a free slot of the SaveData folder. Never overwrites.
pub fn install(ctx: &Context, file: &Path, slot_arg: &str) -> Result<String, Failure> {
    let slot = guards::slot_number(slot_arg)?;
    let root = ctx.root.clone()?;
    let dir = guards::steam_dir(&root)?;
    let target = dir.join(guards::slot_file_name(slot));
    if fs::symlink_metadata(&target).is_ok() {
        return Err(refused(format!(
            "refused: slot {slot} already exists ({}); pick a free slot",
            guards::slot_file_name(slot)
        )));
    }
    if guards::game_is_running(&(ctx.process_names)()?) {
        return Err(refused("refused: the game is running; quit it first"));
    }
    let (data, doc) = load(file)?;

    // Save-game ID guards: the ID is the character's identity inside the game.
    let id = doc.save_game_id();
    if id == 1 {
        return Err(refused(
            "refused: this save has save-game ID 1 (slot 1's); never install a copy of the slot 1 character",
        ));
    }
    if guards::is_real_save_0001(&data, &ctx.protected_roots()) {
        return Err(refused(
            "refused: this file is identical to a real Save0001.sav; never install a copy of the slot 1 character",
        ));
    }
    if id != i64::from(slot) {
        return Err(refused(format!(
            "refused: save-game ID is {id} but the target slot is {slot}; run 'gk set <in> <out> savegameid={slot}' first and install the result"
        )));
    }
    let id_of = |bytes: &[u8]| {
        SaveDocument::from_file_bytes(bytes).ok().and_then(|d| u64::try_from(d.save_game_id()).ok())
    };
    if let Some(other) = guards::slot_using_save_game_id(u64::from(slot), &dir, &id_of)? {
        return Err(refused(format!(
            "refused: {other} already uses save-game ID {slot}; two saves with the same ID could overwrite each other"
        )));
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    guards::place_exclusively(&target, &data, &format!("{}-{nanos}", std::process::id()))?;
    Ok(format!("{}\n", target.display()))
}

fn refused(message: impl Into<String>) -> Failure {
    Failure::Error(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use savekit::FallbackGearClassifier;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Sandbox {
        dir: PathBuf,
        ctx: Context,
        running: Rc<Cell<bool>>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    impl Sandbox {
        fn new(tag: &str) -> Sandbox {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("gk-cmd-{tag}-{}-{nanos}", std::process::id()));
            let root = dir.join("SaveData");
            fs::create_dir_all(root.join("76561190000000000")).unwrap();
            let running = Rc::new(Cell::new(false));
            let flag = running.clone();
            let ctx = Context {
                root: Ok(root.clone()),
                real_roots: vec![root],
                process_names: Box::new(move || {
                    Ok(if flag.get() { vec!["Borderlands2.exe".to_string()] } else { vec!["explorer.exe".to_string()] })
                }),
                classifier: Box::new(FallbackGearClassifier),
            };
            Sandbox { dir, ctx, running }
        }
        fn slot_dir(&self) -> PathBuf {
            self.dir.join("SaveData").join("76561190000000000")
        }
        /// A minimal save with the given save-game ID, written outside the save folder.
        fn save(&self, name: &str, id: i64) -> PathBuf {
            let mut doc = SaveDocument::default();
            doc.set_save_game_id(id).unwrap();
            let p = self.dir.join(name);
            fs::write(&p, doc.to_file_bytes().unwrap()).unwrap();
            p
        }
        fn put_in_slot(&self, file_name: &str, id: i64) {
            let mut doc = SaveDocument::default();
            doc.set_save_game_id(id).unwrap();
            fs::write(self.slot_dir().join(file_name), doc.to_file_bytes().unwrap()).unwrap();
        }
    }

    fn error_text(r: Result<String, Failure>) -> String {
        match r {
            Err(Failure::Error(m)) | Err(Failure::Usage(m)) => m,
            Ok(o) => panic!("expected a failure, got: {o}"),
        }
    }

    #[test]
    fn install_places_a_matching_save_in_a_free_slot() {
        let s = Sandbox::new("ok");
        let src = s.save("three.sav", 3);
        let printed = install(&s.ctx, &src, "3").unwrap();
        let target = s.slot_dir().join("Save0003.sav");
        assert_eq!(printed.trim(), target.display().to_string());
        assert_eq!(fs::read(&target).unwrap(), fs::read(&src).unwrap());
        // Installing again must not overwrite.
        assert!(error_text(install(&s.ctx, &src, "3")).contains("already exists"));
    }

    #[test]
    fn install_refuses_slot_1_and_id_1() {
        let s = Sandbox::new("one");
        let one = s.save("one.sav", 1);
        assert!(error_text(install(&s.ctx, &one, "1")).contains("slot 1"));
        assert!(error_text(install(&s.ctx, &one, "2")).contains("save-game ID 1"));
        assert!(fs::read_dir(s.slot_dir()).unwrap().next().is_none(), "nothing may be written");
    }

    #[test]
    fn install_refuses_id_mismatch_and_collisions() {
        let s = Sandbox::new("ids");
        let four = s.save("four.sav", 4);
        assert!(error_text(install(&s.ctx, &four, "3")).contains("save-game ID is 4 but the target slot is 3"));
        // Another file in the folder already claims ID 4, under an unexpected name.
        s.put_in_slot("SAVE0009.SAV", 4);
        assert!(error_text(install(&s.ctx, &four, "4")).contains("already uses save-game ID 4"));
        assert!(!s.slot_dir().join("Save0004.sav").exists());
    }

    #[test]
    fn install_fails_closed_on_an_unreadable_neighbour() {
        let s = Sandbox::new("closed");
        fs::write(s.slot_dir().join("Save0007.sav"), b"not a save").unwrap();
        let three = s.save("three.sav", 3);
        assert!(error_text(install(&s.ctx, &three, "3")).contains("Save0007.sav"));
        assert!(!s.slot_dir().join("Save0003.sav").exists());
    }

    #[test]
    fn install_refuses_while_the_game_runs_or_when_it_cannot_tell() {
        let mut s = Sandbox::new("running");
        let three = s.save("three.sav", 3);
        s.running.set(true);
        assert!(error_text(install(&s.ctx, &three, "3")).contains("game is running"));
        s.running.set(false);
        s.ctx.process_names = Box::new(|| refuse("refused: cannot list running processes"));
        assert!(error_text(install(&s.ctx, &three, "3")).contains("cannot list running processes"));
        assert!(!s.slot_dir().join("Save0003.sav").exists());
    }

    #[test]
    fn install_refuses_a_copy_of_the_real_slot_1_file() {
        let s = Sandbox::new("copy");
        // The real slot 1 file happens to carry ID 5 here, so only the byte comparison catches it.
        s.put_in_slot("Save0001.sav", 5);
        let copy = s.dir.join("copy.sav");
        fs::copy(s.slot_dir().join("Save0001.sav"), &copy).unwrap();
        assert!(error_text(install(&s.ctx, &copy, "5")).contains("identical to a real Save0001.sav"));
    }

    #[test]
    fn install_refuses_garbage_and_bad_slots() {
        let s = Sandbox::new("bad");
        let junk = s.dir.join("junk.sav");
        fs::write(&junk, b"junk").unwrap();
        assert!(error_text(install(&s.ctx, &junk, "3")).contains("not a valid save"));
        let three = s.save("three.sav", 3);
        for slot in ["0", "10", "x"] {
            assert!(error_text(install(&s.ctx, &three, slot)).contains("slot must be"));
        }
    }

    #[test]
    fn set_writes_only_outside_the_save_folder_and_never_overwrites() {
        let s = Sandbox::new("set");
        let src = s.save("src.sav", 2);
        let inside = s.slot_dir().join("Save0005.sav");
        let a = vec!["savegameid=5".to_string()];
        assert!(error_text(set(&s.ctx, &src, &inside, &a)).contains("must not be inside"));
        assert!(!inside.exists());

        let out = s.dir.join("out.sav");
        let printed = set(&s.ctx, &src, &out, &a).unwrap();
        assert!(printed.contains("verified: savegameid=5"));
        assert_eq!(load(&out).unwrap().1.save_game_id(), 5);
        assert!(error_text(set(&s.ctx, &src, &out, &a)).contains("already exists"));
        assert_eq!(load(&src).unwrap().1.save_game_id(), 2, "the input is never modified");
    }

    #[test]
    fn set_rejects_bad_assignments_before_writing() {
        let s = Sandbox::new("setbad");
        let src = s.save("src.sav", 2);
        let out = s.dir.join("out.sav");
        for (arg, expect) in [
            ("level", "expected key=value"),
            ("colour=3", "unknown key"),
            ("level=abc", "whole number"),
            ("level=999", "level"),
        ] {
            let r = set(&s.ctx, &src, &out, &[arg.to_string()]);
            assert!(error_text(r).contains(expect), "{arg}");
            assert!(!out.exists(), "{arg}");
        }
        assert!(matches!(set(&s.ctx, &src, &out, &[]), Err(Failure::Usage(_))));
    }

    #[test]
    fn duplicate_reports_bad_references_and_missing_entries() {
        let s = Sandbox::new("dup");
        let src = s.save("src.sav", 2);
        let out = s.dir.join("out.sav");
        assert!(matches!(duplicate(&s.ctx, &src, &out, "nonsense"), Err(Failure::Usage(_))));
        assert!(error_text(duplicate(&s.ctx, &src, &out, "54:0")).contains("duplicate failed"));
        assert!(!out.exists());
    }

    fn fixtures() -> Vec<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/local");
        let mut v: Vec<PathBuf> = fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "sav")).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn fixture_duplicate_and_set_round_trip() {
        for fixture in fixtures() {
            let s = Sandbox::new("fx");
            let text = info(&s.ctx, &fixture).unwrap();
            assert!(text.starts_with("name: "));
            let (_, original) = load(&fixture).unwrap();
            let Some(weapon) = original
                .inventory()
                .into_iter()
                .find(|e| e.is_weapon && e.is_duplicable(s.ctx.classifier.as_ref()))
            else {
                continue;
            };
            // Make room first: a fresh character's backpack can already be over capacity.
            let roomy = s.dir.join("roomy.sav");
            set(&s.ctx, &fixture, &roomy, &["backpack=39".to_string(), "savegameid=3".to_string()]).unwrap();
            let dup = s.dir.join("dup.sav");
            let printed = duplicate(&s.ctx, &roomy, &dup, &weapon.entry_ref.to_string()).unwrap();
            assert!(printed.contains("verified: 1 new copy"));
            let (_, after) = load(&dup).unwrap();
            let weapons = |d: &SaveDocument| d.inventory().iter().filter(|e| e.is_weapon).count();
            assert_eq!(weapons(&after), weapons(&original) + 1);
            assert_eq!(after.backpack_slots(), 39);
            assert_eq!(after.save_game_id(), 3);
            assert_eq!(after.name(), original.name());
            // And the result installs into the sandbox slot 3.
            install(&s.ctx, &dup, "3").unwrap();
            assert!(s.slot_dir().join("Save0003.sav").exists());
        }
    }
}

//! `gk`: Vault Key's developer command-line tool.
//!
//! Every command that writes produces a new file and never touches its input. The only way
//! into the game's save folder is `install`, which only fills a free slot.

mod commands;

use std::path::Path;
use std::process::ExitCode;

use commands::{Context, Failure};
use gkcore::guards;

const USAGE: &str = "usage:
  gk info <file.sav>
  gk names <file.sav>                        (item names and rarity; needs the game installed)
  gk export <file.sav>                       (item codes, one per line)
  gk import <in.sav> <out.sav> <code>...     (items into the backpack)
  gk duplicate <in.sav> <out.sav> <field>:<index>
  gk set <in.sav> <out.sav> key=value ...    (name savegameid level money eridium seraph torgue backpack bank gunslots oplevel)
  gk install <file.sav> <slot>               (slots 2 to 9; never overwrites)
  gk protect <slot>                          (lock: Vault Key never writes that character)
  gk unlock <slot>                           (take the lock off: Vault Key may edit it in place)
  gk test-slot <slot>                        (Vault Key may edit that character in place)
  gk slots                                   (the characters in the save folder and what Vault Key may do)";

/// The game data, read from the local cache or the game's files (read-only).
fn catalog() -> Result<gkcore::Catalog, String> {
    gkcore::Catalog::load(None, &mut |_| {})
}

/// `with_game_data`: classify gear from the game data when it can be read, so story and DLC
/// items are not offered for duplication. Without it the manufacturer rule is used.
fn context(with_game_data: bool) -> Context {
    let env = |k: &str| std::env::var_os(k);
    let classifier: Box<dyn savekit::GearClassifier> = match with_game_data.then(catalog) {
        Some(Ok(catalog)) => Box::new(catalog),
        _ => Box::new(savekit::FallbackGearClassifier),
    };
    Context {
        root: guards::save_data_root(&env),
        real_roots: guards::real_save_data_dirs(),
        process_names: Box::new(guards::live_process_names),
        classifier,
    }
}

fn run(args: &[String]) -> Result<String, Failure> {
    let command = args.first().map(String::as_str);
    let ctx = context(matches!(command, Some("info" | "duplicate")));
    let path = |i: usize| Path::new(&args[i]);
    match (command, args.len()) {
        (Some("info"), 2) => commands::info(&ctx, path(1)),
        (Some("names"), 2) => match catalog() {
            Ok(catalog) => commands::names(path(1), &catalog),
            Err(why) => Err(Failure::Error(format!("no game data ({why}); 'names' needs it"))),
        },
        (Some("export"), 2) => commands::export(path(1)),
        (Some("import"), n) if n >= 4 => commands::import(&ctx, path(1), path(2), &args[3..]),
        (Some("duplicate"), 4) => commands::duplicate(&ctx, path(1), path(2), &args[3]),
        (Some("set"), n) if n >= 4 => commands::set(&ctx, path(1), path(2), &args[3..]),
        (Some("install"), 3) => commands::install(&ctx, path(1), &args[2]),
        (Some("protect"), 2) => settings_command(&args[1], |core, slot| core.protect(slot)),
        (Some("unlock"), 2) => settings_command(&args[1], |core, slot| core.unlock(slot)),
        (Some("test-slot"), 2) => settings_command(&args[1], |core, slot| core.mark_test_slot(slot)),
        (Some("slots"), 1) => slots(),
        _ => Err(Failure::Usage("bad or missing arguments".to_string())),
    }
}

/// Vault Key's core on the live save folder and settings (the same ones the app uses).
fn core() -> Result<(gkcore::Core, String), Failure> {
    let core = gkcore::CoreConfig::live().and_then(gkcore::Core::new).map_err(|e| Failure::Error(e.message))?;
    let ids = core.steam_ids().map_err(|e| Failure::Error(e.message))?;
    match ids.as_slice() {
        [one] => {
            let id = one.clone();
            Ok((core, id))
        }
        _ => Err(Failure::Error(format!("expected one Steam ID folder, found {}", ids.len()))),
    }
}

/// Changes Vault Key's settings for one slot; never touches a save file.
fn settings_command(slot: &str, change: impl FnOnce(&mut gkcore::Core, &gkcore::SlotId) -> gkcore::Result<()>) -> Result<String, Failure> {
    let number: u32 = slot.parse().map_err(|_| Failure::Usage(format!("not a slot number: {slot}")))?;
    let (mut core, id) = core()?;
    let slot_id = gkcore::SlotId::for_slot(&id, number).map_err(|e| Failure::Error(e.message))?;
    change(&mut core, &slot_id).map_err(|e| Failure::Error(e.message))?;
    Ok(format!("{}: {}\n", slot_id.file_name(), core.access(&slot_id).explanation()))
}

fn slots() -> Result<String, Failure> {
    let (mut core, _) = core()?;
    let mut out = String::new();
    for c in core.list_characters().map_err(|e| Failure::Error(e.message))? {
        let state = if c.protected { "PROTECTED" } else if c.writable { "editable" } else { "read-only" };
        out += &format!("{} {} {} lvl {} id {}: {}\n", c.file_name, c.name, c.class_label, c.level, c.save_game_id, state);
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(Failure::Usage(message)) => {
            eprintln!("gk: {message}\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Error(message)) => {
            eprintln!("gk: {message}");
            ExitCode::from(1)
        }
    }
}

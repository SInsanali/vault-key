//! Item-card tests against the real, installed game. Like `real_game.rs`, they only read the
//! install and skip (pass with a note) when none is found. Run with `--nocapture` to see the
//! computed cards and the census figures.
//!
//! The cards here are **computed, not yet compared in game** (research doc
//! `docs/research/2026-11-09-item-stats.md`), with one exception: the two Infinity pistols of
//! `published_infinity_cards` reproduce cards a player posted.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use gamedata::package::{PropertyList, PropertyValue, UPackage};
use gamedata::{
    build_database, AssetCategory, AttributeSource, DlcResolution, GameInstall, ItemDatabase,
    ItemLevel, ItemRefs, ItemStats, ObjectInfo, PresentationList, StatKind, StatRow, StatValue,
    PART_SLOTS,
};

// MARK: Support

struct Game {
    install: GameInstall,
    /// `Startup.upk`, then the DLC startup packages in name order.
    packages: Vec<UPackage>,
    db: ItemDatabase,
}

fn game() -> Option<&'static Game> {
    static GAME: OnceLock<Option<Game>> = OnceLock::new();
    let game = GAME
        .get_or_init(|| {
            let install = GameInstall::locate()?;
            let packages: Vec<UPackage> = install
                .source_packages()
                .iter()
                .map(|path| {
                    UPackage::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
                })
                .collect();
            let db = build_database(&packages, "test").expect("database from all packages");
            Some(Game {
                install,
                packages,
                db,
            })
        })
        .as_ref();
    if game.is_none() {
        eprintln!("skipped: no Borderlands 2 install found (set GK_GAME_DATA to point at one)");
    }
    game
}

const W: u32 = 131071;
const I: u32 = 65535;

fn weapon(item_type: u32, balance: u32, manufacturer: u32, parts: [u32; PART_SLOTS]) -> ItemRefs {
    ItemRefs {
        is_weapon: true,
        set: Some(0),
        item_type: Some(item_type),
        balance: Some(balance),
        manufacturer: Some(manufacturer),
        parts: parts.map(Some),
    }
}

fn item(item_type: u32, balance: u32, manufacturer: u32, parts: [u32; PART_SLOTS]) -> ItemRefs {
    ItemRefs {
        is_weapon: false,
        ..weapon(item_type, balance, manufacturer, parts)
    }
}

/// Each row as `label = text`, with `~` after an approximate row and `?` for an unknown one.
fn lines(stats: &ItemStats) -> Vec<String> {
    stats
        .rows
        .iter()
        .map(|row| {
            let label = row.label.as_deref().unwrap_or("(no label)");
            match &row.value {
                StatValue::Computed { approximate, .. } => format!(
                    "{label} = {}{}",
                    row.text().unwrap(),
                    if approximate.is_some() { " ~" } else { "" }
                ),
                StatValue::Unknown { reason } => format!("{label} ? {reason}"),
            }
        })
        .collect()
}

fn print_card(name: &str, level: u32, stats: &ItemStats) {
    println!("{name} (level {level})");
    if let Some(reason) = &stats.unavailable {
        println!("  unavailable: {reason}");
    }
    for row in &stats.rows {
        if let StatValue::Computed {
            approximate: Some(reason),
            ..
        } = &row.value
        {
            println!(
                "  note on {}: {reason}",
                row.label.as_deref().unwrap_or("?")
            );
        }
    }
    for line in lines(stats) {
        println!("  {line}");
    }
    for line in &stats.lines {
        let color = line
            .color
            .map(|c| format!(" [{}]", c.hex()))
            .unwrap_or_default();
        match (&line.text, &line.note) {
            (Some(text), None) => println!("  - {text}{color}"),
            (Some(text), Some(note)) => println!("  - {text}{color}  (note: {note})"),
            (None, reason) => println!(
                "  - ? {} ({})",
                reason.as_deref().unwrap_or(""),
                line.attribute.as_deref().map(short).unwrap_or("text")
            ),
        }
    }
}

/// The last component of an object path.
fn short(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

fn expect_card(db: &ItemDatabase, name: &str, refs: &ItemRefs, level: u32, expected: &[&str]) {
    let stats = db.stats(refs, ItemLevel::new(level));
    print_card(name, level, &stats);
    assert_eq!(stats.unavailable, None, "{name}");
    assert_eq!(lines(&stats), expected, "{name}");
}

/// A package beside `Startup.upk` in the install.
fn cooked(g: &Game, name: &str) -> UPackage {
    let path = g
        .install
        .root()
        .join("WillowGame")
        .join("CookedPCConsole")
        .join(name);
    UPackage::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn props(pkg: &UPackage, path: &str) -> PropertyList {
    let index = pkg
        .export_index(path)
        .unwrap_or_else(|| panic!("missing {path}"));
    pkg.properties_of_export(index).unwrap()
}

/// The value of a `FloatAttributeProperty` or `IntAttributeProperty` default: four raw bytes.
fn attribute_property_bytes(list: &PropertyList, name: &str) -> [u8; 4] {
    match list.get(name) {
        Some(PropertyValue::Unknown { bytes, .. }) => bytes.as_slice().try_into().unwrap(),
        other => panic!("{name} is {other:?}"),
    }
}

// MARK: Class defaults the crate hard-codes

/// The numbers in `extract::stats::weapon_defaults`: what a weapon type has when it does not
/// set a property.
#[test]
fn weapon_type_class_defaults() {
    let Some(g) = game() else { return };
    let pkg = cooked(g, "WillowGame.upk");
    let d = props(&pkg, "Default__WeaponTypeDefinition");
    assert_eq!(d.get("ClipSize"), None); // 0
    assert_eq!(d.float("FireRate"), Some(0.4));
    assert_eq!(d.float("ReloadTime"), Some(2.1));
    assert_eq!(d.float("Spread"), Some(0.01));
    assert_eq!(d.int("ProjectilesPerShot"), Some(1));
    assert_eq!(d.int("ShotCost"), Some(1));
    assert_eq!(d.get("PerShotAccuracyImpulse"), None); // 0
    assert_eq!(d.get("DefaultDamageTypeDefinition"), None);
    let constant = |name: &str| {
        d.struct_fields(name)
            .and_then(|s| s.float("BaseValueConstant"))
    };
    assert_eq!(constant("InstantHitDamage"), Some(20.0));
    assert_eq!(constant("StatusEffectDamage"), Some(1.0));
    assert_eq!(constant("BaseStatusEffectChanceModifier"), Some(1.0));
    // None of them names an attribute or a definition by default.
    for name in [
        "InstantHitDamage",
        "StatusEffectDamage",
        "BaseStatusEffectChanceModifier",
    ] {
        let fields = d.struct_fields(name).unwrap();
        assert_eq!(fields.object("BaseValueAttribute"), Some(0), "{name}");
        assert_eq!(fields.object("InitializationDefinition"), Some(0), "{name}");
        assert_eq!(fields.float("BaseValueScaleConstant"), Some(1.0), "{name}");
    }
}

/// `stats::PLAYER_BASES`: the carrier's base values that bonus lines are relative to, and
/// `stats::GRENADE_MOD_PROPERTIES`: a grenade mod's attribute properties, which have no class
/// default (they start at 0).
#[test]
fn player_and_grenade_mod_class_defaults() {
    let Some(g) = game() else { return };
    let willow = cooked(g, "WillowGame.upk");
    let engine = cooked(g, "Engine.upk");
    let controller = props(&willow, "Default__WillowPlayerController");
    for property in [
        "CurrentInstantHitCriticalHitBonus",
        "CurrentMeleeCriticalHitBonus",
    ] {
        assert_eq!(
            attribute_property_bytes(&controller, property),
            2.0f32.to_le_bytes(),
            "{property}"
        );
    }
    // Every `Instigated…Modifier` attribute property of `Controller` defaults to 1 (one that
    // defaulted to 0 would be absent from the default object, so the property list of the
    // class is compared with the defaults).
    let base_controller = props(&engine, "Default__Controller");
    let is_instigated = |name: &str| name.starts_with("Instigated") && name.ends_with("Modifier");
    let declared: Vec<String> = engine
        .exports
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            engine.class_name_of_export(*i) == "FloatAttributeProperty"
                && is_instigated(&e.object_name)
                && engine.path_of(e.outer).ok().as_deref() == Some("Controller")
        })
        .map(|(_, e)| e.object_name.clone())
        .collect();
    assert!(declared.len() >= 20, "{declared:?}");
    for property in &declared {
        assert_eq!(
            attribute_property_bytes(&base_controller, property),
            1.0f32.to_le_bytes(),
            "{property}"
        );
    }
    // The attributes read exactly those properties.
    let a = &g.db.stat_data.attributes;
    let property_of = |path: &str| match a.get(path) {
        Some(AttributeSource::Property { property, .. }) => property.clone(),
        other => panic!("{path}: {other:?}"),
    };
    assert_eq!(
        property_of("D_Attributes.GameplayAttributes.PlayerCriticalHitBonus"),
        "CurrentInstantHitCriticalHitBonus"
    );
    assert_eq!(
        property_of("D_Attributes.DamageSourceModifiers.InstigatedMeleeDamageModifier"),
        "InstigatedMeleeDamageModifier"
    );
    let grenade_mod = props(&willow, "Default__WillowGrenadeMod");
    for property in [
        "GrenadeDamage",
        "BlastRadius",
        "FuseTime",
        "NumberOfChildProjectiles",
        "StatusEffectDamage",
        "BaseStatusEffectChanceModifier",
        "StatusEffectChanceModifier",
    ] {
        assert_eq!(grenade_mod.get(property), None, "{property}");
        assert!(
            willow
                .export_index(&format!("WillowGrenadeMod.{property}"))
                .is_some(),
            "{property} is not a WillowGrenadeMod property"
        );
    }
}

/// `stats::WEAPON_STATUS_EFFECT_CHANCE_MODIFIER`: the weapon's own default, which no script
/// replaces from the weapon type.
#[test]
fn weapon_class_defaults() {
    let Some(g) = game() else { return };
    let pkg = cooked(g, "WillowGame.upk");
    let d = props(&pkg, "Default__WillowWeapon");
    let one = 1.0f32.to_le_bytes();
    assert_eq!(
        attribute_property_bytes(&d, "StatusEffectChanceModifier"),
        one
    );
    assert_eq!(
        attribute_property_bytes(&d, "StatusEffectChanceModifierBaseValue"),
        one
    );
}

/// `extract::stats::presentation_defaults`, and the slot limit in `stats`.
#[test]
fn presentation_class_defaults() {
    let Some(g) = game() else { return };
    let pkg = cooked(g, "WillowGame.upk");
    let d = props(&pkg, "Default__AttributePresentationDefinition");
    assert_eq!(d.enum_name("RoundingMode"), Some("ATTRROUNDING_IntRound"));
    assert_eq!(d.int("FloatPrecision"), Some(1));
    assert_eq!(d.get("bDisplayAsInverse"), None);
    assert_eq!(d.get("bValueRemappingEnabled"), None);
    let remap = d.struct_fields("RemappingData").unwrap();
    for bound in [
        "InputValueMn",
        "InputValueMx",
        "OutputValueMn",
        "OutputValueMx",
    ] {
        let fields = remap.struct_fields(bound).unwrap();
        assert_eq!(fields.float("BaseValueConstant"), Some(0.0), "{bound}");
        assert_eq!(fields.float("BaseValueScaleConstant"), Some(1.0), "{bound}");
    }

    let engine = cooked(g, "Engine.upk");
    let inventory = props(&engine, "Default__WillowInventoryDefinition");
    assert_eq!(inventory.int("AttributeSlotMaxActivated"), Some(19));
    // The formula the game's own data states for initialisation definitions.
    let init = props(&engine, "Default__AttributeInitializationDefinition");
    assert_eq!(
        init.struct_fields("ValueFormula")
            .and_then(|f| f.string("Formula")),
        Some("Multiplier * ( Level^Power + Offset )")
    );
    assert_eq!(init.get("RoundingMode"), None); // ATTRROUNDING_Float
    assert_eq!(init.get("BaseValueMode"), None); // the definition sets the value
}

// MARK: What was extracted

#[test]
fn stat_data_shape() {
    let Some(g) = game() else { return };
    let s = &g.db.stat_data;
    let weapon_types = s
        .definitions
        .values()
        .filter(|d| d.weapon.is_some())
        .count();
    let with_problems: Vec<&String> = s
        .definitions
        .iter()
        .filter(|(_, d)| !d.problems.is_empty())
        .map(|(path, _)| path)
        .collect();
    let unsupported = s
        .attributes
        .values()
        .filter(|a| matches!(a, AttributeSource::Unsupported(_)))
        .count();
    println!(
        "definitions {} (weapon types {weapon_types}), attributes {} ({unsupported} unsupported), \
         initialisation definitions {} ({} unsupported)",
        s.definitions.len(),
        s.attributes.len(),
        s.initializations.len(),
        s.initializations
            .values()
            .filter(|i| i.unsupported.is_some())
            .count(),
    );
    for (list, entries) in &s.presentations {
        println!("  presentation list {list:?}: {} entries", entries.len());
    }
    assert!(with_problems.is_empty(), "{with_problems:?}");
    // Every located base-game weapon type (33) and both located DLC ones.
    assert_eq!(weapon_types, 35);
    let lists: Vec<PresentationList> = s.presentations.iter().map(|(l, _)| *l).collect();
    assert_eq!(lists, PresentationList::ALL);

    // The values the calculation rests on.
    assert_eq!(
        s.attributes
            .get("GD_Balance_HealthAndDamage.HealthAndDamage.Att_UniversalBalanceScaler"),
        Some(&AttributeSource::Constant(1.13))
    );
    let damage = &s.initializations["GD_Balance_HealthAndDamage.HealthAndDamage.Init_WeaponDamage"];
    let formula = damage.formula.as_ref().unwrap();
    assert_eq!(formula.multiplier.constant, 8.0);
    assert_eq!(
        formula.power.attribute.as_deref(),
        Some("D_Attributes.Weapon.WeaponLevel")
    );
    // Its minimum of 5 is stated but not enabled.
    assert_eq!((&damage.minimum, &damage.unsupported), (&None, &None));
    assert_eq!(
        s.attributes.get("D_Attributes.Weapon.WeaponLevel"),
        Some(&AttributeSource::Property {
            context: "WeaponAttributeContextResolver".into(),
            resource: None,
            property: "ExpLevel".into(),
        })
    );

    // Every weapon type shows the same seven rows, in this order.
    let rows: BTreeSet<Vec<&str>> = s
        .definitions
        .values()
        .filter(|d| d.weapon.is_some())
        .map(|d| {
            d.ui_stats
                .iter()
                .map(|stat| {
                    stat.attribute
                        .as_deref()
                        .and_then(|a| a.rsplit('.').next())
                        .unwrap_or("-")
                })
                .collect()
        })
        .collect();
    assert_eq!(
        rows.into_iter().collect::<Vec<_>>(),
        [[
            "WeaponDamage",
            "WeaponSpread",
            "WeaponFireInterval",
            "WeaponReloadSpeed",
            "WeaponClipSize",
            "WeaponStatusEffectDamage",
            "WeaponCombinedStatusEffectChanceModifier"
        ]]
    );

    // The stat data survives the cache encoding.
    let back = ItemDatabase::from_bytes(&g.db.to_bytes()).unwrap();
    assert_eq!(back.stat_data, g.db.stat_data);
}

/// Every attribute a base-game or DLC definition modifies is an attribute definition in the
/// packages, and every attribute or initialisation definition used as a value was found.
#[test]
fn referenced_attributes_resolve() {
    let Some(g) = game() else { return };
    let s = &g.db.stat_data;
    let class_of = |path: &str| -> Option<&str> {
        g.packages.iter().find_map(|pkg| {
            pkg.export_index(path)
                .map(|index| pkg.class_name_of_export(index))
        })
    };
    let mut modified = BTreeSet::new();
    let mut values = BTreeSet::new();
    let mut initializations = BTreeSet::new();
    let mut note = |data: &gamedata::ValueData| {
        values.extend(data.attribute.clone());
        initializations.extend(data.initialization.clone());
    };
    for definition in s.definitions.values() {
        for effect in definition
            .effects
            .iter()
            .chain(&definition.external_effects)
        {
            modified.insert(effect.attribute.as_str());
            note(&effect.value);
        }
        for slot in &definition.slot_effects {
            modified.insert(slot.attribute.as_str());
            note(&slot.base);
            note(&slot.per_grade);
        }
        if let Some(base) = &definition.weapon {
            note(&base.damage);
            note(&base.status_effect_damage);
            note(&base.base_status_effect_chance);
        }
    }
    let not_attributes: Vec<&str> = modified
        .iter()
        .copied()
        .filter(|path| !class_of(path).is_some_and(|c| c.ends_with("AttributeDefinition")))
        .collect();
    let missing_values: Vec<&String> = values
        .iter()
        .filter(|path| !s.attributes.contains_key(*path))
        .collect();
    let missing_initializations: Vec<&String> = initializations
        .iter()
        .filter(|path| !s.initializations.contains_key(*path))
        .collect();
    println!(
        "modified attributes {} (not found {}), value attributes {} (not found {}), \
         initialisation definitions {} (not found {})",
        modified.len(),
        not_attributes.len(),
        values.len(),
        missing_values.len(),
        initializations.len(),
        missing_initializations.len()
    );
    assert!(not_attributes.is_empty(), "{not_attributes:?}");
    assert!(missing_values.is_empty(), "{missing_values:?}");
    assert!(
        missing_initializations.is_empty(),
        "{missing_initializations:?}"
    );
}

/// The level a weapon's formulas use is taken to be its grade index. The balance definition
/// could add to it through its grade's `ExpLevel`; no base-game weapon balance does.
#[test]
fn weapon_balance_grades_add_no_level() {
    let Some(g) = game() else { return };
    let pkg = &g.packages[0];
    let mut balances = 0;
    let mut grades_seen = 0;
    for &export in pkg.exports_of_class("WeaponBalanceDefinition") {
        let list = pkg.properties_of_export(export).unwrap();
        let Some(manufacturers) = list.get("Manufacturers") else {
            continue;
        };
        balances += 1;
        for manufacturer in pkg.struct_array(manufacturers).unwrap() {
            let Some(grades) = manufacturer.get("Grades") else {
                continue;
            };
            for grade in pkg.struct_array(grades).unwrap() {
                grades_seen += 1;
                let level = grade
                    .struct_fields("GradeModifiers")
                    .and_then(|m| m.int("ExpLevel"));
                assert_eq!(level, Some(0), "{}", pkg.export_path(export).unwrap());
            }
        }
    }
    println!("{balances} weapon balances with manufacturers, {grades_seen} grades, all ExpLevel 0");
    assert!(balances > 30 && grades_seen >= balances);
}

// MARK: Cards

/// The three weapons of the fixture save `win-throwaway-Save0001.sav` (numbers decoded with
/// `savekit`). **Computed, not yet compared in game:** these are the cards to check there.
#[test]
fn fixture_weapon_cards() {
    let Some(g) = game() else { return };
    let db = &g.db;
    println!("--- fixture weapons: compare with the in-game cards ---");
    expect_card(
        db,
        "Gearbox Rifle",
        &weapon(
            1668,
            129043,
            133,
            [6316, 6275, 6145, 6151, 6281, W, W, W, 6317, 6320, 6292],
        ),
        1,
        &[
            "Damage = 13",
            "Accuracy = 84.7",
            "Fire Rate = 6.7",
            "Reload Speed = 3.7",
            "Magazine Size = 20",
        ],
    );
    expect_card(
        db,
        "Gearbox Projectile Convergence",
        &weapon(
            2114,
            133132,
            134,
            [
                22690, 22564, 22529, 22534, 22569, W, W, W, 22691, 22692, 22674,
            ],
        ),
        1,
        &[
            "Damage = 10",
            "Accuracy = 92.3",
            "Fire Rate = 10.4",
            "Reload Speed = 2.7",
            // 24 × 1.25 ÷ 1.2 is exactly 25 and so lies on the rounding boundary. A posted card
            // of this gun with the same grip shows 25 (`posted_cards`).
            "Magazine Size = 25 ~",
        ],
    );
    expect_card(
        db,
        "Gearbox Callipeen",
        &weapon(
            2180,
            134159,
            131,
            [
                24723, 24586, 24590, 24622, 24599, W, W, W, 24724, 24729, 24635,
            ],
        ),
        1,
        &[
            "Damage = 37",
            "Accuracy = 94.4",
            "Fire Rate = 0.8",
            "Reload Speed = 5.0",
            "Magazine Size = 6",
        ],
    );
    // The fixture's grenade mod: its type lists no damage row ("Damage is based on player
    // level"), so the card has two rows. Its grade index is 0.
    let grenade = item(
        3851,
        29735,
        129,
        [I, 3140, 3074, 3080, 3095, 3097, I, 3102, 3106, I, 3204],
    );
    let stats = db.stats(&grenade, ItemLevel::new(0));
    print_card("Contraband Sky Rocket", 0, &stats);
    assert_eq!(lines(&stats), ["Blast Radius = 550", "Fuse Time = 1.6"]);
    assert_eq!(
        line_texts(&stats),
        [
            "WARNING: Not intended for indoor use.",
            "Damage is based on player level."
        ]
    );
    // The fixture's relic: its one row is an attribute with no value resolver.
    let relic = item(2840, 27695, 137, [0, 1, I, I, I, I, 52, 14, I, I, I]);
    let stats = db.stats(&relic, ItemLevel::new(1));
    print_card("Vault Hunter's Relic", 1, &stats);
    assert_eq!(stats.unavailable, None);
    assert_eq!(stats.rows.len(), 1);
    assert!(matches!(stats.rows[0].value, StatValue::Unknown { .. }));
    assert_eq!(
        line_texts(&stats),
        ["Increased chance of enemies dropping rare loot."]
    );
}

/// Items of the owner's main character (`mac-main-Save0001.sav`, read with `savekit`): the
/// grenade mods, class mods and a shield with a special row. **Computed, not compared.**
#[test]
fn maya_fixture_cards() {
    let Some(g) = game() else { return };
    let db = &g.db;
    println!("--- items of the main character ---");
    let card = |name: &str, refs: ItemRefs, level: u32| {
        let stats = db.stats(&refs, ItemLevel::new(level));
        print_card(name, level, &stats);
        assert_eq!(stats.unavailable, None, "{name}");
        (lines(&stats), line_texts(&stats))
    };
    let (rows, texts) = card(
        "Unyielding Banshee Class Mod",
        item(
            3341,
            118975,
            129,
            [1062, 1032, 1028, I, I, I, I, I, 1039, 1137, 1024],
        ),
        12,
    );
    assert_eq!(rows[0], "Shield Recharge Rate = +26%");
    assert!(rows[1].starts_with("Shield Recharge Delay = -19.35% ~"));
    assert_eq!(rows[2], "Max Health = -75");
    assert_eq!(texts, ["+2 Immolate Skill"]);
    let (rows, texts) = card(
        "Relentless Matriarch Class Mod",
        item(
            3338,
            118975,
            144,
            [1056, 1036, 1029, I, I, I, I, I, 1040, 1119, 1024],
        ),
        12,
    );
    assert_eq!(rows, ["Max Health = +69", "Magazine Size = +16.8%"]);
    assert_eq!(texts, ["+2 Sustenance Skill"]);
    let (rows, texts) = card(
        "Bullet Witch Class Mod",
        item(
            3335,
            118975,
            136,
            [1050, 1035, 1026, I, I, I, I, I, 1042, 1102, 1024],
        ),
        12,
    );
    assert_eq!(rows, ["Corrode Chance = +18.4%", "Corrode Damage = +21%"]);
    assert_eq!(texts, ["+2 Kinetic Reflection Skill"]);
    let (rows, texts) = card(
        "Warder Class Mod",
        item(
            3336,
            118976,
            143,
            [1043, 1035, 1026, I, I, I, I, I, 1040, 1109, 1024],
        ),
        8,
    );
    assert_eq!(rows, ["Shield Capacity = +57"]);
    assert!(texts.is_empty());
    let (rows, texts) = card(
        "Lobbed MIRV",
        item(
            3841,
            29719,
            132,
            [
                3176, 3073, 3074, 3078, 3128, 3098, 3119, 3189, 3168, 3107, 3229,
            ],
        ),
        12,
    );
    assert_eq!(
        rows,
        [
            "Grenade Damage = 469 x8",
            "Blast Radius = 412",
            "Fuse Time = 0.6"
        ]
    );
    assert_eq!(
        texts,
        [
            "Spawns 8 child grenades upon detonating.",
            "Breaks into a number of smaller child grenades that explode on impact."
        ]
    );
    let (rows, texts) = card(
        "Explosive Fastball",
        item(
            3844,
            29726,
            135,
            [I, 3138, 3190, 3078, 3128, 3129, I, 3103, 3164, 3111, 3259],
        ),
        8,
    );
    assert_eq!(
        rows,
        [
            "Grenade Damage = 681",
            "Blast Radius = 85",
            "Fuse Time = 0.6"
        ]
    );
    assert_eq!(texts, ["Forget the curveball Ricky, give him the heater."]);
    let (rows, texts) = card(
        "Snapping Turtle Shield",
        item(
            4359,
            118838,
            144,
            [4098, 4112, 4127, 4210, I, I, I, I, 4207, 4343, 4330],
        ),
        8,
    );
    assert_eq!(
        rows,
        [
            "Capacity = 272",
            "Recharge Rate = 31",
            "Recharge Delay = 2.30",
            "Max Health = -93"
        ]
    );
    assert_eq!(texts, ["High capacity, reduces max health."]);
}

/// The text of each line that has one, in card order; `?` for one that has none.
fn line_texts(stats: &ItemStats) -> Vec<String> {
    stats
        .lines
        .iter()
        .map(|line| line.text.clone().unwrap_or_else(|| "?".into()))
        .collect()
}

/// The format doc's weapon vector and other items whose decoded numbers `real_game.rs` holds.
#[test]
fn known_item_cards() {
    let Some(g) = game() else { return };
    let db = &g.db;
    println!("--- known items ---");
    let infinity = weapon(
        1924,
        131111,
        133,
        [
            12538, 12466, 12552, 12301, W, 12306, 12313, W, 12554, 12352, 12568,
        ],
    );
    expect_card(
        db,
        "Righteous Infinity",
        &infinity,
        50,
        &[
            "Damage = 8523",
            "Accuracy = 92.6",
            "Fire Rate = 8.0",
            "Reload Speed = 2.2",
            "Magazine Size = 1",
        ],
    );
    let stats = db.stats(&infinity, ItemLevel::new(50));
    assert_eq!(stats.texts.len(), 1);
    assert_eq!(
        stats.texts[0].text,
        "It's closer than you think! (no it isn't)"
    );
    assert_eq!(
        stats.texts[0].color.map(|c| c.hex()).as_deref(),
        Some("#DC4646")
    );
    assert_eq!(
        stats.row(StatKind::Damage).map(StatRow::is_exact),
        Some(true)
    );
    assert_eq!(
        stats
            .row(StatKind::MagazineSize)
            .and_then(StatRow::text)
            .as_deref(),
        Some("1")
    );
    // Without a level only the damage is unknown.
    let no_level = lines(&db.stats(&infinity, ItemLevel::default()));
    assert_eq!(no_level[0], "Damage ? the item's level is not known");
    assert_eq!(no_level[1], "Accuracy = 92.6");

    expect_card(
        db,
        "Basic Repeater",
        &weapon(
            1921,
            131094,
            130,
            [12468, 12460, 12509, 12297, W, W, W, W, 12510, 12511, 12483],
        ),
        1,
        &[
            "Damage = 11",
            "Accuracy = 89.3",
            "Fire Rate = 12.5",
            "Reload Speed = 1.5",
            "Magazine Size = 7",
        ],
    );
    // An incendiary pistol: two more rows.
    expect_card(
        db,
        "Flynt's Tinderbox",
        &weapon(
            1920,
            131123,
            129,
            [
                12512, 12459, 12608, 12297, W, 12307, 12311, W, 12602, 12594, 12611,
            ],
        ),
        4,
        &[
            "Damage = 19",
            "Accuracy = 82.4",
            "Fire Rate = 3.3",
            "Reload Speed = 2.5",
            "Magazine Size = 57",
            "Burn Damage / sec. = 13.0",
            "Ignite Chance = 24.0%",
        ],
    );
    expect_card(
        db,
        "Skookum Diaub",
        &weapon(
            2180,
            134156,
            131,
            [
                24680, 24587, 24594, 24623, 24598, W, 24676, W, 24681, 24718, 24602,
            ],
        ),
        5,
        &[
            "Damage = 91",
            "Accuracy = 94.4",
            "Fire Rate = 4.0",
            "Reload Speed = 4.0",
            "Magazine Size = 11",
        ],
    );
    expect_card(
        db,
        "Quick Shield",
        &item(
            4360,
            118794,
            135,
            [4098, 4112, 4121, 4237, I, I, I, I, 4185, 4372, 4184],
        ),
        5,
        &[
            "Capacity = 121",
            "Recharge Rate = 23",
            "Recharge Delay = 2.24",
        ],
    );
    expect_card(
        db,
        "Cut Rate Shield",
        &item(
            4360,
            118811,
            135,
            [4104, 4115, 4122, 4237, I, I, I, I, 4234, 4365, 4184],
        ),
        5,
        &[
            "Capacity = 132",
            "Recharge Rate = 15",
            "Recharge Delay = 3.64",
        ],
    );
}

/// Two cards a player posted (Steam discussion 558753804482920402, April 2014, when the level
/// cap with Overpower levels was 80): a Vengeful Infinity with damage 277805, accuracy 89.6
/// and fire rate 10.4, and a Dva Infinity with 282918, 89.6 and 9.0, both elemental. A
/// level-80 Infinity with a Vladof grip and any elemental part reproduces all six numbers.
///
/// This is the one comparison with the running game so far. It exercises the level formula up
/// to level 80, slot grades, manufacturer-matched part effects, a negative scale modifier beside
/// positive ones (netting them would give 292462 and 298598), the accuracy remapping, the fire
/// rate inversion and both rounding modes.
#[test]
fn published_infinity_cards() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let part = |path: &str| {
        db.value_of(path, AssetCategory::WeaponParts, 0)
            .unwrap_or_else(|| panic!("{path} is not in the base tables"))
    };
    let infinity = |accessory: &str, element: &str| {
        weapon(
            1924,
            131111,
            133,
            [
                part("GD_Weap_Pistol.Body.Pistol_Body_Vladof_4"),
                part("GD_Weap_Pistol.Grip.Pistol_Grip_Vladof"),
                part("GD_Weap_Pistol.Barrel.Pistol_Barrel_Vladof_Infinity"),
                part("GD_Weap_Pistol.Sight.Pistol_Sight_None"),
                W,
                part(&format!(
                    "GD_Weap_Pistol.elemental.Pistol_Elemental_{element}"
                )),
                part(&format!(
                    "GD_Weap_Pistol.Accessory.Pistol_Accessory_{accessory}"
                )),
                W,
                part("GD_Weap_Pistol.ManufacturerMaterials.Mat_Vladof_5_Legendary"),
                W,
                part("GD_Weap_Pistol.Name.Title_Vladof.Title_Legendary_Infinity"),
            ],
        )
    };
    let top = |stats: &ItemStats| -> Vec<String> {
        [StatKind::Damage, StatKind::Accuracy, StatKind::FireRate]
            .map(|kind| stats.row(kind).and_then(StatRow::text).unwrap_or_default())
            .to_vec()
    };
    println!("--- published cards ---");
    for element in ["Fire", "Corrosive", "Shock", "Slag"] {
        let vengeful = db.stats(&infinity("Tech_3_Firerate", element), ItemLevel::new(80));
        let dva = db.stats(
            &infinity("Laser_Double_DvaInfinity", element),
            ItemLevel::new(80),
        );
        if element == "Fire" {
            print_card("Vengeful Infinity, incendiary", 80, &vengeful);
            print_card("Dva Infinity, incendiary", 80, &dva);
        }
        assert_eq!(top(&vengeful), ["277805", "89.6", "10.4"], "{element}");
        assert_eq!(top(&dva), ["282918", "89.6", "9.0"], "{element}");
    }
    // The non-elemental gun does not carry the element's −20% damage: 277805 × 1.2.
    let plain = db.stats(&infinity("Tech_3_Firerate", "None"), ItemLevel::new(80));
    assert_eq!(top(&plain), ["333366", "89.6", "10.4"]);
    // Other levels and grips do not reproduce the cards.
    let other_level = db.stats(&infinity("Tech_3_Firerate", "Fire"), ItemLevel::new(72));
    assert_ne!(top(&other_level)[0], "277805");
}

/// Four cards from two screenshots other players posted (research doc §7.3). The name of each
/// gun fixes or narrows its parts; an inverse search over what each balance allows found the
/// rest, and the level requirement is the item's level.
///
/// - A Gearbox Projectile Convergence with a Bandit grip and a Maliwan barrel: every row. Its
///   magazine, 24 × 1.25 ÷ 1.2, is the boundary case that pure 32-bit arithmetic gets as 24.
/// - "Inflammatory Phobia" (Maliwan pistol, Hyperion barrel, fire): every row, among them the
///   first outside check of the two elemental rows and of the "%" after the chance.
/// - "Grim TMP" (Vladof pistol, Torgue grip): every row.
/// - "redy Magamum!" (Bandit pistol, Tediore grip, Torgue barrel), level 30: damage 525 is
///   524.34 rounded up; reload 3.2 is 3.0 × 1.05, another boundary case. Its magazine cannot be
///   read from the screenshot and is not asserted.
#[test]
fn posted_cards() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let value = |path: &str, category: AssetCategory| {
        db.value_of(path, category, 0)
            .unwrap_or_else(|| panic!("{path} is not in the base tables"))
    };
    let gun = |weapon_type: &str, maker: &str, parts: [&str; PART_SLOTS]| {
        weapon(
            value(weapon_type, AssetCategory::WeaponTypes),
            0,
            value(
                &format!("GD_Manufacturers.Manufacturers.{maker}"),
                AssetCategory::Manufacturers,
            ),
            parts.map(|path| {
                if path.is_empty() {
                    W
                } else {
                    value(path, AssetCategory::WeaponParts)
                }
            }),
        )
    };
    println!("--- posted cards ---");
    expect_card(
        db,
        "Gearbox Projectile Convergence (Bandit grip, Maliwan barrel)",
        &gun(
            "GD_Weap_SMG.A_Weapons.WT_SMG_Hyperion",
            "Hyperion",
            [
                "GD_Weap_SMG.Body.SMG_Body_Gearbox_1",
                "GD_Weap_SMG.Grip.SMG_Grip_Bandit",
                "GD_Weap_SMG.Barrel.SMG_Barrel_Maliwan",
                "GD_Weap_SMG.Sight.SMG_Sight_None",
                "GD_Weap_SMG.Stock.SMG_Stock_Bandit",
                "",
                "",
                "",
                "GD_Weap_SMG.ManufacturerMaterials.Mat_Gearbox_1",
                "GD_Weap_SMG.Name.Prefix.Prefix__Gearbox",
                "GD_Weap_SMG.Name.Title_Hyperion.Title__SMG",
            ],
        ),
        1,
        &[
            "Damage = 10",
            "Accuracy = 93.0",
            "Fire Rate = 10.4",
            "Reload Speed = 3.0",
            "Magazine Size = 25 ~",
        ],
    );
    expect_card(
        db,
        "Inflammatory Phobia",
        &gun(
            "GD_Weap_Pistol.A_Weapons.WeaponType_Maliwan_Pistol",
            "Maliwan",
            [
                "GD_Weap_Pistol.Body.Pistol_Body_Maliwan_2",
                "GD_Weap_Pistol.Grip.Pistol_Grip_Dahl",
                "GD_Weap_Pistol.Barrel.Pistol_Barrel_Hyperion",
                "GD_Weap_Pistol.Sight.Pistol_Sight_None",
                "",
                "GD_Weap_Pistol.elemental.Pistol_Elemental_Fire",
                "GD_Weap_Pistol.Accessory.Pistol_Accessory_None",
                "",
                "GD_Weap_Pistol.ManufacturerMaterials.Mat_Maliwan_2",
                "GD_Weap_Pistol.Name.Prefix_Maliwan.Prefix_Elemental_Incendiary",
                "GD_Weap_Pistol.Name.Title_Maliwan.Title_Barrel_Hyperion_Accurate",
            ],
        ),
        4,
        &[
            "Damage = 21",
            "Accuracy = 93.3",
            "Fire Rate = 2.5",
            "Reload Speed = 2.1",
            "Magazine Size = 6",
            "Burn Damage / sec. = 15.7",
            "Ignite Chance = 15.6%",
        ],
    );
    // The card's five lines below the rows, in its order (screenshot b.jpg).
    let phobia = db.stats(
        &gun(
            "GD_Weap_Pistol.A_Weapons.WeaponType_Maliwan_Pistol",
            "Maliwan",
            [
                "GD_Weap_Pistol.Body.Pistol_Body_Maliwan_2",
                "GD_Weap_Pistol.Grip.Pistol_Grip_Dahl",
                "GD_Weap_Pistol.Barrel.Pistol_Barrel_Hyperion",
                "GD_Weap_Pistol.Sight.Pistol_Sight_None",
                "",
                "GD_Weap_Pistol.elemental.Pistol_Elemental_Fire",
                "GD_Weap_Pistol.Accessory.Pistol_Accessory_None",
                "",
                "GD_Weap_Pistol.ManufacturerMaterials.Mat_Maliwan_2",
                "GD_Weap_Pistol.Name.Prefix_Maliwan.Prefix_Elemental_Incendiary",
                "GD_Weap_Pistol.Name.Title_Maliwan.Title_Barrel_Hyperion_Accurate",
            ],
        ),
        ItemLevel::new(4),
    );
    assert_eq!(
        line_texts(&phobia),
        [
            "High elemental effect chance.",
            "+8% Critical Hit Damage",
            "Consumes 2 ammo per shot.",
            "Deals bonus elemental damage.",
            "Highly effective vs Flesh.",
        ]
    );
    assert!(phobia.lines.iter().all(|line| line.note.is_none()));
    expect_card(
        db,
        "Grim TMP",
        &gun(
            "GD_Weap_Pistol.A_Weapons.WeaponType_Vladof_Pistol",
            "Vladof",
            [
                "GD_Weap_Pistol.Body.Pistol_Body_Vladof",
                "GD_Weap_Pistol.Grip.Pistol_Grip_Torgue",
                "GD_Weap_Pistol.Barrel.Pistol_Barrel_Dahl",
                "GD_Weap_Pistol.Sight.Pistol_Sight_None",
                "",
                "",
                "",
                "",
                "GD_Weap_Pistol.ManufacturerMaterials.Mat_Vladof_1",
                "GD_Weap_Shared_Names.Name.Prefix_Vladof.Prefix_Grip_Torgue",
                "GD_Weap_Pistol.Name.Title_Vladof.Title__TMP",
            ],
        ),
        2,
        &[
            "Damage = 15",
            "Accuracy = 81.8",
            "Fire Rate = 8.0",
            "Reload Speed = 2.4",
            "Magazine Size = 20",
        ],
    );
    let magamum = gun(
        "GD_Weap_Pistol.A_Weapons.WeaponType_Bandit_Pistol",
        "Bandit",
        [
            "GD_Weap_Pistol.Body.Pistol_Body_Bandit",
            "GD_Weap_Pistol.Grip.Pistol_Grip_Tediore",
            "GD_Weap_Pistol.Barrel.Pistol_Barrel_Torgue",
            "GD_Weap_Pistol.Sight.Pistol_Sight_None",
            "",
            "",
            "",
            "",
            "GD_Weap_Pistol.ManufacturerMaterials.Mat_Bandit_1",
            "GD_Weap_Shared_Names.Name.Prefix_Bandit.Prefix_Grip_Tediore",
            "GD_Weap_Pistol.Name.Title_Bandit.Title_Barrel_Torgue_InaccurateDamage",
        ],
    );
    let stats = db.stats(&magamum, ItemLevel::new(30));
    print_card("redy Magamum!", 30, &stats);
    assert_eq!(
        lines(&stats)[..4],
        [
            "Damage = 525",
            "Accuracy = 84.0",
            "Fire Rate = 4.0",
            "Reload Speed = 3.2 ~"
        ]
    );
    // The names the parts carry are the ones on the cards.
    let named = |refs: &ItemRefs| db.info(refs).name;
    assert_eq!(named(&magamum).as_deref(), Some("redy Magamum!"));
}

// MARK: Census

/// Every weapon type with every weapon part of the same set, one part at a time, at level 30:
/// every row is computed. (The pairings are not all ones the game would roll; the point is
/// that nothing a part or type refers to is beyond the calculation.) The only unknown rows
/// allowed are those of parts whose definitions no startup package holds.
#[test]
fn every_weapon_part_evaluates() {
    let Some(g) = game() else { return };
    let db = &g.db;
    for (&set_id, set) in &db.sets {
        // The weapon types an item of this set can name: the base game's and the set's own.
        let mut types: Vec<u32> = Vec::new();
        for (source, tables) in [(0, &db.sets[&0]), (set_id, set)] {
            for path in tables.paths(AssetCategory::WeaponTypes) {
                if matches!(db.object(path), Some(ObjectInfo::WeaponType { .. })) {
                    types.extend(db.value_of(path, AssetCategory::WeaponTypes, source));
                }
            }
            if set_id == 0 {
                break;
            }
        }
        types.sort_unstable();
        types.dedup();
        let parts: Vec<(u32, bool)> = set
            .paths(AssetCategory::WeaponParts)
            .filter_map(|path| {
                let value = db.value_of(path, AssetCategory::WeaponParts, set_id)?;
                Some((value, db.stat_data.definitions.contains_key(path)))
            })
            .collect();
        let (mut cards, mut exact, mut approximate) = (0, 0, 0);
        let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
        for &item_type in &types {
            for &(part, located) in &parts {
                let mut slots = [Some(W); PART_SLOTS];
                slots[0] = Some(part);
                let refs = ItemRefs {
                    is_weapon: true,
                    set: Some(set_id),
                    item_type: Some(item_type),
                    balance: None,
                    manufacturer: Some(133),
                    parts: slots,
                };
                let stats = db.stats_with(&refs, ItemLevel::new(30), DlcResolution::Verified);
                assert_eq!(stats.unavailable, None);
                assert!(stats.rows.len() >= 5);
                cards += 1;
                for row in &stats.rows {
                    match &row.value {
                        StatValue::Unknown { reason } => {
                            assert!(!located || set_id != 0, "set 0 part {part}: {reason}");
                            *unknown.entry(reason.clone()).or_default() += 1;
                        }
                        StatValue::Computed {
                            approximate: Some(_),
                            ..
                        } => approximate += 1,
                        StatValue::Computed { .. } => exact += 1,
                    }
                }
            }
        }
        if cards > 0 {
            println!(
                "set {set_id}: {} types × {} parts = {cards} cards; rows: {exact} exact, \
                 {approximate} on a rounding boundary, unknown {unknown:?}",
                types.len(),
                parts.len(),
            );
        }
        if set_id == 0 {
            assert_eq!(types.len(), 33);
            // Unknown rows come only from the table entries with no definition.
            assert_eq!(unknown.len(), 1, "{unknown:?}");
            assert!(unknown
                .keys()
                .all(|reason| reason.contains("not in the database")));
        }
    }
}

/// Every grenade mod, class mod and relic type of the base game with every base item part, one
/// part at a time, and every base weapon type with every base weapon part: how many rows and
/// lines compute, and why the rest do not. Printed for the record; the asserts only pin that
/// every card is produced and that the grenade rows compute.
#[test]
fn item_kinds_and_lines_census() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let base = &db.sets[&0];
    let census = |is_weapon: bool, class: &str, manufacturer: u32| {
        let (types_category, parts_category) = (
            AssetCategory::types(is_weapon),
            AssetCategory::parts(is_weapon),
        );
        let types: Vec<u32> = base
            .paths(types_category)
            .filter(|path| match db.object(path) {
                Some(ObjectInfo::WeaponType { .. }) => is_weapon,
                Some(ObjectInfo::ItemType { class_name, .. }) => class_name == class,
                _ => false,
            })
            .filter_map(|path| db.value_of(path, types_category, 0))
            .collect();
        let parts: Vec<u32> = base
            .paths(parts_category)
            .filter(|path| db.stat_data.definitions.contains_key(*path))
            .filter_map(|path| db.value_of(path, parts_category, 0))
            .collect();
        let empty = parts_category.all_ones();
        let mut rows_ok = 0;
        let mut rows_unknown: BTreeMap<String, usize> = BTreeMap::new();
        let mut lines_ok = 0;
        let mut lines_noted = 0;
        let mut lines_unknown: BTreeMap<String, usize> = BTreeMap::new();
        for &item_type in &types {
            for &part in &parts {
                let mut slots = [Some(empty); PART_SLOTS];
                slots[0] = Some(part);
                let refs = ItemRefs {
                    is_weapon,
                    set: Some(0),
                    item_type: Some(item_type),
                    balance: None,
                    manufacturer: Some(manufacturer),
                    parts: slots,
                };
                let stats = db.stats(&refs, ItemLevel::new(30));
                assert_eq!(stats.unavailable, None, "{class}");
                for row in &stats.rows {
                    match &row.value {
                        StatValue::Computed { .. } => rows_ok += 1,
                        StatValue::Unknown { reason } => {
                            let label = row.label.as_deref().unwrap_or("-");
                            *rows_unknown
                                .entry(format!("{label}: {reason}"))
                                .or_default() += 1;
                        }
                    }
                }
                for line in &stats.lines {
                    match (&line.text, &line.note) {
                        (Some(_), None) => lines_ok += 1,
                        (Some(_), Some(_)) => lines_noted += 1,
                        (None, note) => {
                            *lines_unknown
                                .entry(note.clone().unwrap_or_default())
                                .or_default() += 1;
                        }
                    }
                }
            }
        }
        println!(
            "{class}: {} types × {} parts; rows {rows_ok} computed, {} unknown; lines {lines_ok} \
             computed, {lines_noted} with a note, {} without text",
            types.len(),
            parts.len(),
            rows_unknown.values().sum::<usize>(),
            lines_unknown.values().sum::<usize>(),
        );
        for (what, n) in rows_unknown.iter().take(12) {
            println!("  unknown row ({n}): {what}");
        }
        for (what, n) in lines_unknown.iter().take(12) {
            println!("  line without text ({n}): {what}");
        }
        (types.len(), rows_ok, rows_unknown)
    };
    let (grenade_types, grenade_rows, grenade_unknown) = census(false, "GrenadeModDefinition", 129);
    assert!(grenade_types >= 12 && grenade_rows > 10_000);
    // Grenade rows are damage, blast radius and fuse time; all compute.
    assert!(grenade_unknown.is_empty(), "{grenade_unknown:?}");
    let (class_mod_types, _, _) = census(false, "ClassModDefinition", 129);
    assert!(class_mod_types >= 40);
    let (relic_types, _, _) = census(false, "ArtifactDefinition", 137);
    assert!(relic_types >= 20);
    census(true, "", 133);
}

/// Every shield type with every item part of the base game: capacity, recharge rate and
/// recharge delay are always computed. A shield's fourth row (its special effect) is shown as
/// modifier text, which is not modelled, and stays unknown.
#[test]
fn every_shield_part_evaluates() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let base = &db.sets[&0];
    let types: Vec<u32> = base
        .paths(AssetCategory::ItemTypes)
        .filter(|path| {
            matches!(
                db.object(path),
                Some(ObjectInfo::ItemType { class_name, .. }) if class_name == "ShieldDefinition"
            )
        })
        .filter_map(|path| db.value_of(path, AssetCategory::ItemTypes, 0))
        .collect();
    let parts: Vec<u32> = base
        .paths(AssetCategory::ItemParts)
        .filter(|path| db.stat_data.definitions.contains_key(*path))
        .filter_map(|path| db.value_of(path, AssetCategory::ItemParts, 0))
        .collect();
    let mut cards = 0;
    let mut other_unknown: BTreeMap<String, usize> = BTreeMap::new();
    for &item_type in &types {
        for &part in &parts {
            let mut slots = [Some(I); PART_SLOTS];
            slots[0] = Some(part);
            let refs = ItemRefs {
                is_weapon: false,
                set: Some(0),
                item_type: Some(item_type),
                balance: None,
                manufacturer: Some(135),
                parts: slots,
            };
            let stats = db.stats(&refs, ItemLevel::new(30));
            assert_eq!(stats.unavailable, None);
            cards += 1;
            for kind in [
                StatKind::ShieldCapacity,
                StatKind::ShieldRechargeRate,
                StatKind::ShieldRechargeDelay,
            ] {
                let row = stats.row(kind).unwrap_or_else(|| panic!("no {kind:?} row"));
                assert!(
                    matches!(row.value, StatValue::Computed { .. }),
                    "{kind:?}: {:?}",
                    row.value
                );
            }
            for row in &stats.rows {
                if let (StatKind::Other, StatValue::Unknown { reason }) = (row.kind(), &row.value) {
                    let label = row.label.as_deref().unwrap_or("(no label)");
                    *other_unknown
                        .entry(format!("{label}: {reason}"))
                        .or_default() += 1;
                }
            }
        }
    }
    println!(
        "{} shield types × {} parts = {cards} cards; capacity, rate and delay always computed",
        types.len(),
        parts.len()
    );
    for (what, count) in &other_unknown {
        println!("  unknown special row, {count} cards: {what}");
    }
    assert!(types.len() >= 30 && cards > 20_000);
}

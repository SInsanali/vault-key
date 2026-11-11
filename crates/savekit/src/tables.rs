//! Static game data used by character edits.

/// Minimum total XP for each level; index 0 is level 1 (0 XP), index 79 is level 80.
///
/// Sources (public data, not computed from the formula):
/// 1. Borderlands fandom wiki, "Experience Points", table "Experience table (BL2)",
///    levels 1-80 (<https://borderlands.fandom.com/wiki/Experience_Points>).
/// 2. SegmentNext "Borderlands: Pre-Sequel Leveling Guide" XP table (same curve),
///    levels 1-50.
///
/// Levels 1-50 agree exactly between the two sources; levels 51-80 have only the first.
/// Every entry is within 3 of `ceil(60 * L^2.8 - 60)`.
///
/// The Python reference editor's thresholds at levels 72-80 are 1-3 XP lower than this table.
/// This table is kept on purpose: a higher threshold is safe whichever source is right,
/// because the character lands in level L either way.
pub const REQUIRED_XP: [i64; 80] = [
    0, 358, 1_241, 2_850, 5_376,
    8_997, 13_886, 20_208, 28_126, 37_798,
    49_377, 63_016, 78_861, 97_061, 117_757,
    141_092, 167_206, 196_238, 228_322, 263_595,
    302_190, 344_238, 389_873, 439_222, 492_414,
    549_578, 610_840, 676_325, 746_158, 820_463,
    899_363, 982_980, 1_071_435, 1_164_850, 1_263_343,
    1_367_034, 1_476_041, 1_590_483, 1_710_476, 1_836_137,
    1_967_582, 2_104_926, 2_248_285, 2_397_772, 2_553_501,
    2_715_586, 2_884_139, 3_059_273, 3_241_098, 3_429_728,
    3_625_271, 3_827_840, 4_037_543, 4_254_491, 4_478_792,
    4_710_556, 4_949_890, 5_196_902, 5_451_701, 5_714_393,
    5_985_086, 6_263_885, 6_550_897, 6_846_227, 7_149_982,
    7_462_266, 7_783_184, 8_112_840, 8_451_340, 8_798_786,
    9_155_282, 9_520_932, 9_895_840, 10_280_105, 10_673_832,
    11_077_123, 11_490_079, 11_912_803, 12_345_396, 12_787_958,
];

/// One ammo type: its pool size is `base + step * SDU level` (format doc 5.12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmoType {
    pub name: &'static str,
    pub base: u32,
    pub step: u32,
    /// Field 11 subfield 1.
    pub resource: &'static str,
    /// Field 11 subfield 2.
    pub pool: &'static str,
}

/// Highest ammo SDU level.
pub const MAX_AMMO_SDU_LEVEL: u64 = 7;

/// Ammo types in black-market (field 36) index order 0..=6.
pub const AMMO_TYPES: [AmmoType; 7] = [
    AmmoType {
        name: "rifle",
        base: 280,
        step: 140,
        resource: "D_Resources.AmmoResources.Ammo_Combat_Rifle",
        pool: "D_Resourcepools.AmmoPools.Ammo_Combat_Rifle_Pool",
    },
    AmmoType {
        name: "pistol",
        base: 200,
        step: 100,
        resource: "D_Resources.AmmoResources.Ammo_Repeater_Pistol",
        pool: "D_Resourcepools.AmmoPools.Ammo_Repeater_Pistol_Pool",
    },
    AmmoType {
        name: "launcher",
        base: 12,
        step: 3,
        resource: "D_Resources.AmmoResources.Ammo_Rocket_Launcher",
        pool: "D_Resourcepools.AmmoPools.Ammo_Rocket_Launcher_Pool",
    },
    AmmoType {
        name: "shotgun",
        base: 80,
        step: 20,
        resource: "D_Resources.AmmoResources.Ammo_Combat_Shotgun",
        pool: "D_Resourcepools.AmmoPools.Ammo_Combat_Shotgun_Pool",
    },
    AmmoType {
        name: "smg",
        base: 360,
        step: 180,
        resource: "D_Resources.AmmoResources.Ammo_Patrol_SMG",
        pool: "D_Resourcepools.AmmoPools.Ammo_Patrol_SMG_Pool",
    },
    AmmoType {
        name: "sniper",
        base: 48,
        step: 12,
        resource: "D_Resources.AmmoResources.Ammo_Sniper_Rifle",
        pool: "D_Resourcepools.AmmoPools.Ammo_Sniper_Rifle_Pool",
    },
    AmmoType {
        name: "grenade",
        base: 3,
        step: 1,
        resource: "D_Resources.AmmoResources.Ammo_Grenade_Protean",
        pool: "D_Resourcepools.AmmoPools.Ammo_Grenade_Protean_Pool",
    },
];

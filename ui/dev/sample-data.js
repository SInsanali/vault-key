// Made-up characters, items and backups for the browser preview. Not built into the app.

const STEAM = "76561190000000000";
const NOW = Math.floor(Date.now() / 1000);

function character(slot, name, classLabel, heroName, level, extra = {}) {
  const fileName = `Save000${slot}.sav`;
  return {
    slotId: `${STEAM}/${fileName}`,
    slot,
    steamId: STEAM,
    fileName,
    path: `C:\\Users\\you\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData\\${STEAM}\\${fileName}`,
    modifiedUnix: NOW - slot * 86_400,
    readable: true,
    problem: null,
    name,
    classLabel,
    heroName,
    level,
    saveGameId: slot,
    isTestSlot: false,
    createdByApp: false,
    canBeTestSlot: true,
    steamCloud: true,
    ...extra,
  };
}

export const CHARACTERS = [
  character(1, "Gaige", "Mechromancer", "Gaige", 61, { sample: "main" }),
  character(2, "Zer0", "Assassin", "Zer0", 1, { sample: "fresh" }),
  character(3, "ZeroDup", "Assassin", "Zer0", 1, { sample: "fresh", isTestSlot: true }),
  character(4, "Zer0", "Assassin", "Zer0", 30, { sample: "zero", createdByApp: true }),
  character(6, "", "", null, 0, { readable: false, problem: "Not a Borderlands 2 save: the file is too short.", sample: null }),
];

function snapshot(name, classLabel, heroName, level, extra = {}) {
  return {
    name,
    classLabel,
    heroName,
    level,
    experience: 0,
    saveGameId: 1,
    money: 0,
    eridium: 0,
    seraph: 0,
    torgue: 0,
    backpackSlots: 12,
    backpackUsed: 0,
    bankSlots: 6,
    bankUsed: 0,
    gunSlots: 2,
    opLevel: 0,
    playthroughsUnlocked: 0,
    slaughterdomeUnlocked: false,
    ...extra,
  };
}

export const SNAPSHOTS = {
  main: snapshot("Gaige", "Mechromancer", "Gaige", 61, { money: 2_481_920, eridium: 212, seraph: 40, backpackSlots: 27, bankSlots: 10, gunSlots: 4, playthroughsUnlocked: 2 }),
  fresh: snapshot("Zer0", "Assassin", "Zer0", 1, { money: 12 }),
  zero: snapshot("Zer0", "Assassin", "Zer0", 30, { backpackSlots: 39, gunSlots: 3, playthroughsUnlocked: 1, saveGameId: 4 }),
};

// A plausible card for the previews. The app computes the real one from the game's files.
function sampleStats(category, level, name) {
  const row = (key, label, text, note = null) => ({ key, label, text, note });
  const scale = Math.pow(1.13, level);
  if (["pistol", "smg", "assault-rifle", "sniper-rifle", "shotgun", "launcher"].includes(category)) {
    const rows = [
      row("damage", "Damage", String(Math.ceil(8 * scale * 1.1))),
      row("accuracy", "Accuracy", "91.4"),
      row("fire-rate", "Fire Rate", "8.6"),
      row("reload-speed", "Reload Speed", "2.3"),
      row("magazine-size", "Magazine Size", "24", name === "Lucid Florentine" ? "This value lies on a rounding boundary; the game may show 25." : null),
    ];
    if (name === "Thunderball Fists") {
      rows.push(row("elemental-damage", "Electrocute Dmg/Sec", "412.8"), row("elemental-chance", "Chance to Shock", "16.5%"));
    }
    const lines = name?.endsWith("Infinity") ? [{ kind: "text", text: "It's closer than you think! (no it isn't)", color: "#DC4646", note: null }] : [];
    lines.push({ kind: "bonus", text: "+8% Critical Hit Damage", color: null, note: null }, { kind: "bonus", text: "Consumes 2 ammo per shot.", color: null, note: null });
    return { rows, lines, missingLines: category === "sniper-rifle" ? 1 : 0 };
  }
  if (category === "shield") {
    return { rows: [row("shield-capacity", "Capacity", "1924"), row("shield-recharge-rate", "Recharge Rate", "312"), row("shield-recharge-delay", "Recharge Delay", "3.52"), row("other", "Max Health", "−93")], lines: [{ kind: "text", text: "High capacity, reduces max health.", color: null, note: null }], missingLines: 0 };
  }
  return null;
}

function item(ref, name, tier, tierLabel, color, manufacturer, category, categoryLabel, level, location, extra = {}) {
  const isWeapon = ref.startsWith("54:");
  return {
    ref,
    location,
    isWeapon,
    level,
    code: `BL2(${btoa(`mock-item-${ref}`)})`,
    canDuplicate: true,
    duplicateBlocked: null,
    isNew: false,
    changeId: null,
    copyOf: null,
    name,
    tier,
    tierLabel,
    color,
    manufacturer,
    category,
    categoryLabel,
    unverified: false,
    stats: sampleStats(category, level, name),
    raw: { set: 0, itemType: 1668 + Number(ref.split(":")[1]), balance: 129043 + Number(ref.split(":")[1]) * 7, manufacturer: 133 },
    ...extra,
  };
}

const UNVERIFIED = "This item is from add-on content that has not been checked in the game yet, so it can't be duplicated.";
const NOT_GEAR = "Only weapons and gear (shields, grenade mods, class mods and relics) can be duplicated.";

const ITEM_LISTS = {
  zero: [
    item("54:0", "Thunderball Fists", "legendary", "Legendary", "#FFB400", "Maliwan", "pistol", "Pistol", 30, "equipped"),
    item("54:1", "Infinity", "legendary", "Legendary", "#FFB400", "Vladof", "pistol", "Pistol", 30, "backpack"),
    item("53:0", "Vault Hunter's Relic", "rare", "Rare", "#3C8EFF", "Eridian", "relic", "Relic", 30, "equipped"),
    item("53:1", "Grounded Shield", "uncommon", "Uncommon", null, "Pangolin", "shield", "Shield", 29, "equipped"),
    item("54:2", "Basic Repeater", "common", "Common", "#FFFFFF", "Dahl", "pistol", "Pistol", 1, "backpack"),
    item("53:2", "Chaotic Evil Monk", "very-rare", "Very Rare", null, "Bandit", "class-mod", "Class Mod", 28, "backpack", {
      unverified: true,
      canDuplicate: false,
      duplicateBlocked: UNVERIFIED,
      raw: { set: 5, itemType: 3310, balance: 77120, manufacturer: 129 },
    }),
    item("53:3", null, "rare", "Rare", "#3C8EFF", null, "customization", "Customization", 1, "backpack", {
      unverified: true,
      canDuplicate: false,
      duplicateBlocked: UNVERIFIED,
      definition: "Head_MechromancerAster",
      raw: { set: 9, itemType: 68608, balance: 536576, manufacturer: 2047 },
    }),
    item("54:3", "Lucid Florentine", "seraph", "Seraph", "#FF9AB8", "Maliwan", "smg", "SMG", 30, "bank"),
    item("53:4", "Fire Storm", "e-tech", "E-tech", null, "Maliwan", "grenade-mod", "Grenade Mod", 27, "bank"),
  ],
  fresh: [
    item("54:0", "Basic Repeater", "common", "Common", "#FFFFFF", "Dahl", "pistol", "Pistol", 1, "equipped"),
    item("53:0", "Helmet Data Rec0rding", "common", "Common", "#FFFFFF", null, "mission", "Mission Item", 1, "backpack", { canDuplicate: false, duplicateBlocked: NOT_GEAR }),
  ],
  main: [
    item("54:0", "Righteous Infinity", "legendary", "Legendary", "#FFB400", "Vladof", "pistol", "Pistol", 61, "equipped"),
    item("54:1", "Casual Flakker", "legendary", "Legendary", "#FFB400", "Torgue", "shotgun", "Shotgun", 60, "equipped"),
    item("54:2", "Corrosive Plasma Caster", "e-tech", "E-tech", null, "Hyperion", "smg", "SMG", 59, "backpack"),
    item("53:0", "The Bee", "legendary", "Legendary", "#FFB400", "Hyperion", "shield", "Shield", 61, "equipped"),
    item("53:1", "Slayer of Terramorphous", "legendary", "Legendary", "#FFB400", "Dahl", "class-mod", "Class Mod", 50, "backpack"),
  ],
  imported: item("53:99", null, "unknown", "Unknown", null, null, "unknown", "Item", 30, "backpack"),
};

// Equipped weapons get their weapon slots in list order; every item gets an id that stays.
for (const list of Object.values(ITEM_LISTS)) {
  if (!Array.isArray(list)) continue;
  let n = 0;
  for (const entry of list) {
    entry.uid = `u-${entry.ref}`;
    entry.quickSlot = entry.isWeapon && entry.location === "equipped" ? ++n : null;
  }
}
ITEM_LISTS.imported.uid = "u-import";
export const ITEMS = ITEM_LISTS;

export const BACKUPS = [
  { id: `${STEAM}/Save0004.sav/20261108-221500Z`, slotId: `${STEAM}/Save0004.sav`, fileName: "Save0004.sav", slot: 4, createdUnix: NOW - 3_600 * 5, createdUtc: "", reason: "Before saving 3 changes", characterName: "Zer0", level: 30, size: 31_337 },
  { id: `${STEAM}/Save0004.sav/20261107-193000Z`, slotId: `${STEAM}/Save0004.sav`, fileName: "Save0004.sav", slot: 4, createdUnix: NOW - 3_600 * 31, createdUtc: "", reason: "Before saving 1 change", characterName: "Zer0", level: 18, size: 30_112 },
  { id: `${STEAM}/Save0004.sav/20261106-090000Z`, slotId: `${STEAM}/Save0004.sav`, fileName: "Save0004.sav", slot: 4, createdUnix: NOW - 3_600 * 65, createdUtc: "", reason: "Before restoring the backup from 2026-11-05 20:10:00 UTC", characterName: "Zer0", level: 1, size: 29_870 },
];

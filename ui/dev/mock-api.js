// A stand-in for the Rust side, for working on the UI in a browser:
//   index.html?mock&screen=gear
// It answers the same commands with made-up data (see sample-data.js) and keeps staged edits
// in memory. It is close enough to the real thing to show every screen and state; it does no
// checking worth the name. This folder is never built into the app.

import { ApiError } from "../js/api.js";
import { BACKUPS, CHARACTERS, ITEMS, SNAPSHOTS } from "./sample-data.js";

const params = new URLSearchParams(location.search);
const data = params.get("data") ?? "full";

const READ_ONLY =
  'This character is locked (read-only): Vault Key will not change its file. Unlock it to save changes into it, or use "Save to new slot".';
const BACKPACK_FULL = "Backpack full — free a slot or increase backpack size";
const EXTERNAL =
  "This character was opened from a file outside your save folder, so there is no slot to save it into. Export it (back over that file, if you like), or add it to your characters.";
/** The file "Import a save to edit…" pretends the user picked. */
const PICKED = "C:\\Users\\you\\Downloads\\From a friend\\Save0002.sav";

class Refusal extends Error {}

const characters = data === "empty" || data === "nofolder" ? [] : structuredClone(CHARACTERS);
const backups = structuredClone(BACKUPS);
let session = null;
let nextChange = 1;

const money = (n) => `$${n.toLocaleString("en-US")}`;
const number = (n) => n.toLocaleString("en-US");

function accessOf(entry) {
  if (entry.protected) return { writable: false, because: null, explanation: READ_ONLY };
  if (entry.createdByApp) return { writable: true, because: "created", explanation: "Vault Key created this character, so it can be edited in place." };
  if (entry.isTestSlot) return { writable: true, because: "test-slot", explanation: "This slot is marked as a test slot, so it can be edited in place." };
  return { writable: false, because: null, explanation: READ_ONLY };
}

function listing() {
  const taken = characters.map((c) => c.slot);
  return {
    characters: characters.map((c) => ({ ...c, access: accessOf(c) })),
    freeSlots: { [CHARACTERS[0].steamId]: [2, 3, 4, 5, 6, 7, 8, 9].filter((n) => !taken.includes(n)) },
  };
}

function itemData() {
  const names = params.get("names") ?? "ready";
  if (names === "loading") return { state: "loading", percent: 40, message: "Reading item names from your game…" };
  if (names === "off") return { state: "unavailable", message: "Borderlands 2 was not found in your Steam libraries. Item names and pictures need the game installed on this PC." };
  return { state: "ready", message: "Item names come from your game's own files." };
}

let mockTheme = params.get("theme") ?? "yellow";

function status() {
  const game = {
    running: { state: "running", message: "Borderlands 2 is running. Quit the game before you save." },
    unknown: { state: "unknown", message: "Can't tell whether Borderlands 2 is running (the list of running programs came back empty). Saving is blocked until that is known." },
  }[params.get("game")] ?? { state: "not-running", message: "Game closed. Safe to save." };
  const found = data !== "nofolder";
  return {
    app: "Vault Key",
    version: "0.9.0",
    logFile: "C:\\Users\\you\\AppData\\Local\\VaultKey\\vault-key.log",
    theme: mockTheme,
    saveFolder: found
      ? { found: true, path: "C:\\Users\\you\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData", dataDir: "C:\\Users\\you\\AppData\\Local\\VaultKey", problem: null }
      : {
          found: false,
          path: null,
          dataDir: null,
          problem: {
            kind: "save-folder",
            message: "Cannot find the game's save folder (looked in: C:\\Users\\you\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData).",
          },
        },
    game: found ? game : null,
    itemData: itemData(),
    open: session?.slotId ?? null,
    pendingCount: session?.edits.length ?? 0,
    notices: [],
    selftest: false,
  };
}

const LABELS = {
  name: ["Name", (v) => v],
  level: ["Level", number],
  money: ["Money", money],
  eridium: ["Eridium", number],
  seraph: ["Seraph crystals", number],
  torgue: ["Torgue tokens", number],
  backpackSlots: ["Backpack", number, (v) => `${v} slots`],
  bankSlots: ["Bank", number, (v) => `${v} slots`],
  gunSlots: ["Gun slots", number],
  opLevel: ["Overpower level", (v) => `OP ${v}`],
};

const DOES = {
  unlockTvhm: ["Unlock", "True Vault Hunter Mode"],
  unlockUvhm: ["Unlock", "Ultimate Vault Hunter Mode"],
  unlockSlaughterdome: ["Unlock", "Creature Slaughter Dome"],
  unlockAmmo: ["Unlock", "all ammo upgrades"],
  maximizeAmmo: ["Ammo", "fill to max"],
};

const AMMO = [
  ["rifle", "Assault rifle ammo", 280, 140],
  ["pistol", "Pistol ammo", 200, 100],
  ["launcher", "Launcher ammo", 12, 3],
  ["shotgun", "Shotgun ammo", 80, 20],
  ["smg", "SMG ammo", 360, 180],
  ["sniper", "Sniper ammo", 48, 12],
  ["grenade", "Grenades", 3, 1],
];

function ammoOf(index, level) {
  const [name, label, base, step] = AMMO[index];
  return { name, label, level, capacity: base + step * level, capacities: Array.from({ length: 8 }, (_, l) => base + step * l) };
}

function noNames(item) {
  return { ...item, name: null, tier: "unknown", tierLabel: "Unknown", color: null, manufacturer: null, category: "unknown", categoryLabel: "Item", unverified: false };
}

function view() {
  const s = session;
  const entry = s.external ?? characters.find((c) => c.slotId === s.slotId);
  s.original.ammo ??= AMMO.map((_, i) => ammoOf(i, i < 2 ? 2 : 0));
  const current = { ...s.original };
  let items = structuredClone(s.items);
  const changes = [];
  for (const edit of s.edits) {
    if (edit.kind in LABELS) {
      const [label, show, showTo] = LABELS[edit.kind];
      const from = show(s.original[edit.kind]);
      const to = (showTo ?? show)(edit.value);
      current[edit.kind] = edit.value;
      changes.push({ id: edit.id, kind: edit.kind, label, from, to, text: `${label} ${from} → ${to}`, itemRef: null });
    } else if (edit.kind in DOES) {
      const [label, to] = DOES[edit.kind];
      if (edit.kind === "unlockTvhm") current.playthroughsUnlocked = Math.max(current.playthroughsUnlocked, 1);
      if (edit.kind === "unlockUvhm") current.playthroughsUnlocked = 2;
      if (edit.kind === "unlockSlaughterdome") current.slaughterdomeUnlocked = true;
      changes.push({ id: edit.id, kind: edit.kind, label, from: null, to, text: `${label}: ${to}`, itemRef: null });
    } else if (edit.kind === "itemLevels") {
      const target = edit.level > 0 ? edit.level : current.level;
      items = items.map((item) => (item.location === "bank" || (!edit.force && item.level <= 1) ? item : { ...item, level: target }));
      const to = `all gear → ${edit.level > 0 ? `level ${edit.level}` : "the character's level"}${edit.force ? " (including level 1 items)" : ""}`;
      changes.push({ id: edit.id, kind: "itemLevels", label: "Item levels", from: null, to, text: `Item levels: ${to}`, itemRef: null });
    } else if (edit.kind === "itemLevel") {
      const item = items.find((i) => i.ref === edit.item);
      const from = item.level;
      item.level = edit.level;
      changes.push({ id: edit.id, kind: "itemLevel", label: "Item level", from: String(from), to: String(edit.level), text: `Item level ${from} → ${edit.level}`, itemRef: item.ref, itemName: item.name, itemTier: item.tier, itemColor: item.color });
    } else if (edit.kind === "moveItem") {
      const item = items.find((i) => i.ref === edit.item);
      item.location = edit.toBank ? "bank" : "backpack";
      const to = edit.toBank ? "to the bank" : "to the backpack";
      changes.push({ id: edit.id, kind: "moveItem", label: "Move", from: null, to, text: `Move: ${to}`, itemRef: item.ref, itemName: item.name, itemTier: item.tier, itemColor: item.color });
    } else if (edit.kind === "ammoLevel") {
      const index = current.ammo.findIndex((a) => a.name === edit.ammo);
      const was = s.original.ammo[index];
      current.ammo = current.ammo.map((a, i) => (i === index ? ammoOf(i, edit.level) : a));
      const at = (a) => `upgrade ${a.level} (${a.capacity} max)`;
      changes.push({ id: edit.id, kind: "ammoLevel", label: was.label, from: at(was), to: at(current.ammo[index]), text: `${was.label} ${at(was)} → ${at(current.ammo[index])}`, itemRef: null });
    } else if (edit.kind === "duplicate") {
      const source = items.find((item) => item.ref === edit.source);
      items.push({ ...source, ref: `copy:${edit.id}`, location: "backpack", isNew: true, changeId: edit.id, copyOf: edit.source });
      const kind = source.isWeapon ? "weapon" : "item";
      const to = `+1 copy of ${kind} ${edit.source} in the backpack`;
      changes.push({ id: edit.id, kind: "duplicate", label: "Duplicate", from: null, to, text: `Duplicate: ${to}`, itemRef: edit.source, itemName: source.name, itemTier: source.tier, itemColor: source.color });
    } else if (edit.kind === "importCodes") {
      const place = edit.toBank ? "bank" : "backpack";
      edit.codes.forEach((code, n) => {
        items.push({ ...noNames(ITEMS.imported), ref: `import:${edit.id}:${n}`, code, location: place, isNew: true, changeId: edit.id, copyOf: null, canDuplicate: false, duplicateBlocked: "Save the imported item first, then duplicate it." });
      });
      const to = `+${edit.codes.length} ${edit.codes.length === 1 ? "item" : "items"} in the ${place}`;
      changes.push({ id: edit.id, kind: "importCodes", label: "Import codes", from: null, to, text: `Import codes: ${to}`, itemRef: null });
    }
  }
  if (itemData().state !== "ready") items = items.map(noNames);
  current.backpackUsed = items.filter((item) => item.location === "backpack").length;
  current.bankUsed = items.filter((item) => item.location === "bank").length;
  const taken = characters.map((c) => c.slot);
  const free = [2, 3, 4, 5, 6, 7, 8, 9].filter((n) => !taken.includes(n));
  return {
    slotId: s.slotId,
    slot: entry.slot,
    steamId: entry.steamId,
    fileName: entry.fileName,
    source: s.external ? { kind: "external", path: s.external.path, folder: s.external.folder, fileName: entry.fileName } : { kind: "slot" },
    access: s.external ? { writable: false, because: null, explanation: EXTERNAL } : accessOf(entry),
    isTestSlot: entry.isTestSlot,
    current,
    original: { ...s.original },
    items,
    changes,
    pendingCount: s.edits.length,
    freeSlots: free,
    freeSlotsBySteamId: { [CHARACTERS[0].steamId]: free },
    backpackFull: current.backpackUsed >= current.backpackSlots,
    backpackFullMessage: BACKPACK_FULL,
    limits: { level: [1, 80], backpack: [12, 39], bank: [6, 24], opLevel: [0, 10], currency: [0, 2147483647], itemLevel: [1, 127] },
  };
}

function open(slotId) {
  const entry = characters.find((c) => c.slotId === slotId);
  if (!entry || !entry.readable) throw new Refusal(`${slotId.split("/")[1]} can't be opened. It is not a save file.`);
  session = { slotId, original: structuredClone(SNAPSHOTS[entry.sample]), items: structuredClone(ITEMS[entry.sample] ?? []), edits: [] };
  return view();
}

function openFile(path) {
  const fileName = path.split("\\").pop();
  const folder = path.slice(0, path.length - fileName.length - 1);
  const from = CHARACTERS[1];
  const external = { slot: null, steamId: from.steamId, fileName, isTestSlot: false, sample: from.sample, path, folder };
  session = { slotId: `file:${path}`, external, original: structuredClone(SNAPSHOTS[from.sample]), items: structuredClone(ITEMS[from.sample] ?? []), edits: [] };
  return view();
}

function exportFile(path) {
  if (!session) throw new Refusal("No character is open.");
  if (/\\SaveData(\\|$)/i.test(path)) throw new Refusal("The character was not exported there. That is inside the game's save folder.");
  const now = view();
  const fileName = path.split("\\").pop();
  return { report: { path, fileName, name: now.current.name, classLabel: now.current.classLabel, level: now.current.level, saveGameId: now.current.saveGameId, replacedBackup: null } };
}

function stage(args) {
  if (!session) throw new Refusal("No character is open.");
  const kind = args.kind;
  if (kind === "level" && (args.value < 1 || args.value > 80)) throw new Refusal("Level must be between 1 and 80.");
  if (kind === "name" && !args.value.trim()) throw new Refusal("Name can't be empty.");
  let edit;
  if (kind === "importCodes") {
    const codes = args.text.match(/BL2\([^)]*\)/g) ?? [];
    if (codes.length === 0) throw new Refusal("No item codes found. A code looks like BL2(…).");
    edit = { id: nextChange++, kind, codes, toBank: Boolean(args.toBank) };
    session.edits.push(edit);
    return { staged: true, changeId: edit.id, newItems: codes.map((_, n) => `import:${edit.id}:${n}`), character: view() };
  }
  if (kind === "moveItem") {
    const undone = session.edits.find((e) => e.kind === "moveItem" && e.item === args.item);
    if (undone) {
      session.edits = session.edits.filter((e) => e !== undone);
      return { staged: false, changeId: null, newItems: [], character: view() };
    }
    edit = { id: nextChange++, ...args };
    session.edits.push(edit);
    return { staged: true, changeId: edit.id, newItems: [], character: view() };
  }
  const same = (e) => e.kind === kind && (kind !== "itemLevel" || e.item === args.item) && (kind !== "ammoLevel" || e.ammo === args.ammo);
  const existing = session.edits.find(same);
  const unchanged =
    (kind in LABELS && session.original[kind] === args.value) ||
    (kind === "unlockTvhm" && session.original.playthroughsUnlocked >= 1) ||
    (kind === "unlockUvhm" && session.original.playthroughsUnlocked >= 2) ||
    (kind === "unlockSlaughterdome" && session.original.slaughterdomeUnlocked);
  if (unchanged) {
    session.edits = session.edits.filter((e) => !same(e));
    return { staged: false, changeId: null, newItems: [], character: view() };
  }
  if (existing) {
    Object.assign(existing, args);
    return { staged: true, changeId: existing.id, newItems: [], character: view() };
  }
  edit = { id: nextChange++, ...args };
  session.edits.push(edit);
  return { staged: true, changeId: edit.id, newItems: [], character: view() };
}

function duplicate(ref) {
  const now = view();
  const item = now.items.find((i) => i.ref === ref);
  if (!item) throw new Refusal("That item is no longer in the list.");
  if (!item.canDuplicate) throw new Refusal(item.duplicateBlocked);
  if (now.backpackFull) throw new Refusal(BACKPACK_FULL);
  const edit = { id: nextChange++, kind: "duplicate", source: item.copyOf ?? ref };
  session.edits.push(edit);
  return { staged: true, changeId: edit.id, newItems: [`copy:${edit.id}`], character: view() };
}

function gameCheck() {
  if (params.get("game") === "running") throw new Refusal("Borderlands 2 is running. Quit the game, then save.");
}

/** Makes the staged state the saved one. */
function settle() {
  const now = view();
  session.original = now.current;
  session.items = now.items.map((item, n) => ({ ...item, ref: item.isNew ? `53:${90 + n}` : item.ref, isNew: false, changeId: null, copyOf: null, canDuplicate: item.copyOf ? true : item.canDuplicate, duplicateBlocked: item.copyOf ? null : item.duplicateBlocked }));
  session.edits = [];
  const entry = characters.find((c) => c.slotId === session.slotId);
  entry.name = now.current.name;
  entry.level = now.current.level;
  SNAPSHOTS[entry.sample] = structuredClone(session.original);
  ITEMS[entry.sample] = structuredClone(session.items);
}

function addBackup(entry, snapshot, reason) {
  const createdUnix = Math.floor(Date.now() / 1000);
  const backup = { id: `${entry.slotId}/mock-${backups.length + 1}`, slotId: entry.slotId, fileName: entry.fileName, slot: entry.slot, createdUnix, createdUtc: "", reason, characterName: snapshot.name, level: snapshot.level, size: 31337 };
  backups.unshift(backup);
  return backup;
}

function save() {
  if (!session) throw new Refusal("No character is open.");
  const entry = characters.find((c) => c.slotId === session.slotId);
  const access = accessOf(entry);
  if (!access.writable) throw new Refusal(access.explanation);
  if (session.edits.length === 0) throw new Refusal("There are no changes to save.");
  gameCheck();
  const count = session.edits.length;
  const backup = addBackup(entry, session.original, `Before saving ${count} change${count === 1 ? "" : "s"}`);
  settle();
  return { report: { slotId: entry.slotId, slot: entry.slot, fileName: entry.fileName, path: `…\\${entry.fileName}`, createdNew: false, backup, warning: null }, character: view() };
}

function saveAsNew(slot) {
  if (!session) throw new Refusal("No character is open.");
  if (!(slot >= 2 && slot <= 9) || characters.some((c) => c.slot === slot)) throw new Refusal(`Slot ${slot} is not free.`);
  gameCheck();
  const from = session.external ?? characters.find((c) => c.slotId === session.slotId);
  const now = view();
  const sample = `made-${slot}`;
  const fileName = `Save000${slot}.sav`;
  const entry = { ...from, slotId: `${from.steamId}/${fileName}`, slot, fileName, saveGameId: slot, isTestSlot: false, createdByApp: true, canBeTestSlot: true, sample };
  characters.push(entry);
  characters.sort((a, b) => a.slot - b.slot);
  // The new file holds the staged state; the character it was made from is unchanged.
  SNAPSHOTS[sample] = { ...now.current, saveGameId: slot };
  ITEMS[sample] = now.items;
  session = { slotId: entry.slotId, original: SNAPSHOTS[sample], items: now.items, edits: [] };
  delete entry.path;
  delete entry.folder;
  settle();
  return { report: { slotId: entry.slotId, slot, fileName, path: `…\\${fileName}`, createdNew: true, backup: null, warning: null }, character: view() };
}

function restore(backupId) {
  const backup = backups.find((b) => b.id === backupId);
  if (!backup) throw new Refusal(`There is no backup "${backupId}".`);
  gameCheck();
  const entry = characters.find((c) => c.slotId === backup.slotId);
  if (!accessOf(entry).writable) throw new Refusal(accessOf(entry).explanation);
  addBackup(entry, SNAPSHOTS[entry.sample], "Before restoring an earlier backup");
  SNAPSHOTS[entry.sample] = { ...SNAPSHOTS[entry.sample], name: backup.characterName, level: backup.level };
  entry.level = backup.level;
  if (session?.slotId === entry.slotId) open(entry.slotId);
  return { report: { slotId: entry.slotId, slot: entry.slot, fileName: entry.fileName, path: "", createdNew: false, backup: backups[0], warning: null }, character: session ? view() : null };
}

function run(command, args) {
  switch (command) {
    case "ping":
      return { app: "Vault Key", version: "0.9.0" };
    case "status":
    case "retry_start":
      return status();
    case "list_characters":
      return listing();
    case "open_character":
      return open(args.slotId);
    case "character":
      if (!session) throw new Refusal("No character is open.");
      return view();
    case "pick_open_file":
      return PICKED;
    case "pick_export_file":
      return `${args.folder ?? "C:\\Users\\you\\Documents"}\\${args.suggestedName}`;
    case "open_file":
      return openFile(args.path);
    case "export_file":
      return exportFile(args.path);
    case "close_character":
      session = null;
      return null;
    case "stage":
      return stage(args);
    case "duplicate_item":
      return duplicate(args.item);
    case "remove_change":
      session.edits = session.edits.filter((e) => e.id !== args.changeId);
      return view();
    case "discard_changes":
      session.edits = [];
      return view();
    case "save":
      return save();
    case "save_as_new":
      return saveAsNew(args.slot);
    case "list_backups":
      return { backups: session?.external ? [] : backups.filter((b) => b.slotId === (args?.slotId ?? session?.slotId)) };
    case "restore_backup":
      return restore(args.backupId);
    case "mark_test_slot":
    case "unmark_test_slot": {
      const entry = characters.find((c) => c.slotId === args.slotId);
      entry.isTestSlot = command === "mark_test_slot";
      return { ...listing(), character: session ? view() : null };
    }
    case "set_theme":
      mockTheme = args.theme;
      return { theme: args.theme };
    case "lock_character":
    case "unlock_character": {
      const entry = characters.find((c) => c.slotId === args.slotId);
      entry.protected = command === "lock_character";
      entry.isTestSlot = !entry.protected && !entry.createdByApp;
      return { ...listing(), character: session ? view() : null };
    }
    case "export_codes": {
      const codes = view().items.map((item) => item.code).filter(Boolean);
      return { codes, text: codes.join("\n") };
    }
    case "quit":
      return null;
    default:
      throw new Refusal(`unknown command: ${command}`);
  }
}

/** Same contract as the real transport: resolves with the result or rejects with a message. */
export async function call(command, args) {
  try {
    return structuredClone(run(command, args ?? {}));
  } catch (error) {
    if (error instanceof Refusal) throw new ApiError(error.message);
    throw error;
  }
}

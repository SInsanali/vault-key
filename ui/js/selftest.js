// The end-to-end self-test. `GK_SELFTEST=1` loads the page with `?selftest`; this drives the
// real app through a whole session and writes each step's result to `selftest.log` in the
// data folder, then closes the app.
//
// It creates a character, saves and restores, so it must only ever run on a sandbox save
// folder. The Rust side enforces that (it refuses to start a self-test without one, and only
// then accepts `selftest_log`); this file checks again and does nothing if the app says no.

import { call } from "./api.js";

const TIMEOUT = 120_000;
const NAMES_TIMEOUT = 60_000;

class Failed extends Error {}

function check(condition, what) {
  if (!condition) throw new Failed(what);
}

function same(actual, expected, what) {
  check(actual === expected, `${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const count = (selector) => document.querySelectorAll(selector).length;
const text = (selector) => document.querySelector(selector)?.textContent ?? "";

export async function run({ state, actions }) {
  let status;
  try {
    status = await call("status");
  } catch {
    return;
  }
  if (!status.selftest) {
    // Not a sandbox: touch nothing, just close.
    await call("quit");
    return;
  }

  let passed = 0;
  let failed = 0;
  const log = (line) => call("selftest_log", { line }).catch(() => {});
  const step = async (name, body) => {
    try {
      const detail = await body();
      passed += 1;
      await log(`PASS ${name}${detail ? `: ${detail}` : ""}`);
      return true;
    } catch (error) {
      failed += 1;
      await log(`FAIL ${name}: ${error instanceof Failed ? error.message : `${error?.name ?? "Error"}: ${error?.message ?? error}`}`);
      return false;
    }
  };

  // Whatever happens, the app closes by itself.
  const watchdog = setTimeout(async () => {
    await log(`RESULT: FAIL (timed out after ${TIMEOUT / 1000} s; ${passed} passed, ${failed} failed)`);
    await call("quit");
  }, TIMEOUT);

  await log(`START self-test, Vault Key ${status.version}`);
  await log(`INFO save folder: ${status.saveFolder.path}`);
  await log(`INFO data folder: ${status.saveFolder.dataDir}`);

  const memo = {};

  const flow = async () => {
    if (!(await step("the save folder is found and the game is closed", async () => {
      check(status.saveFolder.found, status.saveFolder.problem?.message ?? "no save folder");
      const fresh = await call("status", { fresh: true });
      same(fresh.game.state, "not-running", "game state");
      return fresh.game.message;
    }))) return;

    await step("the fonts and every other file come from inside the app", async () => {
      // `load` fetches a face if the page has not needed it yet, and gives back what it got.
      const loaded = async (font, what) => {
        const faces = await document.fonts.load(font);
        check(faces.length === 1 && faces[0].status === "loaded", `${what} did not load`);
      };
      await loaded('400 26px "Bangers"', "the heading font (Bangers)");
      for (const weight of [500, 600, 700, 800]) {
        await loaded(`${weight} 16px "Barlow Condensed"`, `the body font (Barlow Condensed ${weight})`);
      }
      const resources = performance.getEntriesByType("resource").map((entry) => new URL(entry.name));
      const outside = resources.filter((url) => url.origin !== location.origin);
      check(outside.length === 0, `loaded from outside the app: ${outside.map(String).join(", ")}`);
      const fonts = resources.filter((url) => url.pathname.endsWith(".ttf")).length;
      return `${resources.length} files, ${fonts} of them fonts, all from ${location.origin}; secure context: ${window.isSecureContext}`;
    });

    await step("item names load from the game", async () => {
      const started = Date.now();
      let now = await call("status");
      while (now.itemData.state === "loading" && Date.now() - started < NAMES_TIMEOUT) {
        await sleep(250);
        now = await call("status");
      }
      memo.names = now.itemData.state === "ready";
      check(now.itemData.state !== "loading", "still loading after 60 s");
      // No game install is not a failure of the app: it then shows raw facts instead.
      return `${now.itemData.state} (${now.itemData.message}) after ${Date.now() - started} ms`;
    });
    await actions.refreshStatus();

    if (!(await step("the character list shows slot 1 as read-only", async () => {
      await actions.reload();
      const list = await call("list_characters");
      check(list.characters.length >= 1, "no characters in the sandbox");
      const first = list.characters.find((c) => c.slot === 1);
      check(first, "no slot 1 in the sandbox");
      check(first.readable, `slot 1 is unreadable: ${first.problem}`);
      same(first.access.writable, false, "slot 1 writable");
      same(count("#sidebar button.char"), list.characters.filter((c) => c.readable).length, "character buttons in the sidebar");
      check(document.querySelector('#sidebar button.char[data-slot="1"].locked'), "slot 1 has no lock in the sidebar");
      memo.slot1 = first.slotId;
      memo.steamId = first.steamId;
      memo.listed = list.characters.length;
      return `${list.characters.length} character(s); slot 1 is ${first.name}, ${first.classLabel} level ${first.level}`;
    }))) return;

    if (!(await step("opening slot 1 shows the character and its gear", async () => {
      check(await actions.openCharacter(memo.slot1, { force: true }), "could not open slot 1");
      actions.setTab("gear");
      const c = state.character;
      same(c.slotId, memo.slot1, "open character");
      same(c.pendingCount, 0, "pending changes");
      // The backpack's cards, and the equipped items (chips, or cards with the slots shown).
      same(count("#cards .card, #cards .eq-chip[data-ref]"), c.items.filter((item) => item.location !== "bank").length, "items shown on the Gear tab");
      actions.setGearView("bank");
      same(count('[data-frame="inventory"] .card'), c.items.filter((item) => item.location === "bank").length, "cards in the Bank view");
      actions.setGearView("backpack");
      check(text(".char-title h1").includes(c.current.name), "the header does not show the name");
      check(document.querySelector(".char-head .chip.lock"), "the header does not show the read-only lock");
      memo.before = { ...c.current };
      memo.itemCount = c.items.length;
      const named = c.items.filter((item) => item.name).length;
      if (memo.names) check(named > 0, "item data is loaded but no item has a name");
      return `${c.items.length} items, ${named} with names, backpack ${c.current.backpackUsed}/${c.current.backpackSlots}`;
    }))) return;

    await step("the other tabs render", async () => {
      actions.setTab("overview");
      same(count("#content .panel"), 3, "Overview panels");
      same(document.querySelector('[data-fk="field-name"]').value, state.character.current.name, "name box");
      actions.setTab("unlocks");
      same(count("#content .panel"), 4, "Unlocks panels");
      await actions.setTab("backups");
      same(count("#content .panel"), 1, "Backups panel");
      same(state.backups.length, 0, "backups of an untouched character");
      actions.setTab("gear");
    });

    await step("stage level 30, money 123,456 and a 39-slot backpack", async () => {
      check((await actions.stage({ kind: "level", value: 30 }))?.staged, "level was not staged");
      // Money is typed into its box on the Overview tab, the way a person would.
      actions.setTab("overview");
      const moneyBox = () => document.querySelector('[data-fk="field-money"]');
      moneyBox().focus();
      moneyBox().value = "123456";
      moneyBox().dispatchEvent(new Event("input", { bubbles: true }));
      for (let n = 0; n < 60 && state.character.current.money !== 123456; n++) await sleep(50);
      same(state.character.current.money, 123456, "money staged while typing");
      same(document.activeElement, moneyBox(), "the box being typed in keeps the keyboard focus");
      same(moneyBox().value, "123456", "the half-typed text is left alone");
      moneyBox().blur();
      for (let n = 0; n < 60 && moneyBox().value !== "$123,456"; n++) await sleep(50);
      same(moneyBox().value, "$123,456", "the money box after leaving it");
      check((await actions.stage({ kind: "backpackSlots", value: 39 }))?.staged, "backpack was not staged");
      const c = state.character;
      same(c.current.level, 30, "level");
      same(c.current.money, 123456, "money");
      same(c.current.backpackSlots, 39, "backpack slots");
      same(c.original.level, memo.before.level, "level on disk");
      same(c.pendingCount, 3, "pending changes");
      same(text("#unsaved-count"), "3 unsaved changes", "the header's count");
      actions.setTab("overview");
      same(count("#content .field.changed"), 3, "fields marked as changed on Overview");
      actions.setTab("gear");
      return c.changes.map((change) => change.text).join("; ");
    });

    await step("a refused edit says why and stages nothing", async () => {
      const before = state.character.pendingCount;
      same(await actions.stage({ kind: "level", value: 500 }), null, "an impossible level");
      same(state.character.pendingCount, before, "pending changes");
      const shown = text("#toasts .toast.error");
      check(shown.includes("Level must be between 1 and 80"), `no error message shown (got "${shown}")`);
      for (const toast of [...state.toasts]) actions.dismissToast(toast.id);
      same(count("#toasts .toast"), 0, "messages left after dismissing");
      return shown.replace("That didn't work", "").trim();
    });

    await step("duplicate a weapon", async () => {
      const weapon = state.character.items.find((item) => item.isWeapon && item.canDuplicate);
      check(weapon, "no weapon that can be duplicated");
      actions.selectItem(weapon.ref);
      check(!document.getElementById("duplicate-button").disabled, "the Duplicate button is disabled");
      document.getElementById("duplicate-button").click();
      for (let n = 0; n < 40 && state.character.pendingCount < 4; n++) await sleep(50);
      const c = state.character;
      same(c.pendingCount, 4, "pending changes");
      same(c.items.length, memo.itemCount + 1, "items");
      const copy = c.items.find((item) => item.isNew);
      check(copy, "no new item");
      same(copy.copyOf, weapon.ref, "the copy's source");
      same(copy.code, weapon.code, "the copy's item code");
      same(count("#cards .card.is-new"), 1, "cards marked as a new copy");
      memo.weaponCode = weapon.code;
      return `${weapon.name ?? "unnamed weapon"} (${weapon.ref}), ${weapon.tierLabel}`;
    });

    await step("the selected weapon shows its card, computed from the game's data", async () => {
      const weapon = state.character.items.find((item) => item.code === memo.weaponCode && !item.isNew);
      check(weapon?.stats, "the weapon has no computed card");
      actions.selectItem(weapon.ref);
      const rows = [...document.querySelectorAll(".detail .stat-rows > div")].map(
        (row) => `${row.querySelector("dt").textContent} ${row.querySelector("dd").textContent}`,
      );
      check(rows.length >= 5, `only ${rows.length} card rows are shown`);
      same(rows.length, weapon.stats.rows.length, "card rows shown");
      check(/^\d/.test(document.querySelector('.detail [data-stat="damage"] dd')?.textContent.trim() ?? ""), "no damage number");
      return `${weapon.name}: ${rows.join(", ")}`;
    });

    await step("the selected weapon shows its picture, and an item without one says why", async () => {
      const until = async (find, what) => {
        for (let n = 0; n < 300; n++) {
          const found = find();
          if (found) return found;
          await sleep(50);
        }
        throw new Failed(what);
      };
      const weapon = state.character.items.find((item) => item.code === memo.weaponCode && !item.isNew);
      // With Equipped expanded, an equipped weapon has a card with its picture.
      actions.setEquippedOpen(true);
      const picture = await until(() => {
        const img = document.querySelector(".detail .gun-pic img");
        return img && img.complete && img.naturalWidth > 0 ? img : null;
      }, "the weapon's picture did not load");
      const shown = picture.getBoundingClientRect();
      check(shown.width > 200 && shown.height > 100, `the picture is shown at ${shown.width}×${shown.height}`);
      const natural = `${picture.naturalWidth}×${picture.naturalHeight}`;
      // The weapon's card and its new copy's card show the same small picture.
      const thumbs = [...document.querySelectorAll("#cards .card-pic img")];
      const mine = thumbs.filter((img) => img.src.includes(encodeURIComponent(weapon.code)));
      same(mine.length, 2, "cards showing this weapon's picture");
      same(mine[0].src, mine[1].src, "the copy's picture address");

      // An item with no picture: a story item, or one from DLC (shields, grenade mods, class
      // mods and relics of the base game have pictures).
      const other = state.character.items.find((item) => !item.isWeapon && item.code && !["shield", "grenade-mod", "relic", "class-mod"].includes(item.category));
      check(other, "no item without a picture");
      actions.selectItem(other.ref);
      const none = await until(() => document.querySelector(".detail .gun-pic.none[title]"), "no placeholder for an item without a picture");
      const why = none.getAttribute("title");
      check(why.length > 10 && !document.querySelector(".detail .gun-pic img"), "the placeholder does not say why");
      same(none.getBoundingClientRect().height, picture.height + 6, "the placeholder's height");
      actions.selectItem(weapon.ref);
      actions.setEquippedOpen(false);
      return `picture ${natural}, shown at ${Math.round(shown.width)}×${Math.round(shown.height)}; ${thumbs.length} card pictures; an item says "${why}"`;
    });

    await step("the Gear screen is laid out inside the window", async () => {
      const box = (selector) => {
        const found = document.querySelector(selector);
        check(found, `${selector} is missing`);
        return found.getBoundingClientRect();
      };
      const inside = (rect, what) =>
        check(rect.width > 0 && rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight, `${what} is not fully inside the window`);
      check(document.documentElement.scrollWidth <= innerWidth, "the page is wider than the window");
      inside(box("#sidebar"), "the sidebar");
      inside(box("#save-button"), "the Save button");
      inside(box(".tabs"), "the tabs");
      inside(box("#duplicate-button"), "the Duplicate Item button");
      inside(box(".game-status"), "the game status");
      const cards = box("#cards");
      const detail = box(".detail");
      check(detail.left >= cards.right, "the detail panel is not beside the item cards");
      check(box("#sidebar").right <= cards.left, "the sidebar overlaps the item cards");
      const family = (selector) => getComputedStyle(document.querySelector(selector)).fontFamily.split(",")[0].replaceAll('"', "").trim();
      same(family(".char-title h1"), "Bangers", "heading font");
      same(family(".card-maker"), "Barlow Condensed", "label font");
      const banner = getComputedStyle(document.querySelector("#cards .card:not(.raw) .banner")).backgroundColor;
      return `window ${innerWidth}×${innerHeight} at ${devicePixelRatio}x; first rarity banner is ${banner}`;
    });

    await step("saving over a read-only character (slot 1) is refused", async () => {
      let refusal = null;
      try {
        await call("save");
      } catch (error) {
        refusal = error.message;
      }
      check(refusal, "the save was not refused");
      same(state.character.pendingCount, 4, "pending changes after the refusal");
      return refusal;
    });

    if (!(await step("save as a new character in a free slot", async () => {
      actions.openSaveSheet();
      const sheet = state.sheet;
      same(sheet?.mode, "new", "the Save sheet's mode for a read-only character");
      check(sheet.slot !== null, "no free slot");
      check(document.getElementById("sheet").open, "the Save sheet is not open");
      same(count("#sheet .change"), 4, "changes listed on the Save sheet");
      for (let n = 0; n < 40 && state.status?.game?.state !== "not-running"; n++) await sleep(50);
      const answer = await actions.confirmSave();
      check(answer, state.sheet?.error ?? "the save failed");
      same(answer.report.createdNew, true, "created a new file");
      same(answer.report.slot, sheet.slot, "slot");
      same(state.sheet.phase, "done", "the Save sheet's result");
      check(text("#sheet-title").includes("Saved"), "the Save sheet does not say Saved");
      actions.closeSheet();
      check(!document.getElementById("sheet").open, "the sheet did not close");
      memo.newSlot = answer.report.slot;
      memo.newSlotId = answer.report.slotId;
      return `${answer.report.fileName} at ${answer.report.path}`;
    }))) return;

    await step("the new character is open, editable, and holds the changes", async () => {
      const list = await call("list_characters");
      same(list.characters.length, memo.listed + 1, "characters");
      const entry = list.characters.find((c) => c.slotId === memo.newSlotId);
      check(entry, "the new character is not in the list");
      same(entry.access.writable, true, "new character writable");
      same(entry.access.because, "created", "why it is writable");
      same(entry.saveGameId, memo.newSlot, "save-game ID");
      const reopened = await call("open_character", { slotId: memo.newSlotId });
      same(reopened.current.level, 30, "level");
      same(reopened.current.money, 123456, "money");
      same(reopened.current.backpackSlots, 39, "backpack slots");
      same(reopened.items.length, memo.itemCount + 1, "items");
      same(reopened.items.filter((item) => item.code === memo.weaponCode).length, 2, "copies of the duplicated weapon");
      same(reopened.pendingCount, 0, "pending changes");
      same(state.character.slotId, memo.newSlotId, "the character shown in the window");
      check(!document.querySelector(".char-head .chip.lock"), "the header still shows a lock");
      check(document.querySelector(`#sidebar button.char[data-slot="${memo.newSlot}"][aria-current="true"]`), "the new character is not selected in the sidebar");
    });

    await step("slot 1 is unchanged", async () => {
      const list = await call("list_characters");
      const first = list.characters.find((c) => c.slot === 1);
      same(first.level, memo.before.level, "slot 1 level");
      same(first.access.writable, false, "slot 1 writable");
    });

    if (!(await step("stage a change on the new character and save in place", async () => {
      memo.eridiumBefore = state.character.current.eridium;
      check((await actions.stage({ kind: "eridium", value: memo.eridiumBefore + 77 }))?.staged, "eridium was not staged");
      actions.openSaveSheet();
      same(state.sheet?.mode, "in-place", "the Save sheet's mode for a writable character");
      for (let n = 0; n < 40 && state.status?.game?.state !== "not-running"; n++) await sleep(50);
      const button = document.getElementById("sheet-go");
      check(button && !button.disabled, "the Back up & save button is disabled");
      button.click();
      for (let n = 0; n < 100 && state.sheet?.phase !== "done" && !state.sheet?.error; n++) await sleep(50);
      check(!state.sheet?.error, state.sheet?.error);
      same(state.sheet?.phase, "done", "the Save sheet's result");
      const report = state.sheet.report;
      same(report.createdNew, false, "created a new file");
      check(report.backup, "no backup was made");
      actions.closeSheet();
      same(state.character.current.eridium, memo.eridiumBefore + 77, "eridium after the save");
      same(state.character.pendingCount, 0, "pending changes");
      memo.backupId = report.backup.id;
      return `backup ${report.backup.id} (${report.backup.reason})`;
    }))) return;

    await step("the backup is listed and restoring it undoes the save", async () => {
      await actions.setTab("backups");
      same(state.backups.length, 1, "backups");
      same(state.backups[0].id, memo.backupId, "backup id");
      same(count("#content .row.backup"), 1, "backup rows");
      actions.askRestore(state.backups[0]);
      same(state.sheet?.type, "restore", "the restore sheet");
      for (let n = 0; n < 40 && state.status?.game?.state !== "not-running"; n++) await sleep(50);
      const answer = await actions.confirmRestore();
      check(answer, state.sheet?.error ?? "the restore failed");
      same(state.character.current.eridium, memo.eridiumBefore, "eridium after the restore");
      same(state.character.current.level, 30, "level after the restore");
      same(state.backups.length, 2, "backups after the restore (the restore backs up first)");
      const reopened = await call("open_character", { slotId: memo.newSlotId });
      same(reopened.current.eridium, memo.eridiumBefore, "eridium on disk after the restore");
    });

    if (!(await step("level a weapon, move it to the bank and upgrade one ammo type with the buttons, then save in place", async () => {
      actions.setTab("gear");
      const weapon = state.character.items.find((item) => item.isWeapon && item.location === "backpack" && item.level !== null);
      check(weapon, "no weapon in the backpack");
      actions.selectItem(weapon.ref);
      check(document.querySelector('[data-frame="equipped"]') && document.querySelector('[data-frame="inventory"]'), "the Gear tab has no Equipped or Inventory frame");
      const up = document.querySelector('[data-fk="level-up"]');
      check(up && !up.disabled, "the level + button is missing or off");
      up.click();
      for (let n = 0; n < 40 && state.character.pendingCount < 1; n++) await sleep(25);
      same(text("#item-level"), String(weapon.level + 1), "the level shown after +");
      const move = document.getElementById("move-button");
      check(move && !move.disabled, "the Move to bank button is missing or off");
      move.click();
      for (let n = 0; n < 40 && state.character.pendingCount < 2; n++) await sleep(25);
      const banked = state.character.items.find((item) => item.uid === weapon.uid);
      same(banked?.location, "bank", "where the weapon is after the move");
      same(state.selectedItem, banked.ref, "the selection follows the weapon");
      actions.setGearView("bank");
      check(document.querySelector(`[data-frame="inventory"] .card[data-ref="${banked.ref}"]`), "the weapon is not in the Bank view");
      actions.setGearView("backpack");
      actions.setTab("unlocks");
      const sniper = state.character.current.ammo.find((a) => a.name === "sniper");
      const ammoUp = document.querySelector('[data-fk="ammo-up-sniper"]');
      check(ammoUp, "no + button for sniper ammo");
      if (sniper.level < 7) {
        ammoUp.click();
        for (let n = 0; n < 40 && state.character.pendingCount < 3; n++) await sleep(25);
      }
      actions.openSaveSheet();
      for (let n = 0; n < 40 && state.status?.game?.state !== "not-running"; n++) await sleep(50);
      document.getElementById("sheet-go").click();
      for (let n = 0; n < 100 && state.sheet?.phase !== "done" && !state.sheet?.error; n++) await sleep(50);
      check(!state.sheet?.error, state.sheet?.error);
      actions.closeSheet();
      const reopened = await call("open_character", { slotId: memo.newSlotId });
      const onDisk = reopened.items.find((item) => item.uid === weapon.uid);
      same(onDisk?.location, "bank", "where the weapon is on disk");
      same(onDisk?.level, weapon.level + 1, "the weapon's level on disk");
      same(reopened.current.ammo.find((a) => a.name === "sniper").level, Math.min(7, sniper.level + 1), "sniper ammo upgrade on disk");
      return `${weapon.name ?? weapon.ref}: level ${weapon.level} → ${onDisk.level}, now ${onDisk.ref}`;
    }))) return;

    await step("closing the window with unsaved changes asks first", async () => {
      check((await actions.stage({ kind: "torgue", value: 5 }))?.staged, "a change was not staged");
      // What the window does when its close button is pressed.
      window.gkCloseRequested();
      check(state.sheet?.thenQuit, "the quit sheet did not open");
      const title = document.querySelector("#sheet h2, #sheet h1")?.textContent ?? "";
      check(/quit with 1 unsaved change/i.test(title), `the quit sheet says "${title}"`);
      actions.closeSheet();
      actions.askDiscard();
      await actions.confirmDrop();
      same(state.character.pendingCount, 0, "pending changes after dropping them");
      return title;
    });

    await step("marking slot 1 as a test slot makes it editable, unmarking makes it read-only again, and Settings lists the characters", async () => {
      const first = (answer) => answer.characters.find((c) => c.slotId === memo.slot1);
      const marked = first(await call("mark_test_slot", { slotId: memo.slot1 }));
      same(marked.access.writable, true, "slot 1 writable after marking");
      same(marked.isTestSlot, true, "slot 1 marked");
      const unmarked = first(await call("unmark_test_slot", { slotId: memo.slot1 }));
      same(unmarked.access.writable, false, "slot 1 writable after unmarking");
      actions.openSettings();
      check(count("#sheet .row") >= 2, "the settings sheet does not list the characters");
      actions.closeSheet();
      return unmarked.access.explanation;
    });

    // Files from outside the save folder. The sandbox has an "Outside" folder beside the data
    // folder, holding a copy of the fixture as Friend.sav (scripts/app-selftest puts it there).
    const outside = `${state.status.saveFolder.dataDir.replace(/[\\/][^\\/]+$/, "")}\\Outside`;
    if (!(await step("a save file from outside the save folder opens as a character that is never written", async () => {
      check(/import a save/i.test(text("#open-file-button")), `the import button says "${text("#open-file-button")}"`);
      check(await actions.openFilePath(`${outside}\\Friend.sav`), "the file did not open");
      const c = state.character;
      same(c.source.kind, "external", "the character's source");
      same(c.source.fileName, "Friend.sav", "the file name");
      same(c.access.writable, false, "writable");
      same(c.slot, null, "slot");
      check(document.querySelector("#sidebar .char.file[aria-current='true']"), "the sidebar does not show the file");
      check(!document.getElementById("tab-backups"), "the Backups tab is shown for a file");
      check(/^export/i.test(text("#save-button")), `the big button says "${text("#save-button")}"`);
      check(/save to new slot/i.test(text("#add-button")), "there is no Save to new slot button");
      check(text(".callout").includes(c.source.path), "the header does not say where the file is");
      memo.friendItems = c.items.length;
      return c.source.path;
    }))) return;

    if (!(await step("stage money, backpack and a copy, export to a file, and the changes stay pending", async () => {
      check((await actions.stage({ kind: "money", value: 4242 }))?.staged, "money was not staged");
      // The fixture's backpack is over capacity (13 of 12), so make room for the copy.
      check((await actions.stage({ kind: "backpackSlots", value: 39 }))?.staged, "the backpack size was not staged");
      const weapon = state.character.items.find((item) => item.isWeapon && item.canDuplicate);
      check(weapon, "no weapon that can be duplicated");
      check(await actions.duplicate(weapon.ref), "the weapon was not duplicated");
      // Into the save folder: refused, with a reason.
      const steamDir = `${state.status.saveFolder.path}\\76561190000000000`;
      let refusal = null;
      try {
        await call("export_file", { path: `${steamDir}\\Save0009.sav` });
      } catch (error) {
        refusal = error.message;
      }
      check(refusal?.includes("save folder"), `exporting into the save folder was not refused (${refusal})`);
      // Export reviews first, as Save does: the changes, with Export as the button.
      document.getElementById("save-button").click();
      same(state.sheet?.type, "export", "the sheet after pressing Export");
      same(count("#sheet .change"), 3, "changes listed on the Export sheet");
      check(/^export/i.test(text("#sheet-go")), `the sheet's button says "${text("#sheet-go")}"`);
      const answer = await actions.exportTo(`${outside}\\Save0001.sav`);
      check(answer, "the export failed");
      same(state.sheet?.type, "exported", "the sheet after exporting");
      same(answer.report.saveGameId, 1, "the export's save-game ID");
      check(text("#export-path").endsWith("Save0001.sav"), "the sheet does not say where the file went");
      check(text("#sheet").includes("Steam Cloud"), "the sheet does not say where save files go");
      actions.closeSheet();
      same(state.character.pendingCount, 3, "pending changes after exporting");
      return `${answer.report.path}; refused in the save folder: ${refusal}`;
    }))) return;

    await step("the exported file opens with the changes in it", async () => {
      check(await actions.openFilePath(`${outside}\\Save0001.sav`), "the export did not open");
      const c = state.character;
      same(c.current.money, 4242, "money in the export");
      same(c.items.length, memo.friendItems + 1, "items in the export");
      same(c.pendingCount, 0, "pending changes");
    });

    await step("add the file's character to my characters, in a free slot with its own ID", async () => {
      actions.openSaveSheet();
      same(state.sheet?.mode, "new", "the sheet's mode for a file");
      check(/new slot/i.test(text("#sheet-title")), `the sheet says "${text("#sheet-title")}"`);
      const slot = state.sheet.slot;
      check(slot, "no free slot offered");
      for (let n = 0; n < 40 && state.status?.game?.state !== "not-running"; n++) await sleep(50);
      const answer = await actions.confirmSave();
      check(answer, state.sheet?.error ?? "adding failed");
      same(answer.report.createdNew, true, "created a new file");
      same(state.character.source.kind, "slot", "the character shown afterwards");
      same(state.character.current.saveGameId, slot, "save-game ID of the added character");
      same(state.character.current.money, 4242, "money of the added character");
      actions.closeSheet();
      return `${answer.report.fileName}, save-game ID ${slot}`;
    });
  };

  try {
    await flow();
  } catch (error) {
    failed += 1;
    await log(`FAIL unexpected error: ${error?.message ?? error}`);
  }
  clearTimeout(watchdog);
  await log(`RESULT: ${failed === 0 ? "PASS" : "FAIL"} (${passed} passed, ${failed} failed)`);
  await call("quit");
}

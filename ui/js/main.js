// Vault Key's UI: state, actions and rendering. Plain modules, no framework, no build step.
//
// The page keeps a small `state` object. Actions change it (usually after asking the Rust
// side to do something) and then re-render the part of the page that shows it. Nothing is
// written to a save file from here: edits are staged in the app's core, and only the Save
// sheet's confirm button asks the core to write.

import { ApiError, call, setTransport } from "./api.js";
import { renderBackups } from "./views/backups.js";
import { renderGear } from "./views/gear.js";
import { renderHeader, renderReadOnlyNote, renderTabs, tabsFor } from "./views/header.js";
import { renderBlank, renderToasts } from "./views/misc.js";
import { renderOverview } from "./views/overview.js";
import { renderSheet } from "./views/sheets.js";
import { renderSidebar } from "./views/sidebar.js";
import { renderUnlocks } from "./views/unlocks.js";

const params = new URLSearchParams(location.search);

/** How often the status (game running? item names loaded?) is asked for, in milliseconds. */
const STATUS_EVERY = 10_000;
const STATUS_EVERY_WHILE_LOADING = 1_000;
const STATUS_EVERY_WHILE_SAVING = 5_000;
const TOAST_LIFE = 6_000;

/** A small preference kept in the window's own storage. Storage may be unavailable; then
    the default is used. */
function remembered(key) {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function remember(key, value) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Not kept; only a convenience.
  }
}

export const state = {
  status: null,
  characters: [],
  listError: null,
  character: null,
  tab: "gear",
  gearFilter: "all",
  /** Which list the Gear tab shows under the equipped items: "backpack" or "bank". */
  gearView: "backpack",
  /** Whether the Gear tab shows the equipped slots in full or as one line. Remembered. */
  equippedOpen: remembered("gk.equippedOpen") !== "0",
  selectedItem: null,
  /** Backups of the open character; null until loaded. */
  backups: null,
  /** The open sheet: { type: "save" | "restore" | "drop" | "settings", ... } or null. */
  sheet: null,
  toasts: [],
  /** Form values that are not staged edits (the Unlocks tab's inputs). */
  ui: {},
};

const el = {
  sidebar: document.getElementById("sidebar"),
  main: document.getElementById("main"),
  toasts: document.getElementById("toasts"),
  sheet: document.getElementById("sheet"),
  content: null,
};

// Rendering

/** Re-renders `root` with `build()`, keeping keyboard focus (and half-typed text) in place. */
function keepingFocus(root, build) {
  const active = document.activeElement;
  const key = root.contains(active) ? active.dataset?.fk : null;
  const typed = key && active.dataset.dirty ? { value: active.value, start: active.selectionStart, end: active.selectionEnd } : null;
  build();
  if (!key) return;
  const again = root.querySelector(`[data-fk="${CSS.escape(key)}"]`);
  if (!again || again.disabled) return;
  again.focus({ preventScroll: true });
  if (typed) {
    again.value = typed.value;
    again.dataset.dirty = "1";
    try {
      again.setSelectionRange(typed.start, typed.end);
    } catch {
      // Not a text box; nothing to restore.
    }
  }
}

/** Runs `build()`, then puts the scrolling areas matching `selector` back where they were. */
function keepingScroll(root, selector, build) {
  const before = [...root.querySelectorAll(selector)].map((area) => area.scrollTop);
  build();
  root.querySelectorAll(selector).forEach((area, n) => {
    if (before[n]) area.scrollTop = before[n];
  });
}

function renderSide() {
  keepingScroll(el.sidebar, ".char-list", () =>
    keepingFocus(el.sidebar, () => el.sidebar.replaceChildren(...renderSidebar(state, actions).filter(Boolean))),
  );
}

function tabContent() {
  switch (state.tab) {
    case "overview":
      return renderOverview(state, actions);
    case "unlocks":
      return renderUnlocks(state, actions);
    case "backups":
      return renderBackups(state, actions);
    default:
      return renderGear(state, actions);
  }
}

function renderMain() {
  keepingFocus(el.main, () => {
    if (!state.character) {
      el.content = null;
      el.main.replaceChildren(renderBlank(state, actions));
      return;
    }
    // The scrolling area is kept between renders, so it stays where the user scrolled to.
    if (!el.content) {
      el.content = document.createElement("div");
      el.content.className = "content";
      el.content.id = "content";
      el.content.setAttribute("role", "tabpanel");
    }
    el.content.setAttribute("aria-labelledby", `tab-${state.tab}`);
    el.content.replaceChildren(tabContent());
    el.main.replaceChildren(...[renderHeader(state, actions), renderReadOnlyNote(state), renderTabs(state, actions), el.content].filter(Boolean));
  });
}

function renderSheetNow() {
  const content = renderSheet(state, actions);
  if (!content) {
    if (el.sheet.open) el.sheet.close();
    el.sheet.replaceChildren();
    return;
  }
  if (el.sheet.open) {
    keepingScroll(el.sheet, ".sheet-body, .changes", () => keepingFocus(el.sheet, () => el.sheet.replaceChildren(...content)));
  } else {
    el.sheet.replaceChildren(...content);
    el.sheet.showModal();
  }
}

function renderToastsNow() {
  keepingFocus(el.toasts, () => el.toasts.replaceChildren(...renderToasts(state, actions)));
}

function renderAll() {
  renderSide();
  renderMain();
  renderSheetNow();
  renderToastsNow();
}

// Escape closes a sheet, except while a save is being written.
el.sheet.addEventListener("cancel", (event) => {
  event.preventDefault();
  if (state.sheet?.phase !== "working") actions.closeSheet();
});

// Helpers

function messageOf(error) {
  if (error instanceof ApiError) return error.message;
  console.error(error);
  return `Something unexpected went wrong in the window (${error?.message ?? error}).`;
}

let nextToast = 1;

function toast(kind, text) {
  // The same message twice in a row is shown once.
  if (state.toasts.some((t) => t.kind === kind && t.text === text)) return;
  const id = nextToast++;
  state.toasts = [...state.toasts.slice(-3), { id, kind, text }];
  renderToastsNow();
  if (kind !== "error") setTimeout(() => actions.dismissToast(id), TOAST_LIFE);
}

/** Puts a color theme on the page; the default (yellow) has no attribute. */
function applyTheme(theme) {
  if (theme && theme !== "yellow") document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
}

function setCharacter(character, { keepSelection = true } = {}) {
  const changedFile = character?.slotId !== state.character?.slotId;
  // A move changes where items are listed: keep the selection on the same item.
  const was = state.character?.items.find((item) => item.ref === state.selectedItem);
  const followed = was?.uid ? character?.items.find((item) => item.uid === was.uid) : null;
  if (followed && !changedFile) state.selectedItem = followed.ref;
  state.character = character;
  if (changedFile) {
    state.backups = null;
    state.ui = {};
    if (el.content) el.content.scrollTop = 0;
  }
  if (!character || changedFile || !keepSelection || !character.items.some((item) => item.ref === state.selectedItem)) {
    state.selectedItem = null;
  }
}

async function loadBackups() {
  if (!state.character) return;
  const slotId = state.character.slotId;
  try {
    const answer = await call("list_backups", { slotId });
    if (state.character?.slotId !== slotId) return;
    state.backups = answer.backups;
  } catch (error) {
    state.backups = [];
    toast("error", messageOf(error));
  }
  if (state.tab === "backups") renderMain();
}

async function loadCharacters() {
  try {
    const answer = await call("list_characters");
    state.characters = answer.characters;
    state.listError = null;
  } catch (error) {
    state.characters = [];
    state.listError = messageOf(error);
  }
}

let statusTimer = null;

function scheduleStatus() {
  clearTimeout(statusTimer);
  const saving = state.sheet?.type === "save" || state.sheet?.type === "restore";
  const loading = state.status?.itemData?.state === "loading";
  const wait = loading ? STATUS_EVERY_WHILE_LOADING : saving ? STATUS_EVERY_WHILE_SAVING : STATUS_EVERY;
  statusTimer = setTimeout(() => {
    if (document.visibilityState === "hidden") scheduleStatus();
    else actions.refreshStatus(saving);
  }, wait);
}

// Actions: everything the views can ask for.

export const actions = {
  toast,

  dismissToast(id) {
    state.toasts = state.toasts.filter((t) => t.id !== id);
    renderToastsNow();
  },

  refresh() {
    renderAll();
  },

  /** Asks the app how things stand. `fresh` makes it check for the running game right now. */
  async refreshStatus(fresh = false) {
    const before = state.status;
    try {
      state.status = await call("status", { fresh });
      applyTheme(state.status.theme);
    } catch (error) {
      toast("error", messageOf(error));
      scheduleStatus();
      return;
    }
    for (const notice of state.status.notices ?? []) toast("info", notice);
    const namesChanged = before?.itemData?.state !== state.status.itemData?.state;
    if (namesChanged && before && state.character) {
      // Item names have just arrived (or gone): show the items again with them.
      try {
        setCharacter(await call("character"));
      } catch {
        // The character stays as it was shown.
      }
    }
    // Most polls find nothing new; then nothing is redrawn.
    const view = (status) => JSON.stringify([status?.saveFolder, status?.game, status?.itemData]);
    if (view(before) !== view(state.status)) {
      renderSide();
      if (state.sheet) renderSheetNow();
    }
    if (namesChanged || before?.saveFolder?.found !== state.status.saveFolder?.found) renderMain();
    scheduleStatus();
  },

  async retryStart() {
    try {
      state.status = await call("retry_start");
    } catch (error) {
      toast("error", messageOf(error));
    }
    await actions.reload();
  },

  /** Reads the character list again and, if nothing is open, opens the first readable one. */
  async reload() {
    if (state.status?.saveFolder?.found) await loadCharacters();
    if (!state.character) {
      const first = state.characters.find((entry) => entry.readable);
      if (first) await actions.openCharacter(first.slotId, { force: true });
    }
    renderAll();
  },

  async openCharacter(slotId, { force = false } = {}) {
    if (state.character?.slotId === slotId && !force) return true;
    if (!force && (state.character?.pendingCount ?? 0) > 0) {
      state.sheet = { type: "drop", thenOpen: slotId };
      renderSheetNow();
      return false;
    }
    try {
      setCharacter(await call("open_character", { slotId }), { keepSelection: false });
    } catch (error) {
      toast("error", messageOf(error));
      return false;
    }
    renderSide();
    renderMain();
    if (state.tab === "backups") loadBackups();
    return true;
  },

  /**
   * "Import a save to edit…": the user picks a file in the Open dialog, and it is opened as a
   * character from outside the save folder (or as its slot, if it is in the save folder).
   * Vault Key never writes to that file.
   */
  async openFile() {
    let path;
    try {
      path = await call("pick_open_file");
    } catch (error) {
      toast("error", messageOf(error));
      return false;
    }
    if (!path) return false;
    if ((state.character?.pendingCount ?? 0) > 0) {
      state.sheet = { type: "drop", thenOpen: null, thenOpenPath: path };
      renderSheetNow();
      return false;
    }
    return actions.openFilePath(path);
  },

  /** Opens a save file by its full path (the self-test uses this directly). */
  async openFilePath(path) {
    try {
      setCharacter(await call("open_file", { path }), { keepSelection: false });
    } catch (error) {
      toast("error", messageOf(error));
      return false;
    }
    if (!tabsFor(state.character).some((tab) => tab.id === state.tab)) state.tab = "gear";
    renderSide();
    renderMain();
    if (state.tab === "backups") loadBackups();
    return true;
  },

  /**
   * "Export…": like saving, it opens a review sheet first (the changes, and what happens).
   * Its button asks where in the Save dialog, then writes the character there. The changes
   * stay pending.
   */
  openExportSheet() {
    if (!state.character) return;
    state.sheet = { type: "export", phase: "confirm", error: null };
    renderSheetNow();
  },

  /** The export sheet's button: pick where, then write and verify. */
  async confirmExport() {
    const sheet = state.sheet;
    const c = state.character;
    if (sheet?.type !== "export" || sheet.phase === "working" || !c) return null;
    const external = c.source?.kind === "external";
    sheet.phase = "working";
    sheet.error = null;
    renderSheetNow();
    let path;
    try {
      path = await call("pick_export_file", { suggestedName: external ? c.source.fileName : c.fileName, folder: external ? c.source.folder : null });
    } catch (error) {
      path = null;
      sheet.error = messageOf(error);
    }
    if (!path) {
      // Canceled in the Save dialog (or it could not open): back to the review.
      sheet.phase = "confirm";
      renderSheetNow();
      return null;
    }
    return actions.exportTo(path);
  },

  /** Exports the open character to a full path (the self-test uses this directly). */
  async exportTo(path) {
    try {
      const answer = await call("export_file", { path });
      state.sheet = { type: "exported", report: answer.report };
      renderSheetNow();
      return answer;
    } catch (error) {
      if (state.sheet?.type === "export") {
        state.sheet.phase = "confirm";
        state.sheet.error = messageOf(error);
        renderSheetNow();
      } else {
        toast("error", messageOf(error));
      }
      return null;
    }
  },

  /** `focus` moves keyboard focus to the tab (for arrow-key navigation along the tabs). */
  setTab(id, { focus = false } = {}) {
    if (!tabsFor(state.character).some((tab) => tab.id === id)) return Promise.resolve();
    state.tab = id;
    if (el.content) el.content.scrollTop = 0;
    renderMain();
    if (focus) document.getElementById(`tab-${id}`)?.focus({ preventScroll: true });
    return id === "backups" ? loadBackups() : Promise.resolve();
  },

  setEquippedOpen(open) {
    state.equippedOpen = open;
    remember("gk.equippedOpen", open ? "1" : "0");
    renderMain();
  },

  setGearView(id) {
    state.gearView = id;
    renderMain();
  },

  setGearFilter(id) {
    state.gearFilter = id;
    renderMain();
  },

  selectItem(ref) {
    state.selectedItem = ref;
    renderMain();
  },

  /**
   * Stages one edit. Resolves with the answer, or null if it was refused (and says why).
   * `quiet` is for staging while someone is still typing: a refusal is then not shown.
   */
  async stage(edit, { quiet = false } = {}) {
    try {
      const answer = await call("stage", edit);
      setCharacter(answer.character);
      if (!answer.staged && /^(unlock|maximize)/.test(edit.kind)) {
        toast("info", "Your save already has this, so there is nothing to change.");
      }
      renderMain();
      if (state.sheet) renderSheetNow();
      return answer;
    } catch (error) {
      if (quiet) return null;
      toast("error", messageOf(error));
      renderMain();
      return null;
    }
  },

  /** Moves an item from the backpack to the bank, or back. The selection follows it. */
  moveItem(ref, toBank) {
    return actions.stage({ kind: "moveItem", item: ref, toBank });
  },

  async duplicate(ref) {
    try {
      const answer = await call("duplicate_item", { item: ref });
      setCharacter(answer.character);
      renderMain();
      return answer;
    } catch (error) {
      toast("error", messageOf(error));
      return null;
    }
  },

  async removeChange(changeId) {
    try {
      setCharacter(await call("remove_change", { changeId }));
    } catch (error) {
      if (state.sheet) state.sheet.error = messageOf(error);
      else toast("error", messageOf(error));
    }
    renderMain();
    if (state.sheet) renderSheetNow();
  },

  askDiscard() {
    state.sheet = { type: "drop", thenOpen: null };
    renderSheetNow();
  },

  /** The window's close button was pressed: quit, or ask first if changes would be lost. */
  requestClose() {
    const pending = state.character?.pendingCount ?? 0;
    // While a save or restore is being written the sheet stays; the quit request waits its
    // turn behind the write, so nothing is cut short.
    if (pending === 0 || state.sheet?.phase === "working") {
      call("quit").catch(() => {});
      return;
    }
    state.sheet = { type: "drop", thenOpen: null, thenQuit: true };
    renderSheetNow();
  },

  async confirmDrop() {
    const next = state.sheet?.thenOpen ?? null;
    const nextPath = state.sheet?.thenOpenPath ?? null;
    if (state.sheet?.thenQuit) {
      await call("quit").catch(() => {});
      return;
    }
    state.sheet = null;
    try {
      if (nextPath) {
        await actions.openFilePath(nextPath);
      } else if (next) {
        await actions.openCharacter(next, { force: true });
      } else {
        setCharacter(await call("discard_changes"));
      }
    } catch (error) {
      toast("error", messageOf(error));
    }
    renderAll();
  },

  openSaveSheet() {
    const c = state.character;
    if (!c) return;
    const asNew = !c.access.writable;
    // A character from a file goes into the only Steam-ID folder, or the first; the user can
    // pick another when there are several.
    const steamId = c.steamId ?? Object.keys(c.freeSlotsBySteamId ?? {})[0] ?? null;
    const free = c.steamId ? c.freeSlots : (c.freeSlotsBySteamId?.[steamId] ?? []);
    state.sheet = { type: "save", mode: asNew ? "new" : "in-place", steamId, slot: free[0] ?? null, phase: "confirm", error: null, report: null };
    renderSheetNow();
    actions.refreshStatus(true);
  },

  setSaveMode(mode) {
    if (state.sheet?.type !== "save") return;
    state.sheet.mode = mode;
    state.sheet.error = null;
    renderSheetNow();
  },

  pickSteamId(steamId) {
    if (state.sheet?.type !== "save") return;
    state.sheet.steamId = steamId;
    state.sheet.slot = (state.character.freeSlotsBySteamId?.[steamId] ?? [])[0] ?? null;
    renderSheetNow();
  },

  pickSaveSlot(slot) {
    if (state.sheet?.type !== "save") return;
    state.sheet.slot = slot;
    renderSheetNow();
    el.sheet.querySelector(`[data-fk="slot-${slot}"]`)?.focus();
  },

  /** The Save sheet's confirm button: the only place a save is asked for. */
  async confirmSave() {
    const sheet = state.sheet;
    if (sheet?.type !== "save" || sheet.phase === "working") return null;
    sheet.phase = "working";
    sheet.error = null;
    renderSheetNow();
    try {
      const answer = sheet.mode === "new" ? await call("save_as_new", { slot: sheet.slot, steamId: sheet.steamId }) : await call("save");
      // What comes back was read from disk again after the write.
      setCharacter(answer.character);
      state.backups = null;
      sheet.phase = "done";
      sheet.report = answer.report;
      await loadCharacters();
      renderAll();
      if (state.tab === "backups") loadBackups();
      return answer;
    } catch (error) {
      sheet.phase = "confirm";
      sheet.error = messageOf(error);
      renderSheetNow();
      actions.refreshStatus(true);
      return null;
    }
  },

  askRestore(backup) {
    state.sheet = { type: "restore", backup, phase: "confirm", error: null };
    renderSheetNow();
    actions.refreshStatus(true);
  },

  async confirmRestore() {
    const sheet = state.sheet;
    if (sheet?.type !== "restore" || sheet.phase === "working") return null;
    sheet.phase = "working";
    sheet.error = null;
    renderSheetNow();
    try {
      const answer = await call("restore_backup", { backupId: sheet.backup.id });
      if (answer.character) setCharacter(answer.character);
      state.sheet = null;
      await loadCharacters();
      renderAll();
      toast("ok", "Backup restored. The file it replaced is in Backups too.");
      await loadBackups();
      return answer;
    } catch (error) {
      sheet.phase = "confirm";
      sheet.error = messageOf(error);
      renderSheetNow();
      actions.refreshStatus(true);
      return null;
    }
  },

  openSettings() {
    state.sheet = { type: "settings", error: null };
    renderSheetNow();
  },

  async setTestSlot(slotId, marked) {
    try {
      const answer = await call(marked ? "mark_test_slot" : "unmark_test_slot", { slotId });
      state.characters = answer.characters;
      if (answer.character) setCharacter(answer.character);
      if (state.sheet) state.sheet.error = null;
    } catch (error) {
      if (state.sheet) state.sheet.error = messageOf(error);
    }
    renderAll();
  },

  /** The Lock / Unlock button: a locked character is never written; an unlocked one can be saved. */
  async setLocked(slotId, locked) {
    try {
      const answer = await call(locked ? "lock_character" : "unlock_character", { slotId });
      state.characters = answer.characters;
      if (answer.character) setCharacter(answer.character);
      if (state.sheet) state.sheet.error = null;
    } catch (error) {
      if (state.sheet) state.sheet.error = messageOf(error);
      else toast("error", messageOf(error));
    }
    renderAll();
  },

  /** Picks a color theme; Vault Key remembers it in its settings. */
  async setTheme(theme) {
    applyTheme(theme);
    try {
      await call("set_theme", { theme });
      if (state.status) state.status.theme = theme;
    } catch (error) {
      applyTheme(state.status?.theme);
      if (state.sheet) state.sheet.error = messageOf(error);
      else toast("error", messageOf(error));
    }
    renderAll();
  },

  closeSheet() {
    state.sheet = null;
    renderSheetNow();
    scheduleStatus();
  },

  async copyText(text, done) {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // No clipboard permission: fall back to the old way.
      const box = document.createElement("textarea");
      box.value = text;
      box.style.position = "fixed";
      box.style.opacity = "0";
      document.body.append(box);
      box.select();
      const ok = document.execCommand("copy");
      box.remove();
      if (!ok) {
        toast("error", "Windows did not let Vault Key use the clipboard.");
        return;
      }
    }
    toast("ok", done);
  },

  async copyAllCodes() {
    try {
      const answer = await call("export_codes", { includeBank: true });
      if (answer.codes.length === 0) {
        toast("info", "This character carries nothing that has an item code.");
        return;
      }
      await actions.copyText(answer.text, `${answer.codes.length} item codes copied.`);
    } catch (error) {
      toast("error", messageOf(error));
    }
  },
};

// Start-up

/** Loads the browser preview (made-up data; `ui/dev` is not part of the app itself). */
function loadPreview() {
  return new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.type = "module";
    script.src = "dev/preview.js";
    script.onload = () => resolve(window.gkPreview);
    script.onerror = () => reject(new Error("the preview files in ui/dev could not be loaded"));
    document.head.append(script);
  });
}

async function boot() {
  // The app's window calls this when its close button is pressed.
  window.gkCloseRequested = () => actions.requestClose();
  const preview = params.has("mock") ? await loadPreview() : null;
  if (preview) setTransport(preview.call);
  renderAll();
  await actions.refreshStatus();
  await actions.reload();
  // Coming back to the window (from the game, say): look again at what is on disk.
  window.addEventListener("focus", async () => {
    actions.refreshStatus();
    if (!state.sheet && state.status?.saveFolder?.found) {
      await loadCharacters();
      renderSide();
    }
  });

  if (preview) {
    await preview.play({ state, actions, params });
  } else if (params.has("selftest")) {
    const selftest = await import("./selftest.js");
    await selftest.run({ state, actions });
  } else if (params.has("open")) {
    // Development only (GK_UI_OPEN): start with this file open, as "Import a save to edit…" would.
    if (await actions.openFilePath(params.get("open"))) {
      actions.setTab("gear");
      if (params.has("select")) actions.selectItem(params.get("select"));
    }
  } else if (params.has("select") && state.character) {
    // Development only (GK_UI_SELECT): start on the Gear tab with this item selected.
    actions.setTab("gear");
    actions.selectItem(params.get("select"));
  }
}

boot().catch((error) => {
  console.error(error);
  toast("error", messageOf(error));
});

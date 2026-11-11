// Sheets: the modal dialogs. Save (in place, or to a new slot), export, restore a
// backup, drop unsaved changes, and settings (test slots).

import { chip, formatWhen, h, icon, plural, skewButton } from "../dom.js";
import { tierLook } from "./gear.js";

function head(label, title, long = false) {
  return h("div", { class: "sheet-head" }, label ? chip(label) : null, h("h1", { id: "sheet-title", class: long ? "long" : "" }, title));
}

function errorBox(text) {
  if (!text) return null;
  return h("div", { class: "callout bad", role: "alert" }, icon("warn", 18, 2.6), h("span", {}, h("strong", {}, "Not saved. "), text));
}

function step(mark, title, sub, extra) {
  return h("li", { class: "step" }, mark, h("span", { class: "step-text" }, h("span", { class: "step-title" }, title), h("span", { class: "step-sub" }, sub)), extra ?? null);
}

function doneMark() {
  return h("span", { class: "step-mark ok" }, icon("check", 20, 3.4));
}

/** Every save and restore first needs the game closed. Shows the live state. */
function gameStep(state, actions) {
  const game = state.status?.game;
  const again = h("button", { type: "button", class: "btn small", dataset: { fk: "check-game" }, onClick: () => actions.refreshStatus(true) }, "Check again");
  if (game?.state === "not-running") {
    return step(doneMark(), "Game is closed", "Saving is blocked while Borderlands 2 is running.");
  }
  if (game?.state === "running") {
    return step(h("span", { class: "step-mark bad" }, icon("cross", 20, 3.4)), "Game is running", "Quit Borderlands 2 first. Saving is blocked while it runs.", again);
  }
  return step(h("span", { class: "step-mark unsure" }, "?"), "Can't tell if the game is running", "Saving is blocked until Vault Key can check.", again);
}

function changeList(c, actions, busy, { none = "No changes. The copy will be exact.", label = "Changes to save" } = {}) {
  if (c.changes.length === 0) {
    return h("ul", { class: "changes" }, h("li", { class: "change none" }, none));
  }
  return h(
    "ul",
    { class: "changes", "aria-label": label },
    c.changes.map((change) => {
      let what;
      let to;
      // A change to one item names it, in its rarity color: copies look alike.
      const itemName = () => {
        const look = tierLook({ name: change.itemName, tier: change.itemTier, color: change.itemColor });
        return h("span", { class: `item-name ${look.class}`, style: look.style }, change.itemName ?? "an unnamed item");
      };
      if (change.kind === "duplicate") {
        what = h("span", { class: "what" }, "Duplicate ", itemName());
        to = h("span", { class: "to" }, "+1 copy · Backpack");
      } else if ((change.kind === "itemLevel" || change.kind === "moveItem") && change.itemName) {
        what = h("span", { class: "what" }, change.kind === "itemLevel" ? "Level " : "Move ", itemName());
        to =
          change.from !== null && change.from !== undefined
            ? h("span", { class: "to" }, `${change.from} → `, h("b", {}, change.to))
            : h("span", { class: "to" }, h("b", {}, change.to));
      } else if (change.from !== null && change.from !== undefined) {
        what = h("span", { class: "what" }, change.label);
        to = h("span", { class: "to" }, `${change.from} → `, h("b", {}, change.to));
      } else {
        what = h("span", { class: "what" }, change.label);
        to = h("span", { class: "to" }, h("b", {}, change.to));
      }
      return h(
        "li",
        { class: "change" },
        what,
        to,
        h(
          "button",
          { type: "button", class: "btn small", disabled: busy, "aria-label": `Remove this change: ${change.text}`, title: "Remove this change", dataset: { fk: `drop-${change.id}` }, onClick: () => actions.removeChange(change.id) },
          icon("cross", 14, 3.2),
        ),
      );
    }),
  );
}

/** The free slots of the Steam-ID folder the save sheet is aimed at. */
function freeSlotsFor(c, sheet) {
  if (c.source?.kind !== "external") return c.freeSlots;
  return c.freeSlotsBySteamId?.[sheet.steamId] ?? [];
}

/** With more than one Steam account's folder, which one a character from a file goes into. */
function steamPicker(c, sheet, actions, busy) {
  const ids = Object.keys(c.freeSlotsBySteamId ?? {});
  if (c.source?.kind !== "external" || ids.length < 2) return null;
  return h(
    "div",
    { class: "sheet-section" },
    h("h2", { id: "steam-label" }, "Into the saves of"),
    h(
      "div",
      { class: "slots", role: "radiogroup", "aria-labelledby": "steam-label" },
      ids.map((id) =>
        h(
          "button",
          { type: "button", class: "slot-btn wide", role: "radio", "aria-checked": sheet.steamId === id ? "true" : "false", disabled: busy, dataset: { fk: `steam-${id}` }, onClick: () => actions.pickSteamId(id) },
          "Steam",
          h("small", {}, id),
        ),
      ),
    ),
  );
}

function slotPicker(c, sheet, actions, busy) {
  const free = freeSlotsFor(c, sheet);
  return h(
    "div",
    { class: "sheet-section" },
    h("h2", { id: "slot-label" }, "Save into slot"),
    h(
      "div",
      { class: "slots", role: "radiogroup", "aria-labelledby": "slot-label" },
      [2, 3, 4, 5, 6, 7, 8, 9].map((n) => {
        const isFree = free.includes(n);
        return h(
          "button",
          {
            type: "button",
            class: "slot-btn",
            role: "radio",
            "aria-checked": sheet.slot === n ? "true" : "false",
            "aria-label": isFree ? `Slot ${n}, free` : `Slot ${n}, taken`,
            disabled: !isFree || busy,
            dataset: { fk: `slot-${n}` },
            onClick: () => actions.pickSaveSlot(n),
          },
          n,
          h("small", {}, isFree ? "free" : "taken"),
        );
      }),
    ),
    free.length === 0 ? h("p", { class: "note" }, "Slots 2 to 9 are all taken. Delete a character in the game to free one.") : null,
  );
}

function saveSheet(state, actions) {
  const c = state.character;
  const sheet = state.sheet;
  const busy = sheet.phase === "working";
  const asNew = sheet.mode === "new";
  const writable = c.access.writable;
  const count = c.pendingCount;
  const external = c.source?.kind === "external";
  const label = `${c.slot ? `Slot ${c.slot}` : c.fileName} · ${c.current.name}`;
  const gameOk = state.status?.game?.state === "not-running";

  if (sheet.phase === "done") return savedSheet(state, actions);

  const title = asNew ? "Save to a new slot?" : `Save ${plural(count, "change", "changes")}?`;
  // In plain words what saving does; the only status row is the live check on the game.
  const what = asNew
    ? `It goes into slot ${sheet.slot ?? "…"}. ${external ? `${c.source.fileName} isn't changed.` : "Your original isn't changed."}`
    : "Your current save is backed up first. You can restore it from Backups.";

  const canGo = !busy && gameOk && (asNew ? sheet.slot !== null : count > 0);
  const goLabel = busy ? "Saving…" : asNew ? (sheet.slot ? `Save to slot ${sheet.slot}` : "Save to new slot") : "Back up & save";

  let switcher = null;
  if (writable && !busy) {
    switcher = asNew
      ? h("button", { type: "button", class: "link", dataset: { fk: "mode" }, onClick: () => actions.setSaveMode("in-place") }, "Save over this character instead")
      : h("button", { type: "button", class: "link", dataset: { fk: "mode" }, onClick: () => actions.setSaveMode("new") }, "Save to a new slot instead");
  }

  return [
    h("div", { class: "hazard" }),
    h(
      "div",
      { class: "sheet-body" },
      head(label, title, asNew),
      changeList(c, actions, busy),
      asNew ? steamPicker(c, sheet, actions, busy) : null,
      asNew ? slotPicker(c, sheet, actions, busy) : null,
      h("ul", { class: "steps" }, gameStep(state, actions)),
      h("p", { class: "note" }, what),
      errorBox(sheet.error),
      h(
        "div",
        { class: "sheet-actions" },
        switcher,
        skewButton({ class: "btn big", disabled: busy, autofocus: !canGo, dataset: { fk: "cancel" }, onClick: () => actions.closeSheet() }, "Cancel"),
        skewButton({ class: "btn hero", id: "sheet-go", disabled: !canGo, autofocus: canGo, dataset: { fk: "go" }, onClick: () => actions.confirmSave() }, goLabel),
      ),
    ),
  ];
}

function savedSheet(state, actions) {
  const sheet = state.sheet;
  const report = sheet.report;
  const lines = report.createdNew
    ? [
        step(doneMark(), `Saved to slot ${report.slot}`, `${report.fileName} was written and verified.`),
        step(doneMark(), "Your original is untouched", "Nothing was replaced."),
        step(doneMark(), `You're now editing slot ${report.slot}`, "Vault Key made it, so you can save it directly from now on."),
      ]
    : [
        step(doneMark(), "Backed up", report.backup ? `The old file is in Backups (${formatWhen(report.backup.createdUnix)}).` : "There was nothing to back up."),
        step(doneMark(), "Written & verified", `${report.fileName} was read back and checked.`),
        step(doneMark(), "Swapped in", "Start the game when you are ready."),
      ];
  return [
    h("div", { class: "hazard", style: { "--stripe": "var(--good)" } }),
    h(
      "div",
      { class: "sheet-body" },
      head(`Slot ${report.slot} · ${state.character?.current.name ?? ""}`, "Saved!"),
      h("ol", { class: "steps" }, lines),
      report.warning ? h("div", { class: "callout" }, icon("warn", 18, 2.6), h("span", {}, report.warning)) : null,
      h("div", { class: "sheet-actions" }, skewButton({ class: "btn hero", autofocus: true, dataset: { fk: "done" }, onClick: () => actions.closeSheet() }, "Done")),
    ),
  ];
}

/** Before an export: the changes, as for a save, with Export as the button. The Save dialog
 *  opens from there. */
function exportSheet(state, actions) {
  const c = state.character;
  const sheet = state.sheet;
  const busy = sheet.phase === "working";
  const count = c.pendingCount;
  const external = c.source?.kind === "external";
  const label = `${c.slot ? `Slot ${c.slot}` : c.fileName} · ${c.current.name}`;
  const title = count > 0 ? `Export ${plural(count, "change", "changes")}?` : "Export a copy?";
  return [
    h("div", { class: "hazard" }),
    h(
      "div",
      { class: "sheet-body" },
      head(label, title),
      changeList(c, actions, busy, { none: "No changes. The file will be an exact copy of this character.", label: "Changes to export" }),
      h(
        "p",
        { class: "note" },
        external ? `You pick where next. It starts on ${c.source.fileName}, in the folder you imported it from.` : "You pick where next.",
        " If you replace a file, Vault Key keeps a copy of the old one.",
      ),
      sheet.error ? h("div", { class: "callout bad", role: "alert" }, icon("warn", 18, 2.6), h("span", {}, h("strong", {}, "Not exported. "), sheet.error)) : null,
      h(
        "div",
        { class: "sheet-actions" },
        skewButton({ class: "btn big", disabled: busy, dataset: { fk: "cancel" }, onClick: () => actions.closeSheet() }, "Cancel"),
        skewButton({ class: "btn hero", id: "sheet-go", disabled: busy, autofocus: true, dataset: { fk: "go" }, onClick: () => actions.confirmExport() }, busy ? "Exporting…" : "Export…"),
      ),
    ),
  ];
}

/** After an export: where it went, and where save files go to be played. */
function exportedSheet(state, actions) {
  const report = state.sheet.report;
  const pending = state.character?.pendingCount ?? 0;
  return [
    h("div", { class: "hazard", style: { "--stripe": "var(--good)" } }),
    h(
      "div",
      { class: "sheet-body" },
      head(`${report.name} · ${report.classLabel} lvl ${report.level}`, "Exported!"),
      h(
        "dl",
        { class: "kv" },
        h("dt", {}, "Written to"),
        h("dd", { class: "path selectable", id: "export-path" }, report.path),
        h("dt", {}, "Save-game ID"),
        h("dd", {}, `${report.saveGameId} (the game's own name for that slot is Save${String(report.saveGameId).padStart(4, "0")}.sav)`),
        report.replacedBackup ? h("dt", {}, "Replaced file") : null,
        report.replacedBackup ? h("dd", { class: "path selectable" }, `Kept a copy of what was there at ${report.replacedBackup}`) : null,
      ),
      h(
        "ol",
        { class: "steps" },
        step(doneMark(), "Written & verified", `${report.fileName} was read back and checked.`),
        step(
          doneMark(),
          pending > 0 ? "Your changes are still here, unsaved" : "Nothing else was changed",
          pending > 0
            ? state.character?.source?.kind === "external"
              ? "Change more and export again, or add it to your own characters."
              : "Exporting does not save them anywhere else. Save them too if you want them in your own save."
            : "The character you have open is as it was.",
        ),
      ),
      h(
        "div",
        { class: "callout", role: "note" },
        icon("key", 18, 2.6),
        h(
          "span",
          {},
          h("strong", {}, "Where save files go: "),
          h("span", { class: "selectable path-inline" }, "Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData\\<Steam ID>"),
          ", under the same name, with the game closed. If Steam Cloud asks which save to keep, choose the local files.",
        ),
      ),
      h("div", { class: "sheet-actions" }, skewButton({ class: "btn hero", autofocus: true, dataset: { fk: "done" }, onClick: () => actions.closeSheet() }, "Done")),
    ),
  ];
}

function restoreSheet(state, actions) {
  const sheet = state.sheet;
  const backup = sheet.backup;
  const c = state.character;
  const busy = sheet.phase === "working";
  const pending = c.pendingCount;
  return [
    h("div", { class: "hazard" }),
    h(
      "div",
      { class: "sheet-body" },
      head(`Slot ${c.slot} · ${c.current.name}`, "Restore this backup?", true),
      h(
        "dl",
        { class: "kv" },
        h("dt", {}, "Taken"),
        h("dd", {}, formatWhen(backup.createdUnix)),
        h("dt", {}, "Why"),
        h("dd", {}, backup.reason || "—"),
        h("dt", {}, "Holds"),
        h("dd", {}, backup.characterName ? `${backup.characterName}, level ${backup.level}` : "Not a readable save"),
      ),
      h("ul", { class: "steps" }, gameStep(state, actions)),
      h("p", { class: "note" }, "What's there now is backed up first, so you can undo this."),
      pending > 0 ? h("div", { class: "callout" }, icon("warn", 18, 2.6), h("span", {}, `Your ${plural(pending, "unsaved change", "unsaved changes")} will be dropped.`)) : null,
      sheet.error ? h("div", { class: "callout bad", role: "alert" }, icon("warn", 18, 2.6), h("span", {}, h("strong", {}, "Not restored. "), sheet.error)) : null,
      h(
        "div",
        { class: "sheet-actions" },
        skewButton({ class: "btn big", disabled: busy, autofocus: true, dataset: { fk: "cancel" }, onClick: () => actions.closeSheet() }, "Cancel"),
        skewButton({ class: "btn hero", disabled: busy || state.status?.game?.state !== "not-running", dataset: { fk: "go" }, onClick: () => actions.confirmRestore() }, busy ? "Restoring…" : "Restore"),
      ),
    ),
  ];
}

function dropSheet(state, actions) {
  const sheet = state.sheet;
  const count = state.character?.pendingCount ?? 0;
  const switching = Boolean(sheet.thenOpen);
  const quitting = Boolean(sheet.thenQuit);
  const changes = plural(count, "unsaved change", "unsaved changes");
  const why = quitting
    ? "Closing Vault Key drops the changes you have not saved. Your save file is not affected."
    : switching
      ? "Opening another character drops the changes you have not saved. Your save file is not affected."
      : "This removes every change you have not saved. Your save file is not affected.";
  return [
    h("div", { class: "hazard" }),
    h(
      "div",
      { class: "sheet-body" },
      head(null, quitting ? `Quit with ${changes}?` : `Drop ${changes}?`, true),
      h("p", { class: "note" }, why),
      h(
        "div",
        { class: "sheet-actions" },
        skewButton({ class: "btn big", autofocus: true, dataset: { fk: "cancel" }, onClick: () => actions.closeSheet() }, quitting ? "Keep editing" : switching ? "Stay here" : "Keep them"),
        skewButton({ class: "btn hero", dataset: { fk: "go" }, onClick: () => actions.confirmDrop() }, quitting ? "Quit without saving" : switching ? "Drop & switch" : "Drop changes"),
      ),
    ),
  ];
}

function settingsSheet(state, actions) {
  const status = state.status;
  const rows = state.characters
    .filter((entry) => entry.slot !== null)
    .map((entry) => {
      const name = entry.readable ? `${entry.name} · ${entry.classLabel} lvl ${entry.level}` : `${entry.fileName} · can't be read`;
      const open = entry.readable && entry.access.writable;
      let side;
      if (!entry.readable) {
        side = h("span", { class: "state off" }, "Can't be read");
      } else if (open) {
        side = [
          h("span", { class: "state" }, icon("unlock", 14, 3), "Unlocked"),
          h("button", { type: "button", class: "btn small", dataset: { fk: `lock-${entry.slotId}` }, onClick: () => actions.setLocked(entry.slotId, true) }, "Lock"),
        ];
      } else {
        side = [
          h("span", { class: "state off" }, icon("lock", 14, 3), "Locked"),
          h("button", { type: "button", class: "btn small yellow", dataset: { fk: `unlock-${entry.slotId}` }, onClick: () => actions.setLocked(entry.slotId, false) }, "Unlock"),
        ];
      }
      return h(
        "div",
        { class: `row ${open ? "staged" : ""}`, role: "listitem" },
        h("div", { class: "row-text" }, h("span", { class: "row-title" }, `Slot ${entry.slot}`), h("span", { class: "row-sub" }, name)),
        h("div", { class: "row-actions" }, side),
      );
    });
  const itemData = status?.itemData;
  return [
    h("div", { class: "hazard" }),
    h(
      "div",
      { class: "sheet-body" },
      head(null, "Settings"),
      h(
        "div",
        { class: "sheet-section" },
        h("h2", {}, "Locks"),
        h("p", { class: "note" }, "Vault Key never writes to a locked character. Unlock one to save into it; its file is backed up first. A different character that later turns up in the same slot starts out locked."),
        rows.length > 0 ? h("div", { class: "rows", role: "list" }, rows) : h("p", { class: "note dim" }, "There are no characters here yet."),
        sheet_error(state),
      ),
      h(
        "div",
        { class: "sheet-section" },
        h("h2", {}, "Colors"),
        h(
          "div",
          { class: "swatches", role: "radiogroup", "aria-label": "Color theme" },
          [
            // The accent, and the background it comes with.
            ["yellow", "Yellow", "#FFC72C", "#1B140D"],
            ["blue", "Blue", "#3FB7FF", "#132C4A"],
            ["purple", "Purple", "#B97CFF", "#24183D"],
            ["red", "Red", "#E8384F", "#22272E"],
          ].map(([id, label, color, ground]) =>
            h(
              "button",
              {
                type: "button",
                class: "swatch",
                role: "radio",
                "aria-checked": (status?.theme ?? "yellow") === id ? "true" : "false",
                style: { "--swatch": color, "--swatch-ground": ground },
                dataset: { fk: `theme-${id}` },
                onClick: () => actions.setTheme(id),
              },
              h("span", { class: "swatch-chip" }),
              label,
            ),
          ),
        ),
      ),
      h(
        "div",
        { class: "sheet-section" },
        h("h2", {}, "Where things are"),
        h(
          "dl",
          { class: "kv" },
          h("dt", {}, "Game saves"),
          h("dd", { class: "path selectable" }, status?.saveFolder?.path ?? "Not found"),
          h("dt", {}, "Backups & settings"),
          h("dd", { class: "path selectable" }, status?.saveFolder?.dataDir ?? "—"),
          h("dt", {}, "Error log"),
          h("dd", { class: "path selectable" }, status?.logFile ?? "—"),
          h("dt", {}, "Item names"),
          h("dd", {}, itemData ? (itemData.state === "loading" ? `${itemData.message} ${itemData.percent ?? 0}%` : itemData.message) : "—"),
          h("dt", {}, "Version"),
          h("dd", {}, status?.version ?? "—"),
        ),
      ),
      h("div", { class: "sheet-actions" }, skewButton({ class: "btn big", autofocus: true, dataset: { fk: "done" }, onClick: () => actions.closeSheet() }, "Close")),
    ),
  ];
}

function sheet_error(state) {
  const text = state.sheet?.error;
  return text ? h("div", { class: "callout bad", role: "alert" }, icon("warn", 18, 2.6), h("span", {}, text)) : null;
}

function build(state, actions) {
  switch (state.sheet?.type) {
    case "save":
      return saveSheet(state, actions);
    case "restore":
      return restoreSheet(state, actions);
    case "drop":
      return dropSheet(state, actions);
    case "settings":
      return settingsSheet(state, actions);
    case "export":
      return exportSheet(state, actions);
    case "exported":
      return exportedSheet(state, actions);
    default:
      return null;
  }
}

/** The content of the open sheet, or null when none is open. */
export function renderSheet(state, actions) {
  const parts = build(state, actions);
  if (!parts) return null;
  // The buttons stay in view below the part that scrolls.
  const buttons = parts[1].querySelector(":scope > .sheet-actions");
  if (buttons) parts.push(buttons);
  return parts;
}

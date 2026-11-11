// The sidebar: app name, characters by slot, game status, settings.

import { chip, h, icon } from "../dom.js";

function characterRow(entry, state, actions) {
  const slot = entry.slot ?? "?";
  if (!entry.readable) {
    return h(
      "div",
      { class: "char broken", role: "listitem", title: entry.problem },
      h("span", { class: "char-slot" }, slot),
      h(
        "span",
        { class: "char-text" },
        h("span", { class: "char-name" }, entry.fileName),
        h("span", { class: "char-sub" }, "Can't be read, so it can't be opened."),
      ),
    );
  }
  const access = entry.access;
  const current = state.character?.slotId === entry.slotId;
  const tag = access.writable
    ? h("span", { class: "char-tag ok" }, "Unlocked")
    : h("span", { class: "char-tag" }, "Locked");
  const label =
    `Slot ${slot}: ${entry.name}, ${entry.classLabel} level ${entry.level}. ` +
    (access.writable ? "Unlocked: can be saved." : `Locked. ${access.explanation}`);
  const who = entry.name || entry.fileName;
  // Locking is offered for every character; unlocking only where Vault Key could then save.
  const lock =
    entry.slot === null && !access.writable
      ? null
      : h(
          "button",
          {
            type: "button",
            class: `char-lock ${access.writable ? "open" : ""}`,
            "aria-label": access.writable ? `Lock ${who}` : `Unlock ${who}`,
            title: access.writable
              ? "Unlocked: Vault Key can save changes into this file. Click to lock it."
              : "Locked: Vault Key never changes this file. Click to unlock it.",
            dataset: { fk: `lock-${entry.slotId}` },
            onClick: () => actions.setLocked(entry.slotId, access.writable),
          },
          icon(access.writable ? "unlock" : "lock", 18, 2.8),
        );
  const open = h(
    "button",
    {
      type: "button",
      class: `char ${access.writable ? "" : "locked"}`,
      "aria-current": current ? "true" : null,
      "aria-label": label,
      title: access.explanation,
      dataset: { fk: `char-${entry.slotId}`, slot: String(slot) },
      onClick: () => actions.openCharacter(entry.slotId),
    },
    h("span", { class: "char-slot" }, slot),
    h(
      "span",
      { class: "char-text" },
      h("span", { class: "char-name" }, entry.name || entry.fileName),
      h("span", { class: "char-sub" }, `Lvl ${entry.level} · ${entry.classLabel}`),
      tag,
    ),
  );
  return h("div", { class: "char-row", role: "listitem" }, open, lock);
}

function gameStatus(status) {
  const game = status?.game;
  if (!game) return null;
  const look = { "not-running": "ok", running: "bad", unknown: "unsure" }[game.state] ?? "unsure";
  const text = {
    "not-running": "Game closed · Safe to save",
    running: "Game running · Can't save",
    unknown: "Can't tell if the game is running",
  }[game.state];
  return h("div", { class: `game-status ${look}`, role: "status", title: game.message }, h("span", { class: "lamp" }), text);
}

function itemDataNote(status) {
  const data = status?.itemData;
  if (!data || data.state === "ready") return null;
  if (data.state === "loading") {
    return h("div", { class: "side-note", title: data.message }, `Reading item names… ${data.percent ?? 0}%`);
  }
  return h("div", { class: "side-note", title: data.message }, "Item names unavailable");
}

/** The character opened from a file outside the save folder, while it is open. */
function fileRow(state) {
  const c = state.character;
  if (c?.source?.kind !== "external") return null;
  return h(
    "div",
    { class: "char file", role: "listitem", "aria-current": "true", title: c.source.path },
    h("span", { class: "char-slot" }, icon("file", 22, 2.6)),
    h(
      "span",
      { class: "char-text" },
      h("span", { class: "char-name" }, c.current.name || c.fileName),
      h("span", { class: "char-sub" }, `Lvl ${c.current.level} · ${c.current.classLabel}`),
      h("span", { class: "char-tag" }, `From ${c.source.fileName}`),
    ),
  );
}

export function renderSidebar(state, actions) {
  const ids = [...new Set(state.characters.map((c) => c.steamId))];
  const rows = [];
  const file = fileRow(state);
  if (file) rows.push(h("div", { class: "steam-id" }, "Imported"), file);
  for (const id of ids) {
    if (ids.length > 1) rows.push(h("div", { class: "steam-id" }, `Steam ${id}`));
    for (const entry of state.characters.filter((c) => c.steamId === id)) rows.push(characterRow(entry, state, actions));
  }
  return [
    h(
      "div",
      { class: "brand" },
      h("span", { class: "brand-mark" }, icon("key", 24, 2.6)),
      h("span", { class: "brand-name" }, "Vault Key"),
    ),
    chip("Characters", { class: "yellow" }),
    rows.length > 0
      ? h("div", { class: "char-list", role: "list" }, rows)
      : h("p", { class: "note dim" }, state.status?.saveFolder?.found ? "No characters yet." : ""),
    h(
      "div",
      { class: "side-foot" },
      itemDataNote(state.status),
      gameStatus(state.status),
      h(
        "div",
        { class: "side-row" },
        h(
          "button",
          { type: "button", class: "btn side-btn", id: "open-file-button", title: "Open a save file from anywhere to edit it. Vault Key changes it only when you export over it.", dataset: { fk: "open-file" }, onClick: () => actions.openFile() },
          icon("file", 16, 2.4),
          "Import a save to edit…",
        ),
      ),
      h(
        "div",
        { class: "side-row" },
        h(
          "button",
          { type: "button", class: "btn side-btn", dataset: { fk: "settings" }, onClick: () => actions.openSettings() },
          icon("gear", 16, 2.4),
          "Settings",
        ),
      ),
    ),
  ];
}

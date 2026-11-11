// The open character's header (name, level, unsaved count, Save) and the section tabs.

import { chip, h, icon, plural, skewButton } from "../dom.js";

export const TABS = [
  { id: "overview", label: "Overview" },
  { id: "gear", label: "Gear" },
  { id: "unlocks", label: "Unlocks" },
  { id: "backups", label: "Backups" },
];

/** True for a character opened from a file outside the save folder. */
export function isExternal(character) {
  return character?.source?.kind === "external";
}

/** The tabs a character has: one from a file has no slot, so no backups to restore. */
export function tabsFor(character) {
  return isExternal(character) ? TABS.filter((tab) => tab.id !== "backups") : TABS;
}

export function renderHeader(state, actions) {
  const c = state.character;
  const now = c.current;
  const writable = c.access.writable;
  const pending = c.pendingCount;
  const external = isExternal(c);
  const slotText = c.slot ? `Slot ${c.slot}` : c.fileName;

  const chips = [chip(`${slotText} · ${now.classLabel}`)];
  if (external) {
    chips.push(chip("Imported", { class: "lock", title: c.access.explanation, mark: icon("file", 13, 3) }));
  } else {
    chips.push(
      writable
        ? chip("Unlocked", { title: c.access.explanation, mark: icon("unlock", 13, 3) })
        : chip("Locked", { class: "lock", title: c.access.explanation, mark: icon("lock", 13, 3) }),
    );
    if (c.slot !== null || writable) {
      chips.push(
        h(
          "button",
          {
            type: "button",
            class: "btn small",
            id: "lock-button",
            title: writable ? "Lock this character: Vault Key will never change its file." : "Unlock this character: Vault Key may then save changes into its file (it backs the file up first).",
            dataset: { fk: "lock" },
            onClick: () => actions.setLocked(c.slotId, writable),
          },
          icon(writable ? "lock" : "unlock", 14, 3),
          writable ? "Lock" : "Unlock",
        ),
      );
    }
  }

  // One workflow either way: the big button opens a review of the changes. For your own
  // character it saves; for an imported file it exports.
  const saveLabel = external ? "Export…" : writable ? "Save" : "Save to new slot…";
  const canSave = external || !writable || pending > 0;
  const saveTitle = external
    ? "Review your changes, then write this character out to a file: back over the one you imported, or anywhere else."
    : writable
      ? pending > 0
        ? "Review your changes and save them."
        : "There is nothing to save yet."
      : "This character is locked. Save a copy, with your changes, to a new slot.";
  const exportTitle =
    pending > 0
      ? "Review, then write this character, with your unsaved changes, to a file of your choosing. Your changes stay here, unsaved."
      : "Review, then write this character to a file of your choosing.";
  const onBig = external ? () => actions.openExportSheet() : () => actions.openSaveSheet();

  return h(
    "header",
    { class: "char-head" },
    h(
      "div",
      { class: "char-title" },
      h("div", { class: "char-chips" }, chips),
      h("h1", { title: `${now.name}, level ${now.level}` }, `${now.name} `, h("span", { class: "lvl" }, `Lvl ${now.level}`)),
    ),
    h(
      "div",
      { class: "unsaved" },
      h(
        "span",
        { class: `unsaved-count ${pending === 0 ? "none" : ""}`, id: "unsaved-count", role: "status" },
        pending === 0 ? "No unsaved changes" : plural(pending, "unsaved change", "unsaved changes"),
      ),
      // An imported file exports with the big button; saving it to a new slot of your own and
      // discarding are real buttons beside it. For a character of your own they stay quiet links.
      external
        ? h(
            "div",
            { class: "head-actions" },
            pending > 0
              ? h("button", { type: "button", class: "btn small danger", dataset: { fk: "discard" }, onClick: () => actions.askDiscard() }, "Discard")
              : null,
            h(
              "button",
              { type: "button", class: "btn small", id: "add-button", title: "Save this character, with your changes, to a free slot in your own save folder.", dataset: { fk: "add" }, onClick: () => actions.openSaveSheet() },
              "Save to new slot…",
            ),
          )
        : [
            pending > 0
              ? h("button", { type: "button", class: "link", dataset: { fk: "discard" }, onClick: () => actions.askDiscard() }, "Discard them")
              : null,
            h("button", { type: "button", class: "link", id: "export-button", title: exportTitle, dataset: { fk: "export" }, onClick: () => actions.openExportSheet() }, "Export to file…"),
          ],
    ),
    skewButton(
      { class: "save-btn", id: "save-button", disabled: !canSave, title: saveTitle, dataset: { fk: "save" }, onClick: onBig },
      saveLabel,
    ),
  );
}

export function renderTabs(state, actions) {
  const select = (id, focus = false) => actions.setTab(id, { focus });
  const tabs = tabsFor(state.character);
  const onKey = (event) => {
    const at = tabs.findIndex((t) => t.id === state.tab);
    let next = null;
    if (event.key === "ArrowRight") next = tabs[(at + 1) % tabs.length];
    if (event.key === "ArrowLeft") next = tabs[(at + tabs.length - 1) % tabs.length];
    if (event.key === "Home") next = tabs[0];
    if (event.key === "End") next = tabs[tabs.length - 1];
    if (next) {
      event.preventDefault();
      select(next.id, true);
    }
  };
  return h(
    "div",
    { class: "tabs", role: "tablist", "aria-label": "Character sections", onKeydown: onKey },
    tabs.map((tab) =>
      skewButton(
        {
          class: "tab",
          role: "tab",
          id: `tab-${tab.id}`,
          "aria-selected": state.tab === tab.id ? "true" : "false",
          "aria-controls": "content",
          tabindex: state.tab === tab.id ? "0" : "-1",
          dataset: { fk: `tab-${tab.id}` },
          onClick: () => select(tab.id),
        },
        tab.label,
      ),
    ),
  );
}

/** The one-line explanation shown under the header of a read-only character. */
export function renderReadOnlyNote(state) {
  const c = state.character;
  if (c.access.writable) return null;
  if (isExternal(c)) {
    return h(
      "div",
      { class: "callout", role: "note" },
      icon("file", 18, 2.6),
      h(
        "span",
        {},
        h("strong", {}, "Imported. "),
        h("span", { class: "selectable path-inline" }, c.source.path),
        ". Export writes your changes back to it, and the old file is backed up first.",
      ),
    );
  }
  return h(
    "div",
    { class: "callout", role: "note" },
    icon("lock", 18, 2.6),
    h("span", {}, h("strong", {}, "Locked. "), c.access.explanation),
  );
}

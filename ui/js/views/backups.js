// Backups: the copies Vault Key took of this character before saving over it.

import { formatWhen, h, icon } from "../dom.js";

export function renderBackups(state, actions) {
  const c = state.character;
  const backups = state.backups;
  const head = h(
    "div",
    { class: "panel-head" },
    h("h2", { id: "b-title" }, "Backups"),
    backups ? h("span", { class: "side-note" }, backups.length === 1 ? "1 backup" : `${backups.length} backups`) : null,
  );
  let body;
  if (backups === null) {
    body = h("p", { class: "note dim" }, "Looking for backups…");
  } else if (backups.length === 0) {
    body = [
      h("p", { class: "note" }, "No backups of this character yet."),
      h("p", { class: "note dim" }, "Vault Key makes one each time it saves over a character, before anything is changed."),
    ];
  } else {
    body = [
      h("p", { class: "note" }, "Each backup is the whole character as it was just before a save. Restoring puts it back, and backs up what is there now first."),
      c.access.writable ? null : h("div", { class: "callout" }, icon("lock", 18, 2.6), h("span", {}, "This character is read-only, so a backup can't be restored over it.")),
      h(
        "div",
        { class: "rows", role: "list" },
        backups.map((backup) =>
          h(
            "div",
            { class: "row backup", role: "listitem" },
            icon("clock", 22, 2.4),
            h(
              "div",
              { class: "row-text" },
              h("span", { class: "row-title when" }, formatWhen(backup.createdUnix)),
              h(
                "span",
                { class: "row-sub" },
                [backup.reason, backup.characterName ? `${backup.characterName}, level ${backup.level}` : "Not a readable save"].filter(Boolean).join(" · "),
              ),
            ),
            h(
              "div",
              { class: "row-actions" },
              h(
                "button",
                { type: "button", class: "btn small yellow", disabled: !c.access.writable, dataset: { fk: `restore-${backup.id}` }, onClick: () => actions.askRestore(backup) },
                "Restore",
              ),
            ),
          ),
        ),
      ),
    ];
  }
  return h("div", { class: "panels" }, h("section", { class: "panel span", "aria-labelledby": "b-title" }, head, h("div", { class: "panel-body" }, body)));
}

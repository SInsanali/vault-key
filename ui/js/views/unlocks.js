// Unlocks: game modes, the Slaughter Dome, ammo upgrades, item levels and item codes.

import { h, icon } from "../dom.js";

function changeOf(c, kind) {
  return c.changes.find((change) => change.kind === kind) ?? null;
}

/** One unlock: already unlocked, staged (with Undo), or a button to stage it. */
function unlockRow(c, actions, { kind, title, sub, unlocked, impliedBy }) {
  const pending = changeOf(c, kind);
  let side;
  if (unlocked) {
    side = h("span", { class: "state" }, icon("check", 16, 3.4), "Unlocked");
  } else if (pending) {
    side = [
      h("span", { class: "state pending" }, "Unlocks on save"),
      h("button", { type: "button", class: "btn small", dataset: { fk: `undo-${kind}` }, onClick: () => actions.removeChange(pending.id) }, "Undo"),
    ];
  } else if (impliedBy) {
    side = h("span", { class: "state pending" }, impliedBy);
  } else {
    side = h("button", { type: "button", class: "btn small yellow", dataset: { fk: `do-${kind}` }, onClick: () => actions.stage({ kind }) }, "Unlock");
  }
  return h(
    "div",
    { class: `row ${pending ? "staged" : ""}` },
    h("div", { class: "row-text" }, h("span", { class: "row-title" }, title), h("span", { class: "row-sub" }, sub)),
    h("div", { class: "row-actions" }, side),
  );
}

function panel(title, id, ...body) {
  return h(
    "section",
    { class: "panel", "aria-labelledby": id },
    h("div", { class: "panel-head" }, h("h2", { id }, title)),
    h("div", { class: "panel-body" }, ...body),
  );
}

function itemLevels(c, actions, ui) {
  const pending = changeOf(c, "itemLevels");
  const [min, max] = c.limits.itemLevel;
  const levelBox = h("input", {
    class: "input short",
    type: "number",
    min,
    max,
    value: ui.itemLevel ?? c.current.level,
    "aria-label": "Item level",
    dataset: { fk: "il-level" },
    onInput: () => {
      ui.itemLevel = levelBox.value;
      ui.itemLevelMode = "fixed";
      fixed.checked = true;
    },
  });
  const mode = ui.itemLevelMode ?? "character";
  const same = h("input", { type: "radio", name: "il-mode", checked: mode === "character", dataset: { fk: "il-same" }, onChange: () => (ui.itemLevelMode = "character") });
  const fixed = h("input", { type: "radio", name: "il-mode", checked: mode === "fixed", dataset: { fk: "il-fixed" }, onChange: () => (ui.itemLevelMode = "fixed") });
  const force = h("input", { type: "checkbox", checked: Boolean(ui.itemLevelForce), dataset: { fk: "il-force" }, onChange: () => (ui.itemLevelForce = force.checked) });
  const apply = () => {
    const level = fixed.checked ? Number(levelBox.value) : 0;
    if (fixed.checked && !Number.isInteger(level)) {
      actions.toast("error", "Item level: enter a whole number.");
      return;
    }
    actions.stage({ kind: "itemLevels", level, force: force.checked });
  };
  return panel(
    "Item levels",
    "u-levels",
    h("p", { class: "note" }, "Sets the level of every weapon and item you carry. Items in the bank stay as they are."),
    h("label", { class: "radio" }, same, `Match the character (level ${c.current.level})`),
    h("div", { class: "inline" }, h("label", { class: "radio" }, fixed, "Set to level"), levelBox),
    h("label", { class: "check" }, force, "Include level 1 items"),
    pending
      ? h(
          "div",
          { class: "rows" },
          h(
            "div",
            { class: "row staged" },
            h("div", { class: "row-text" }, h("span", { class: "row-title" }, "Staged"), h("span", { class: "row-sub" }, pending.to)),
            h("div", { class: "row-actions" }, h("button", { type: "button", class: "btn small", dataset: { fk: "undo-itemLevels" }, onClick: () => actions.removeChange(pending.id) }, "Undo")),
          ),
        )
      : null,
    h("button", { type: "button", class: "btn big", dataset: { fk: "il-apply" }, onClick: apply }, pending ? "Change item levels" : "Set item levels"),
  );
}

function itemCodes(c, actions, ui) {
  const box = h("textarea", {
    class: "input",
    rows: "4",
    spellcheck: "false",
    placeholder: "BL2(…)",
    "aria-label": "Item codes to import",
    value: ui.importText ?? "",
    dataset: { fk: "codes-box" },
    onInput: () => {
      ui.importText = box.value;
      box.dataset.dirty = "1";
    },
  });
  const toBank = h("input", { type: "checkbox", checked: Boolean(ui.importToBank), dataset: { fk: "codes-bank" }, onChange: () => (ui.importToBank = toBank.checked) });
  const imports = c.changes.filter((change) => change.kind === "importCodes");
  const doImport = async () => {
    const done = await actions.stage({ kind: "importCodes", text: box.value, toBank: toBank.checked });
    if (done) {
      ui.importText = "";
      actions.toast("ok", "Items added. They are saved when you press Save.");
      actions.refresh();
    }
  };
  return panel(
    "Item codes",
    "u-codes",
    h("p", { class: "note" }, "An item code is a line of text that stands for one item, like BL2(…). Paste codes here to add those items."),
    box,
    h("label", { class: "check" }, toBank, "Put them in the bank"),
    imports.length > 0
      ? h(
          "div",
          { class: "rows" },
          imports.map((change) =>
            h(
              "div",
              { class: "row staged" },
              h("div", { class: "row-text" }, h("span", { class: "row-title" }, "Staged import"), h("span", { class: "row-sub" }, change.to)),
              h("div", { class: "row-actions" }, h("button", { type: "button", class: "btn small", dataset: { fk: `undo-import-${change.id}` }, onClick: () => actions.removeChange(change.id) }, "Undo")),
            ),
          ),
        )
      : null,
    h(
      "div",
      { class: "btn-pair" },
      h("button", { type: "button", class: "btn big", dataset: { fk: "codes-import" }, onClick: doImport }, "Import codes"),
      h("button", { type: "button", class: "btn big", title: "Copies the codes of everything this character carries, bank included.", dataset: { fk: "codes-export" }, onClick: () => actions.copyAllCodes() }, "Copy all item codes"),
    ),
  );
}

/** One ammo type: its upgrade level with minus and plus, and how much it then holds. */
function ammoRow(c, actions, ammo, index) {
  const change = c.changes.find((ch) => ch.kind === "ammoLevel" && ch.label === ammo.label);
  const was = c.original.ammo?.[index];
  const set = (level) => actions.stage({ kind: "ammoLevel", ammo: ammo.name, level });
  return h(
    "div",
    { class: `row ammo-row ${change ? "staged" : ""}`, dataset: { ammo: ammo.name } },
    h(
      "div",
      { class: "row-text" },
      h("span", { class: "row-title" }, ammo.label),
      h(
        "span",
        { class: "row-sub" },
        `Holds ${ammo.capacity.toLocaleString("en-US")}`,
        change && was ? ` · was upgrade ${was.level} (${was.capacity.toLocaleString("en-US")})` : "",
      ),
    ),
    h(
      "div",
      { class: "row-actions stepper" },
      h("button", { type: "button", class: "btn small", "aria-label": `${ammo.label}: one upgrade less`, disabled: ammo.level <= 0, dataset: { fk: `ammo-down-${ammo.name}` }, onClick: () => set(ammo.level - 1) }, "−"),
      h("b", { title: `Upgrade ${ammo.level} of 7` }, `${ammo.level}/7`),
      h("button", { type: "button", class: "btn small", "aria-label": `${ammo.label}: one upgrade more`, disabled: ammo.level >= 7, dataset: { fk: `ammo-up-${ammo.name}` }, onClick: () => set(ammo.level + 1) }, "+"),
    ),
  );
}

export function renderUnlocks(state, actions) {
  const c = state.character;
  const disk = c.original;
  const uvhmPending = changeOf(c, "unlockUvhm") !== null;
  const ammo = changeOf(c, "unlockAmmo");
  const allBought = (disk.ammo ?? []).length > 0 && disk.ammo.every((a) => a.level >= 7);
  return h(
    "div",
    { class: "panels two" },
    panel(
      "Game modes",
      "u-modes",
      h(
        "div",
        { class: "rows" },
        unlockRow(c, actions, {
          kind: "unlockTvhm",
          title: "True Vault Hunter Mode",
          sub: "The second playthrough.",
          unlocked: disk.playthroughsUnlocked >= 1,
          impliedBy: uvhmPending ? "Comes with Ultimate" : null,
        }),
        unlockRow(c, actions, {
          kind: "unlockUvhm",
          title: "Ultimate Vault Hunter Mode",
          sub: "The third playthrough. Unlocks True Vault Hunter Mode too.",
          unlocked: disk.playthroughsUnlocked >= 2,
        }),
        unlockRow(c, actions, {
          kind: "unlockSlaughterdome",
          title: "Creature Slaughter Dome",
          sub: "The arena add-on.",
          unlocked: disk.slaughterdomeUnlocked,
        }),
      ),
    ),
    panel(
      "Ammo",
      "u-ammo",
      h(
        "div",
        { class: "rows" },
        h(
          "div",
          { class: `row ${ammo ? "staged" : ""}` },
          h("div", { class: "row-text" }, h("span", { class: "row-title" }, "All ammo upgrades"), h("span", { class: "row-sub" }, "Buys every ammo upgrade and fills your ammo.")),
          h(
            "div",
            { class: "row-actions" },
            ammo
              ? [
                  h("span", { class: "state pending" }, "Unlocks on save"),
                  h("button", { type: "button", class: "btn small", dataset: { fk: "undo-unlockAmmo" }, onClick: () => actions.removeChange(ammo.id) }, "Undo"),
                ]
              : allBought
                ? h("span", { class: "state" }, icon("check", 14, 3.4), "All bought")
                : h("button", { type: "button", class: "btn small yellow", dataset: { fk: "do-unlockAmmo" }, onClick: () => actions.stage({ kind: "unlockAmmo" }) }, "Unlock all"),
          ),
        ),
      ),
      h("h3", { class: "sub-head" }, "One type at a time"),
      h("div", { class: "rows" }, (c.current.ammo ?? []).map((a, i) => ammoRow(c, actions, a, i))),
      h("p", { class: "note dim" }, "An upgrade raises how much ammo the type holds. Max out ammo on the Overview tab fills it."),
    ),
    itemLevels(c, actions, state.ui),
    itemCodes(c, actions, state.ui),
  );
}

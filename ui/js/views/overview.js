// Overview: name, level, Overpower level, currencies, backpack, bank, gun slots, ammo.
// Every control stages its edit as soon as it is changed; nothing is written until Save.

import { formatMoney, formatNumber, h, parseWholeNumber } from "../dom.js";

function changeOf(c, kind) {
  return c.changes.find((change) => change.kind === kind) ?? null;
}

/**
 * A text box that stages its edit while you type (quietly: a half-typed value that is not
 * valid yet is just not staged) and again, with any refusal shown, when you leave it or
 * press Enter. `read` turns the text into the value to stage, or null if it is not one.
 */
function liveInput(actions, { kind, shown, read, complaint, attrs = {} }) {
  let timer = null;
  const input = h("input", {
    class: "input",
    type: "text",
    autocomplete: "off",
    spellcheck: "false",
    ...attrs,
    value: shown,
    dataset: { fk: `field-${kind}` },
    onInput: () => {
      input.dataset.dirty = "1";
      clearTimeout(timer);
      timer = setTimeout(() => {
        const value = read(input.value);
        if (value !== null && input.isConnected) actions.stage({ kind, value }, { quiet: true });
      }, 350);
    },
    onKeydown: (event) => {
      if (event.key === "Enter") input.blur();
      if (event.key === "Escape") {
        clearTimeout(timer);
        delete input.dataset.dirty;
        input.value = shown;
        actions.refresh();
      }
    },
    onBlur: () => {
      clearTimeout(timer);
      // (A box that was just replaced by a redraw is not one the user left.)
      if (!input.isConnected || !input.dataset.dirty) return;
      delete input.dataset.dirty;
      const value = read(input.value);
      if (value === null) {
        actions.toast("error", complaint);
        actions.refresh();
        return;
      }
      actions.stage({ kind, value });
    },
  });
  return input;
}

function numberField(c, actions, { kind, label, value, was, format = formatNumber }) {
  const changed = changeOf(c, kind) !== null;
  const input = liveInput(actions, {
    kind,
    shown: format(value),
    read: parseWholeNumber,
    complaint: `${label}: enter a whole number.`,
    attrs: { inputmode: "numeric" },
  });
  return h(
    "label",
    { class: `field ${changed ? "changed" : ""}` },
    label,
    input,
    changed ? h("span", { class: "was" }, `Was ${format(was)}`) : null,
  );
}

function nameField(c, actions) {
  const changed = changeOf(c, "name") !== null;
  const input = liveInput(actions, {
    kind: "name",
    shown: c.current.name,
    read: (text) => (text.trim() === "" ? null : text),
    complaint: "Name can't be empty.",
    attrs: { maxlength: "40", class: "input plain" },
  });
  return h("label", { class: `field ${changed ? "changed" : ""}` }, "Name", input, changed ? h("span", { class: "was" }, `Was ${c.original.name}`) : null);
}

function sliderField(c, actions, { kind, label, value, was, used, min, max, step }) {
  const changed = changeOf(c, kind) !== null;
  const shown = h("b", {}, `${value} slots`);
  const input = h("input", {
    type: "range",
    min,
    max,
    step,
    value,
    "aria-label": `${label} slots`,
    dataset: { fk: `field-${kind}` },
    onInput: () => (shown.textContent = `${input.value} slots`),
    onChange: () => actions.stage({ kind, value: Number(input.value) }),
  });
  return h(
    "label",
    { class: `field ${changed ? "changed" : ""}` },
    h("span", { class: "range-label" }, `${label} · `, shown, ` · ${used} used`),
    input,
    changed ? h("span", { class: "was" }, `Was ${was} slots`) : null,
  );
}

function stamps(c, kinds) {
  const labels = {
    name: "Name changed!",
    level: "Level changed!",
    opLevel: "Overpower changed!",
    itemLevels: "Gear levels changed!",
    money: "Money changed!",
    eridium: "Eridium changed!",
    seraph: "Seraph crystals changed!",
    torgue: "Torgue tokens changed!",
    backpackSlots: "Backpack changed!",
    bankSlots: "Bank changed!",
    gunSlots: "Gun slots changed!",
    maximizeAmmo: "Ammo will be filled!",
  };
  const found = kinds.filter((kind) => changeOf(c, kind));
  if (found.length === 0) return null;
  return h("div", { class: "stamps" }, found.map((kind) => h("span", { class: "stamp" }, labels[kind])));
}

function panel(title, id, ...body) {
  return h(
    "section",
    { class: "panel", "aria-labelledby": id },
    h("div", { class: "panel-head" }, h("h2", { id }, title)),
    h("div", { class: "panel-body" }, ...body),
  );
}

export function renderOverview(state, actions) {
  const c = state.character;
  const now = c.current;
  const was = c.original;
  const limits = c.limits;

  const opChanged = changeOf(c, "opLevel") !== null;
  const opSelect = h(
    "select",
    {
      class: "input",
      dataset: { fk: "field-opLevel" },
      onChange: () => actions.stage({ kind: "opLevel", value: Number(opSelect.value) }),
    },
    Array.from({ length: limits.opLevel[1] + 1 }, (_, n) => h("option", { value: n, selected: n === now.opLevel }, `OP ${n}`)),
  );

  const gearLevels = changeOf(c, "itemLevels");
  const gearCheck = h("input", {
    type: "checkbox",
    checked: gearLevels !== null,
    dataset: { fk: "field-itemLevels" },
    onChange: () => {
      if (gearCheck.checked) actions.stage({ kind: "itemLevels", level: 0, force: false });
      else if (gearLevels) actions.removeChange(gearLevels.id);
    },
  });

  const ammo = changeOf(c, "maximizeAmmo");
  const unlockAmmo = changeOf(c, "unlockAmmo");

  return h(
    "div",
    { class: "panels" },
    panel(
      "Progression",
      "p-prog",
      nameField(c, actions),
      h(
        "div",
        { class: "field-row" },
        numberField(c, actions, { kind: "level", label: `Level (${limits.level[0]}–${limits.level[1]})`, value: now.level, was: was.level }),
        h("label", { class: `field ${opChanged ? "changed" : ""}` }, "Overpower", opSelect, opChanged ? h("span", { class: "was" }, `Was OP ${was.opLevel}`) : null),
      ),
      h("p", { class: "note" }, "XP is set to match the new level."),
      h("label", { class: `check ${gearLevels ? "changed" : ""}` }, gearCheck, "Also level up all gear"),
      gearLevels ? h("p", { class: "note dim" }, `Item levels: ${gearLevels.to}. Items in the bank stay as they are.`) : null,
      stamps(c, ["name", "level", "opLevel", "itemLevels"]),
    ),
    panel(
      "Currencies",
      "p-cur",
      h(
        "div",
        { class: "field-row" },
        numberField(c, actions, { kind: "money", label: "Money", value: now.money, was: was.money, format: formatMoney }),
        numberField(c, actions, { kind: "eridium", label: "Eridium", value: now.eridium, was: was.eridium }),
        numberField(c, actions, { kind: "seraph", label: "Seraph crystals", value: now.seraph, was: was.seraph }),
        numberField(c, actions, { kind: "torgue", label: "Torgue tokens", value: now.torgue, was: was.torgue }),
      ),
      stamps(c, ["money", "eridium", "seraph", "torgue"]),
    ),
    panel(
      "Capacity",
      "p-cap",
      sliderField(c, actions, { kind: "backpackSlots", label: "Backpack", value: now.backpackSlots, was: was.backpackSlots, used: now.backpackUsed, min: limits.backpack[0], max: limits.backpack[1], step: 3 }),
      sliderField(c, actions, { kind: "bankSlots", label: "Bank", value: now.bankSlots, was: was.bankSlots, used: now.bankUsed, min: limits.bank[0], max: limits.bank[1], step: 2 }),
      h(
        "fieldset",
        { class: `field ${changeOf(c, "gunSlots") ? "changed" : ""}`, style: { border: "0", margin: "0", padding: "0" } },
        h("legend", { style: { padding: "0", "margin-bottom": "6px" } }, "Gun slots"),
        h(
          "div",
          { class: "seg" },
          [2, 3, 4].map((n) =>
            h(
              "button",
              {
                type: "button",
                class: `seg-btn ${was.gunSlots === n && now.gunSlots !== n ? "was-here" : ""}`,
                "aria-pressed": now.gunSlots === n ? "true" : "false",
                "aria-label": `${n} gun slots`,
                dataset: { fk: `field-gunSlots-${n}` },
                onClick: () => actions.stage({ kind: "gunSlots", value: n }),
              },
              n,
            ),
          ),
        ),
      ),
      h(
        "button",
        {
          type: "button",
          class: `btn big wide ${ammo ? "yellow" : ""}`,
          "aria-pressed": ammo ? "true" : "false",
          disabled: unlockAmmo !== null,
          title: unlockAmmo ? "Unlocking all ammo upgrades already fills your ammo." : "Fills every ammo type to the most your upgrades allow.",
          dataset: { fk: "field-ammo" },
          onClick: () => (ammo ? actions.removeChange(ammo.id) : actions.stage({ kind: "maximizeAmmo" })),
        },
        ammo ? "Ammo will be filled · Undo" : "Max out ammo",
      ),
      stamps(c, ["backpackSlots", "bankSlots", "gunSlots"]),
    ),
  );
}

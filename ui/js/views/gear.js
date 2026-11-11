// Gear: the character's loadout as the game lays it out (equipped weapons and gear, the
// backpack, the bank), item cards with rarity banners, and the detail panel with the item's
// level, moving it to or from the bank, and Duplicate Item.

import { h, icon } from "../dom.js";

const FILTERS = [
  { id: "all", label: "All", test: () => true },
  { id: "weapons", label: "Weapons", test: (item) => item.isWeapon },
  { id: "gear", label: "Gear", test: (item) => !item.isWeapon },
];

/** The four gear slots of the game's inventory screen, by item category. */
const GEAR_SLOTS = [
  { category: "shield", label: "Shield" },
  { category: "grenade-mod", label: "Grenade mod" },
  { category: "class-mod", label: "Class mod" },
  { category: "relic", label: "Relic" },
];
const GEAR_CATEGORIES = GEAR_SLOTS.map((slot) => slot.category);
/** Items the game keeps out of the bank, and whose level is not worth changing. */
const NOT_GEAR = ["mission", "customization", "usable"];

const WHERE = { backpack: "Backpack", equipped: "Equipped", bank: "Bank" };

// Pictures of weapons, shields, grenade mods and relics. The app draws them from the game's own files and serves them at
// `/picture/<size>/<item code>`, at twice the size shown here. The token keeps the web view
// from showing a picture kept from an earlier run of the app.
const PICTURE_SHOWN = { thumb: [144, 72], detail: [264, 132] };
const PICTURE_RUN = Date.now().toString(36);

/** Where an item's picture comes from, or null when there is nothing to ask for. */
function pictureSrc(item, size) {
  if (!item.code) return null;
  const preview = window.gkPreview;
  if (preview) return preview.picture?.(item, size) ?? null;
  return `/picture/${size}/${encodeURIComponent(item.code)}?run=${PICTURE_RUN}`;
}

/** Kinds of item the app never has a picture of: they have no model to draw. */
const NO_PICTURE = ["mission", "customization", "usable"];

/** True when it is worth asking the app for this item's picture. */
function mayHavePicture(item) {
  return Boolean(item.code) && !NO_PICTURE.includes(item.category);
}

/** The quiet stand-in for a picture that does not exist; `why` is its tooltip. */
function noPicture(why) {
  return h("div", { class: "gun-pic none", role: "img", "aria-label": `No picture. ${why}`, title: why }, h("span", {}, "No picture"));
}

/** The framed picture in the detail panel. Its height is fixed, so the panel does not jump. */
function detailPicture(item, title) {
  const src = mayHavePicture(item) ? pictureSrc(item, "detail") : null;
  if (!src) return noPicture(item.code ? "Story and mission items have no picture." : "This item's data can't be read.");
  const [width, height] = PICTURE_SHOWN.detail;
  const frame = h("div", { class: "gun-pic" });
  const failed = async () => {
    // The app answers a picture it cannot make with a sentence saying why.
    let why = "The picture could not be made.";
    try {
      const response = await fetch(src);
      if (!response.ok) why = (await response.text()).trim() || why;
    } catch {
      // Keep the plain sentence.
    }
    frame.replaceWith(noPicture(why));
  };
  frame.append(h("img", { alt: `Picture of ${title}`, width, height, decoding: "async", draggable: "false", onError: failed, src }));
  return frame;
}

/** The small picture on a weapon's card. The space stays if the picture cannot be made. */
function cardPicture(item) {
  const src = mayHavePicture(item) ? pictureSrc(item, "thumb") : null;
  if (!src) return null;
  const [width, height] = PICTURE_SHOWN.thumb;
  return h(
    "div",
    { class: "card-pic" },
    h("img", { alt: "", width, height, loading: "lazy", decoding: "async", draggable: "false", onError: (event) => event.target.remove(), src }),
  );
}

/** Class and inline color for an item's rarity. Items without a name stay neutral. */
export function tierLook(item) {
  if (!item || !item.name) return { class: "tier-unknown", style: {} };
  if (item.color) return { class: `tier-${item.tier} game-color`, style: { "--tier": item.color } };
  return { class: `tier-${item.tier}`, style: {} };
}

/** What to call an item the game data has no name for: what it is, never a guess. */
export function plainTitle(item, loadingNames = false) {
  const kind = item.category !== "unknown" ? item.categoryLabel : item.isWeapon ? "Weapon" : "Item";
  return loadingNames ? kind : `Unnamed ${kind.toLowerCase()}`;
}

/** The banner text: the rarity, or why there is none. */
function bannerText(item, loadingNames) {
  if (item.name) return item.tierLabel;
  if (loadingNames) return "Reading names…";
  return item.unverified ? "Unverified data" : "No game data";
}

function rawFacts(item) {
  const raw = item.raw;
  if (!raw) return "The item's data can't be read";
  const part = (label, value) => (value === null || value === undefined ? null : `${label} ${value}`);
  return [part("Type", raw.itemType), part("Balance", raw.balance), part("Maker", raw.manufacturer), part("Set", raw.set)]
    .filter(Boolean)
    .join(" · ");
}

function makerLine(item) {
  return [item.manufacturer, item.category !== "unknown" ? item.categoryLabel : null].filter(Boolean).join(" · ");
}

/** Weapons first, then other items, each in save order; a staged copy sits after its source. */
function ordered(items) {
  const settled = items.filter((item) => !item.isNew);
  const base = [...settled.filter((item) => item.isWeapon), ...settled.filter((item) => !item.isWeapon)];
  const out = [];
  for (const item of base) {
    out.push(item);
    out.push(...items.filter((other) => other.isNew && other.copyOf === item.ref));
  }
  out.push(...items.filter((item) => item.isNew && !out.includes(item)));
  return out;
}

/** The pending changes made to one item (its level, a move), newest last. */
function changesFor(c, item) {
  return c.changes.filter((change) => change.itemRef === item.ref && (change.kind === "itemLevel" || change.kind === "moveItem"));
}

function card(item, state, actions, loadingNames, { where = null, listItem = true } = {}) {
  const look = tierLook(item);
  const changed = changesFor(state.character, item);
  const moved = changed.some((change) => change.kind === "moveItem");
  const leveled = changed.some((change) => change.kind === "itemLevel");
  const selected = state.selectedItem === item.ref;
  const named = Boolean(item.name);
  const title = named ? item.name : plainTitle(item, loadingNames);
  const banner = bannerText(item, loadingNames);
  const level = item.level === null ? "Lvl ?" : `Lvl ${item.level}`;
  const label = `${title}, ${named ? `${item.tierLabel}, ` : ""}${level}, ${WHERE[item.location]}${item.isNew ? ", new copy, not saved yet" : ""}`;
  return h(
    "div",
    {
      class: `card ${look.class} ${selected ? "selected" : ""} ${item.isNew ? "is-new" : ""} ${changed.length > 0 && !item.isNew ? "is-changed" : ""} ${named ? "" : "raw"}`,
      style: look.style,
      role: listItem ? "listitem" : null,
      dataset: { ref: item.ref },
    },
    h("button", {
      type: "button",
      class: "card-pick",
      "aria-label": label,
      "aria-pressed": selected ? "true" : "false",
      dataset: { fk: `card-${item.ref}` },
      onClick: () => actions.selectItem(item.ref),
    }),
    h("div", { class: "banner" }, banner),
    item.isNew
      ? h("span", { class: "new-badge" }, item.copyOf ? "New copy!" : "New!")
      : changed.length > 0
        ? h("span", { class: "new-badge" }, moved && leveled ? "Changed!" : moved ? "Moved!" : "Leveled!")
        : null,
    cardPicture(item),
    h(
      "div",
      { class: "card-body" },
      h("div", { class: "card-name" }, title),
      h("div", { class: "card-maker" }, named ? makerLine(item) : item.definition || rawFacts(item)),
      h(
        "div",
        { class: "card-foot" },
        h("span", {}, level, item.unverified && named ? h("span", { class: "tag", title: "From add-on content that has not been checked in the game yet." }, "Unverified") : null),
        item.isNew
          ? h(
              "button",
              { type: "button", class: "btn small", dataset: { fk: `undo-${item.ref}` }, "aria-label": `Undo: remove this new ${item.copyOf ? "copy" : "item"} of ${title}`, onClick: () => actions.removeChange(item.changeId) },
              "Undo",
            )
          : h("span", {}, where ?? WHERE[item.location]),
      ),
    ),
  );
}

/** The item's card rows as the game shows them, computed from the game's data. */
function statsBlock(item) {
  const stats = item.stats;
  if (!stats || (stats.rows.length === 0 && stats.lines.length === 0)) return null;
  return h(
    "section",
    { class: "stats", "aria-label": "Item card" },
    h(
      "dl",
      { class: "stat-rows" },
      stats.rows.map((row) => {
        const props = { class: row.text === null ? "unknown" : "", dataset: { stat: row.key } };
        if (row.note) props.title = row.note;
        return h(
          "div",
          props,
          h("dt", {}, row.label),
          h("dd", {}, row.text ?? "?", row.note && row.text !== null ? h("span", { class: "approx", "aria-label": "may differ slightly in game" }, "≈") : null),
        );
      }),
    ),
    stats.lines.map((line) => {
      const props = { class: `card-text ${line.kind}`, style: line.color ? { color: line.color } : {} };
      if (line.note) props.title = line.note;
      return h("p", props, line.text, line.note ? h("span", { class: "approx", "aria-label": "may differ in game" }, "≈") : null);
    }),
    h(
      "p",
      { class: "stats-note" },
      stats.missingLines > 0
        ? `Worked out from your game's files; ${stats.missingLines === 1 ? "one line the game shows is" : `${stats.missingLines} lines the game shows are`} left out. Not yet checked against the game.`
        : "Worked out from your game's files. Not yet checked against the game.",
    ),
  );
}

function detail(item, state, actions, loadingNames) {
  const c = state.character;
  if (!item) {
    return h(
      "aside",
      { class: "detail empty tier-unknown", "aria-label": "Selected item" },
      h("div", { class: "hazard thin" }),
      h(
        "div",
        { class: "detail-body" },
        h("h2", {}, "Pick an item"),
        h("p", { class: "note" }, "Select a weapon or a piece of gear to duplicate it or to copy its item code."),
      ),
    );
  }
  const look = tierLook(item);
  const named = Boolean(item.name);
  const title = named ? item.name : plainTitle(item, loadingNames);
  const size = title.length > 22 ? "longer" : title.length > 11 ? "long" : "";

  // Why the button is off, if it is: the item itself, or a full backpack.
  let blocked = item.duplicateBlocked;
  let fullBackpack = false;
  if (!blocked && c.backpackFull) {
    blocked = c.backpackFullMessage;
    fullBackpack = true;
  }

  // One line under the name: maker, type, level, where.
  const meta = [
    item.manufacturer,
    item.category !== "unknown" ? item.categoryLabel : item.isWeapon ? "Weapon" : "Item",
    `Lvl ${item.level ?? "?"}`,
    item.isNew ? "Backpack (new)" : WHERE[item.location],
  ].filter(Boolean);
  const stats = statsBlock(item);
  const tools = itemTools(item, state, actions);

  return h(
    "aside",
    { class: `detail ${look.class}`, style: look.style, "aria-label": "Selected item" },
    h("div", { class: "hazard thin" }),
    h(
      "div",
      { class: "detail-body" },
      h(
        "div",
        { class: "detail-chips" },
        h("span", { class: "tier-chip" }, bannerText(item, loadingNames)),
        item.unverified && named ? h("span", { class: "tag" }, "Unverified") : null,
        item.isNew ? h("span", { class: "tag", style: { color: "var(--yellow)" } }, "Not saved yet") : null,
      ),
      h("h2", { class: size }, title),
      h("p", { class: "detail-meta" }, meta.join(" · ")),
      detailPicture(item, title),
      tools,
      stats,
      named
        ? null
        : h(
            "dl",
            { class: "facts" },
            item.definition ? h("div", { class: "wide" }, h("dt", {}, "Game definition"), h("dd", { class: "small selectable" }, item.definition)) : null,
            h("div", { class: "wide" }, h("dt", {}, "What the save says"), h("dd", { class: "small selectable" }, rawFacts(item))),
          ),
      named || loadingNames
        ? null
        : h("p", { class: "note dim" }, "The game's data has no name for this item, so Vault Key shows what the save holds and guesses nothing."),
      h(
        "div",
        { class: "dup-wrap" },
        blocked
          ? h(
              "div",
              { class: "callout", id: "dup-note" },
              icon("warn", 18, 2.6),
              h("span", {}, blocked, fullBackpack ? ". Backpack size is on the Overview tab." : ""),
            )
          : null,
        h(
          "button",
          {
            type: "button",
            class: "dup-btn",
            id: "duplicate-button",
            disabled: Boolean(blocked),
            title: blocked || undefined,
            "aria-describedby": "dup-note",
            dataset: { fk: "duplicate" },
            onClick: () => actions.duplicate(item.ref),
          },
          h("span", { class: "skew-in" }, icon("copy", 26, 2.8), "Duplicate item"),
        ),
      ),
      blocked
        ? null
        : h("p", { class: "note", id: "dup-note" }, "The copy goes into your backpack. Nothing touches your save until you press Save, and a backup is made first."),
      h(
        "div",
        { class: "btn-pair" },
        h(
          "button",
          { type: "button", class: "btn", disabled: !item.code, title: item.code ? "Copies this item's code so you can paste it somewhere else." : "This item has no code.", dataset: { fk: "copy-code" }, onClick: () => actions.copyText(item.code, "Item code copied.") },
          "Copy item code",
        ),
        item.isNew
          ? h("button", { type: "button", class: "btn", dataset: { fk: "undo-detail" }, onClick: () => actions.removeChange(item.changeId) }, icon("undo", 16, 2.8), "Undo copy")
          : null,
      ),
    ),
  );
}

/** The item's level (a stepper and "Match my level") and the move to or from the bank. */
function itemTools(item, state, actions) {
  const c = state.character;
  const now = c.current;
  if (NOT_GEAR.includes(item.category)) return null;
  const levelChange = c.changes.find((change) => change.kind === "itemLevel" && change.itemRef === item.ref);
  const levelTools = [];
  if (item.level !== null) {
    const top = Math.min(c.limits.itemLevel[1], Math.max(item.level, now.level === 80 ? 80 + now.opLevel : 80));
    const setLevel = (level) => actions.stage({ kind: "itemLevel", item: item.ref, level });
    const mine = now.level === 80 ? 80 + now.opLevel : now.level;
    levelTools.push(
      h(
        "div",
        { class: `level-tool ${levelChange ? "changed" : ""}` },
        h("span", { class: "tool-label" }, "Level"),
        h(
          "div",
          { class: "stepper" },
          h("button", { type: "button", class: "btn small", "aria-label": "One level lower", disabled: item.level <= 1, dataset: { fk: "level-down" }, onClick: () => setLevel(item.level - 1) }, "−"),
          h("b", { "aria-live": "polite", id: "item-level" }, item.level),
          h("button", { type: "button", class: "btn small", "aria-label": "One level higher", disabled: item.level >= top, dataset: { fk: "level-up" }, onClick: () => setLevel(item.level + 1) }, "+"),
        ),
        h(
          "button",
          {
            type: "button",
            class: "btn small yellow",
            disabled: item.level === mine,
            title: `Sets this item to level ${mine}${now.level === 80 && now.opLevel > 0 ? ` (level 80 and Overpower ${now.opLevel})` : ""}.`,
            dataset: { fk: "level-match" },
            onClick: () => setLevel(mine),
          },
          `Match my level (${mine})`,
        ),
      ),
      levelChange ? h("p", { class: "note was-line" }, `Was level ${levelChange.from}. Not saved yet.`) : null,
    );
  }

  // Moving to or from the bank.
  let move = null;
  if (item.location !== "equipped") {
    const toBank = item.location === "backpack";
    let why = null;
    if (toBank && now.bankUsed >= now.bankSlots) why = `The bank is full (${now.bankUsed}/${now.bankSlots}). You can make it bigger on the Overview tab.`;
    if (!toBank && now.backpackUsed >= now.backpackSlots) why = `The backpack is full (${now.backpackUsed}/${now.backpackSlots}). You can make it bigger on the Overview tab.`;
    if (item.category === "unknown" && !item.name) why = "Vault Key can't tell what this item is, so it can't be moved.";
    move = h(
      "button",
      {
        type: "button",
        class: "btn wide",
        id: "move-button",
        disabled: Boolean(why),
        title: why ?? (toBank ? "Puts this item in the bank. It stays there for every playthrough of this character." : "Takes this item out of the bank and into your backpack."),
        dataset: { fk: "move" },
        onClick: () => actions.moveItem(item.ref, toBank),
      },
      icon(toBank ? "bank" : "bag", 18, 2.6),
      toBank ? "Move to bank" : "Move to backpack",
    );
    if (why) move = [move, h("p", { class: "note dim" }, why)];
  } else {
    move = h("p", { class: "note dim" }, "Equipped. Unequip it in the game to move it to the bank.");
  }
  return h("section", { class: "item-tools", "aria-label": "Change this item" }, levelTools, move);
}

/** A frame of the loadout: a header strip with a title and a count, and its contents. */
function frame({ id, title, count = null, full = false, side = null }, ...body) {
  return h(
    "section",
    { class: "frame", "aria-labelledby": `frame-${id}`, dataset: { frame: id } },
    h(
      "div",
      { class: "frame-head" },
      h("h2", { id: `frame-${id}` }, title),
      count ? h("span", { class: `frame-count ${full ? "full" : ""}` }, h("b", {}, count)) : null,
      h("span", { class: "grow" }),
      side,
    ),
    h("div", { class: "frame-body" }, ...body),
  );
}

/** One slot of the equipped frame: a faint frame named for the slot (as the game always shows
 *  its weapon, shield, grenade mod, class mod and relic slots), holding the item's own card,
 *  an empty slot, or a locked weapon slot. */
function slot(s, state, actions, loadingNames) {
  let inside;
  if (s.item) {
    inside = card(s.item, state, actions, loadingNames, { where: "", listItem: false });
  } else if (s.locked) {
    inside = h(
      "div",
      { class: "slot-card locked" },
      icon("lock", 26, 2.6),
      h("span", {}, "Locked"),
      h(
        "button",
        {
          type: "button",
          class: "btn small yellow",
          title: `Unlocks weapon slots up to slot ${s.n}. The game gives these as story rewards.`,
          dataset: { fk: `unlock-gun-slot-${s.n}` },
          onClick: () => actions.stage({ kind: "gunSlots", value: s.n }),
        },
        "Unlock slot",
      ),
    );
  } else {
    inside = h("div", { class: `slot-card empty ${s.staged ? "staged" : ""}` }, s.staged ? "Unlocked · not saved" : "Empty");
  }
  return h("div", { class: "eq-slot", role: "listitem", "aria-label": s.label }, h("span", { class: "eq-slot-label" }, s.label), inside);
}

/** What is in each equipped slot: four weapon slots (or locked), then the four gear slots. */
function equippedSlots(c) {
  const equipped = c.items.filter((item) => item.location === "equipped");
  const guns = [1, 2, 3, 4].map((n) => {
    const item = equipped.find((i) => i.isWeapon && i.quickSlot === n) ?? null;
    return { key: `w${n}`, label: `Weapon ${n}`, short: `W${n}`, n, item, locked: !item && n > c.current.gunSlots, staged: !item && n <= c.current.gunSlots && n > c.original.gunSlots };
  });
  const placed = new Set();
  const gear = GEAR_SLOTS.map(({ category, label }) => {
    const item = equipped.find((i) => !i.isWeapon && i.category === category && !placed.has(i.ref)) ?? null;
    if (item) placed.add(item.ref);
    return { key: category, label, short: label, item, locked: false, staged: false };
  });
  // Equipped items the game data can't place (names still loading, say), and a weapon in a
  // slot outside 1 to 4 (which should not happen) still show.
  const others = equipped
    .filter((i) => !placed.has(i.ref) && (i.isWeapon ? !(i.quickSlot >= 1 && i.quickSlot <= 4) : !GEAR_CATEGORIES.includes(i.category)))
    .map((item) => ({ key: `x${item.ref}`, label: "Equipped", short: "Equipped", item, locked: false, staged: false }));
  return { guns, gear: [...gear, ...others] };
}

function equippedFrame(c, state, actions, loadingNames) {
  const { guns, gear } = equippedSlots(c);
  const open = state.equippedOpen;
  const toggle = h(
    "button",
    { type: "button", class: "btn small", "aria-expanded": open ? "true" : "false", dataset: { fk: "equipped-toggle" }, onClick: () => actions.setEquippedOpen(!open) },
    open ? "Collapse" : "Expand",
  );
  const head = h(
    "div",
    { class: "frame-head" },
    h("h2", { id: "frame-equipped" }, "Equipped"),
    h("span", { class: "grow" }),
    toggle,
  );
  let body;
  if (open) {
    // The same cards as the backpack, four to a row as in the game: weapons 1 to 4, then
    // shield, grenade mod, class mod and relic.
    body = h("div", { class: "cards eq-cards", role: "list" }, [...guns, ...gear].map((s) => slot(s, state, actions, loadingNames)));
  } else {
    // Two short lines: a chip per slot, in the item's rarity color, that selects it.
    const chipFor = (s) => {
      if (!s.item) {
        return h(
          "span",
          { class: `eq-chip empty ${s.locked ? "locked" : ""}`, title: s.locked ? `${s.label}: locked` : `${s.label}: empty` },
          h("small", {}, s.short),
          s.locked ? icon("lock", 12, 3) : "—",
        );
      }
      const look = tierLook(s.item);
      const title = s.item.name ?? plainTitle(s.item, loadingNames);
      return h(
        "button",
        {
          type: "button",
          class: `eq-chip ${look.class} ${state.selectedItem === s.item.ref ? "selected" : ""}`,
          style: look.style,
          title: `${s.label}: ${title}, level ${s.item.level ?? "?"}`,
          dataset: { fk: `eq-${s.key}`, ref: s.item.ref },
          onClick: () => actions.selectItem(s.item.ref),
        },
        h("small", {}, s.short),
        h("span", { class: "eq-name" }, title),
      );
    };
    body = h("div", { class: "eq-strip" }, guns.map(chipFor), gear.map(chipFor));
  }
  return h(
    "section",
    { class: `frame ${open ? "" : "collapsed"}`, "aria-labelledby": "frame-equipped", dataset: { frame: "equipped" } },
    head,
    h("div", { class: "frame-body" }, body),
  );
}

function cardGrid(items, state, actions, loadingNames, empty) {
  return items.length > 0
    ? h("div", { class: "cards", role: "list" }, items.map((item) => card(item, state, actions, loadingNames, { where: "" })))
    : h("p", { class: "note dim frame-empty" }, empty);
}

export function renderGear(state, actions) {
  const c = state.character;
  const filter = FILTERS.find((f) => f.id === state.gearFilter) ?? FILTERS[0];
  const selected = c.items.find((item) => item.ref === state.selectedItem) ?? null;
  const loadingNames = state.status?.itemData?.state === "loading";
  const now = c.current;
  // The backpack and the bank share one frame, one at a time, so the cards get the room.
  const view = state.gearView === "bank" ? "bank" : "backpack";
  const shown = ordered(c.items.filter((item) => item.location === view)).filter(filter.test);
  const filtered = filter.id !== "all";
  const places = [
    { id: "backpack", label: "Backpack", count: `${now.backpackUsed}/${now.backpackSlots}`, full: c.backpackFull },
    { id: "bank", label: "Bank", count: `${now.bankUsed}/${now.bankSlots}`, full: now.bankUsed >= now.bankSlots },
  ];
  const switcher = h(
    "div",
    { class: "place-switch", role: "tablist", "aria-label": "Backpack or bank" },
    places.map((place) =>
      h(
        "button",
        {
          type: "button",
          role: "tab",
          class: "place-tab",
          "aria-selected": place.id === view ? "true" : "false",
          dataset: { fk: `view-${place.id}` },
          onClick: () => actions.setGearView(place.id),
        },
        place.label,
        " ",
        h("b", { class: place.full ? "full" : "" }, place.count),
      ),
    ),
  );
  const filters = h(
    "div",
    { class: "filters" },
    FILTERS.map((f) =>
      h(
        "button",
        { type: "button", class: "filter", "aria-pressed": f.id === filter.id ? "true" : "false", dataset: { fk: `filter-${f.id}` }, onClick: () => actions.setGearFilter(f.id) },
        f.label,
      ),
    ),
  );
  const empty = filtered
    ? `Nothing in the ${view} matches this filter.`
    : view === "bank"
      ? "The bank is empty. Pick an item in your backpack and press Move to bank."
      : "The backpack is empty.";

  return h(
    "div",
    { class: "gear" },
    h(
      "div",
      { class: "loadout", id: "cards", "aria-label": "Inventory" },
      equippedFrame(c, state, actions, loadingNames),
      h(
        "section",
        { class: "frame", "aria-label": view === "bank" ? "Bank" : "Backpack", dataset: { frame: "inventory", view } },
        h("div", { class: "frame-head" }, switcher, h("span", { class: "grow" }), filters),
        h("div", { class: "frame-body" }, cardGrid(shown, state, actions, loadingNames, empty)),
      ),
    ),
    detail(selected, state, actions, loadingNames),
  );
}

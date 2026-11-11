// Small helpers for building the page without a framework.

const SVG_NS = "http://www.w3.org/2000/svg";

/**
 * Create an element. `props` keys: `class`, `text`, `dataset` (object), `style` (object of
 * custom properties or styles), `on<Event>` handlers, and anything else as an attribute
 * (`false`, `null` and `undefined` leave the attribute off; `true` sets it empty).
 */
export function h(tag, props, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === false || value === null || value === undefined) continue;
    if (key === "class") el.className = value;
    else if (key === "text") el.textContent = value;
    else if (key === "dataset") Object.assign(el.dataset, value);
    else if (key === "style") {
      for (const [name, v] of Object.entries(value)) {
        if (v !== null && v !== undefined) el.style.setProperty(name, v);
      }
    } else if (key.startsWith("on") && typeof value === "function") el.addEventListener(key.slice(2).toLowerCase(), value);
    else if (key === "value") el.value = value;
    else if (key === "checked") el.checked = Boolean(value);
    else if (value === true) el.setAttribute(key, "");
    else el.setAttribute(key, String(value));
  }
  append(el, children);
  return el;
}

function append(el, children) {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    if (Array.isArray(child)) append(el, child);
    else el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

// Icons are drawn here: original line art, nothing taken from the game.
const ICONS = {
  key: '<circle cx="7.5" cy="12" r="4.5"/><path d="M12 12h10M18 12v4M21 12v3"/>',
  lock: '<rect x="5" y="11" width="14" height="10" rx="1"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
  unlock: '<rect x="5" y="11" width="14" height="10" rx="1"/><path d="M8 11V7a4 4 0 0 1 7.6-1.7"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="1"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>',
  check: '<path d="M5 12l5 5 9-10"/>',
  cross: '<path d="M6 6l12 12M18 6L6 18"/>',
  warn: '<path d="M12 3l10 18H2z"/><path d="M12 10v5M12 18v.5"/>',
  gear: '<circle cx="12" cy="12" r="3.2"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M18.7 5.3l-2.1 2.1M7.4 16.6l-2.1 2.1"/>',
  undo: '<path d="M4 9h10a5.5 5.5 0 0 1 0 11H8"/><path d="M8 5L4 9l4 4"/>',
  flask: '<path d="M9 3h6M10 3v6l-5 10a1.5 1.5 0 0 0 1.4 2h11.2a1.5 1.5 0 0 0 1.4-2l-5-10V3"/><path d="M7.5 15h9"/>',
  bank: '<path d="M3 9l9-5 9 5"/><path d="M5 9v9M10 9v9M14 9v9M19 9v9"/><path d="M3 20h18"/>',
  bag: '<rect x="4" y="8" width="16" height="13" rx="1"/><path d="M9 8V6a3 3 0 0 1 6 0v2"/><path d="M4 13h16"/>',
  file: '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 3"/>',
};

export function icon(name, size = 18, strokeWidth = 2.6) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("width", size);
  svg.setAttribute("height", size);
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", strokeWidth);
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("icon");
  svg.innerHTML = ICONS[name] ?? "";
  return svg;
}

/** A small slanted black label, such as "SLOT 4 · ASSASSIN". */
export function chip(label, { class: extra = "", title = null, mark = null } = {}) {
  return h("span", { class: `chip ${extra}`, title }, h("span", { class: "chip-in" }, mark, label));
}

/** A skewed button whose label stays upright. */
export function skewButton(props, ...children) {
  return h("button", { type: "button", ...props, class: `skew ${props.class ?? ""}` }, h("span", { class: "skew-in" }, ...children));
}

const numberFormat = new Intl.NumberFormat("en-US");

export function formatNumber(value) {
  return numberFormat.format(value ?? 0);
}

export function formatMoney(value) {
  return `$${formatNumber(value)}`;
}

/** The whole number in what someone typed ("$123,456" → 123456), or null if there is none. */
export function parseWholeNumber(text) {
  const cleaned = String(text).replace(/[\s,$]/g, "");
  if (!/^-?\d+$/.test(cleaned)) return null;
  const value = Number(cleaned);
  return Number.isSafeInteger(value) ? value : null;
}

/** A moment (seconds since 1970) in the computer's own time zone: "9 Nov 2026, 03:15". */
export function formatWhen(unixSeconds) {
  if (unixSeconds === null || unixSeconds === undefined) return "unknown time";
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function plural(count, one, many) {
  return `${formatNumber(count)} ${count === 1 ? one : many}`;
}

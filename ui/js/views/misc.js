// Start-up and empty states for the main area, and the message stack (toasts).

import { h, icon, skewButton } from "../dom.js";

export function renderBlank(state, actions) {
  const status = state.status;
  if (!status) {
    return h("div", { class: "blank" }, h("h1", {}, "Starting…"));
  }
  if (!status.saveFolder.found) {
    const problem = status.saveFolder.problem;
    const missing = problem?.kind === "save-folder";
    return h(
      "div",
      { class: "blank" },
      h("h1", {}, missing ? "No save folder found" : "Vault Key can't start"),
      missing
        ? h("p", {}, "Vault Key could not find the folder where Borderlands 2 keeps its saves. Start the game once and make a character, then try again.")
        : h("p", {}, "Something about Vault Key's folders is not right. Nothing was read or changed."),
      h("div", { class: "callout bad" }, icon("warn", 18, 2.6), h("span", { class: "selectable" }, problem?.message ?? "")),
      skewButton({ class: "btn hero", dataset: { fk: "retry" }, onClick: () => actions.retryStart() }, "Try again"),
    );
  }
  if (state.listError) {
    return h(
      "div",
      { class: "blank" },
      h("h1", {}, "Can't read your saves"),
      h("div", { class: "callout bad" }, icon("warn", 18, 2.6), h("span", { class: "selectable" }, state.listError)),
      skewButton({ class: "btn hero", dataset: { fk: "retry" }, onClick: () => actions.reload() }, "Try again"),
    );
  }
  if (state.characters.length === 0) {
    return h(
      "div",
      { class: "blank" },
      h("h1", {}, "No characters yet"),
      h("p", {}, "The save folder is there, but it holds no characters. Make one in Borderlands 2, quit the game, then look again."),
      h("div", { class: "path selectable" }, status.saveFolder.path),
      skewButton({ class: "btn hero", dataset: { fk: "retry" }, onClick: () => actions.reload() }, "Look again"),
    );
  }
  const anyReadable = state.characters.some((entry) => entry.readable);
  return h(
    "div",
    { class: "blank" },
    h("h1", {}, anyReadable ? "Pick a character" : "No readable characters"),
    h("p", {}, anyReadable ? "Choose a character on the left to see and edit it." : "Vault Key found save files, but could not read any of them. They are listed on the left with what is wrong."),
  );
}

const TITLES = { error: "That didn't work", ok: "Done", info: "Heads up" };

export function renderToasts(state, actions) {
  return state.toasts.map((toast) =>
    h(
      "div",
      { class: `toast ${toast.kind}`, role: toast.kind === "error" ? "alert" : "status", dataset: { toast: toast.kind } },
      h("div", { class: "toast-text" }, h("strong", {}, TITLES[toast.kind] ?? TITLES.info), h("span", { class: "selectable" }, toast.text)),
      h("button", { type: "button", class: "btn small", "aria-label": "Dismiss this message", dataset: { fk: `toast-${toast.id}` }, onClick: () => actions.dismissToast(toast.id) }, icon("cross", 14, 3.2)),
    ),
  );
}

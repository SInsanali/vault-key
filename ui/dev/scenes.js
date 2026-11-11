// Puts the browser preview into a given state, for looking at one screen at a time:
//   index.html?mock&screen=<name>
// Screens: gear (default), overview, unlocks, backups, save, save-readonly, saved, restore,
// save-many, drop, settings, error, full, pick, raw, external, export, exported, add, loadout. Data variants (any screen): &data=empty,
// &data=nofolder, &names=loading|off, &game=running|unknown.

const SLOT = (n) => `76561190000000000/Save000${n}.sav`;

export async function play({ state, actions, params }) {
  const screen = params.get("screen") ?? "gear";
  if (state.characters.length === 0) return;

  const zero = async () => {
    await actions.openCharacter(SLOT(4), { force: true });
  };
  const withChanges = async () => {
    await actions.stage({ kind: "money", value: 123_456 });
    await actions.duplicate("54:1");
  };

  switch (screen) {
    case "overview":
      await zero();
      await actions.stage({ kind: "money", value: 123_456 });
      actions.setTab("overview");
      break;
    case "unlocks":
      await zero();
      await actions.stage({ kind: "unlockUvhm" });
      await actions.stage({ kind: "ammoLevel", ammo: "sniper", level: 3 });
      actions.setTab("unlocks");
      break;
    case "loadout":
      await zero();
      await actions.stage({ kind: "itemLevel", item: "54:1", level: 34 });
      await actions.moveItem("54:1", true);
      actions.selectItem("54:1");
      actions.setTab("gear");
      break;
    case "backups":
      await zero();
      await actions.setTab("backups");
      break;
    case "save":
      await zero();
      await withChanges();
      actions.openSaveSheet();
      break;
    case "save-many":
      // A long list of changes: the list grows before it scrolls.
      await zero();
      await withChanges();
      await actions.stage({ kind: "level", value: 31 });
      await actions.stage({ kind: "eridium", value: 250 });
      await actions.stage({ kind: "ammoLevel", ammo: "sniper", level: 3 });
      await actions.stage({ kind: "ammoLevel", ammo: "pistol", level: 2 });
      await actions.stage({ kind: "itemLevel", item: "54:1", level: 31 });
      await actions.moveItem("53:0", true);
      actions.openSaveSheet();
      break;
    case "save-readonly":
      await actions.openCharacter(SLOT(1), { force: true });
      await actions.stage({ kind: "level", value: 72 });
      await actions.stage({ kind: "eridium", value: 500 });
      await actions.duplicate("54:0");
      actions.openSaveSheet();
      break;
    case "saved":
      await zero();
      await withChanges();
      actions.openSaveSheet();
      await actions.confirmSave();
      break;
    case "restore": {
      await zero();
      await actions.setTab("backups");
      await actions.stage({ kind: "level", value: 31 });
      actions.askRestore(state.backups[0]);
      break;
    }
    case "drop":
      await zero();
      await withChanges();
      await actions.openCharacter(SLOT(2));
      break;
    case "quit":
      await zero();
      await withChanges();
      window.gkCloseRequested();
      break;
    case "settings":
      await zero();
      actions.openSettings();
      break;
    case "error":
      await zero();
      await withChanges();
      actions.selectItem("54:1");
      actions.toast("error", "Backpack full — free a slot or increase backpack size");
      break;
    case "full":
      await zero();
      await actions.stage({ kind: "backpackSlots", value: 12 });
      for (let n = 0; n < 8; n++) await actions.duplicate("54:1");
      actions.selectItem("54:1");
      break;
    case "raw":
      await zero();
      actions.selectItem("53:3");
      break;
    case "pick":
      await zero();
      break;
    case "external":
      await actions.openFile();
      await actions.stage({ kind: "money", value: 123_456 });
      await actions.duplicate("54:0");
      actions.selectItem("54:0");
      break;
    case "exported":
      await zero();
      await withChanges();
      actions.openExportSheet();
      await actions.confirmExport();
      break;
    case "export":
      // A friend's file, edited, at the export review.
      await actions.openFile();
      await actions.stage({ kind: "money", value: 123_456 });
      await actions.duplicate("54:0");
      actions.openExportSheet();
      break;
    case "add":
      await actions.openFile();
      await actions.stage({ kind: "money", value: 123_456 });
      actions.openSaveSheet();
      break;
    case "readonly":
      await actions.openCharacter(SLOT(1), { force: true });
      actions.selectItem("54:0");
      break;
    default:
      await zero();
      await withChanges();
      actions.selectItem("54:1");
  }
  // Gear tab layout, on any screen: &view=bank shows the bank, &slots=open expands Equipped.
  if (params.get("view")) actions.setGearView(params.get("view"));
  if (params.get("slots")) actions.setEquippedOpen(params.get("slots") === "open");
  document.documentElement.dataset.scene = screen;
}

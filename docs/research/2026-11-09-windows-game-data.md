# Windows build: game data verified (2026-11-09)

What was checked on the Windows Steam install of Borderlands 2 while building `crates/gamedata`,
and every difference from the Mac notes (`2026-11-08-game-data-spike.md`, the plan 2 ledger, and
`plan2-task10-blocked-report.md`). Facts only. Each one says how it was checked. Anything not
checked is listed in §10.

The install was only read. Checks were made with the crate's own reader, either in
`crates/gamedata/tests/real_game.rs` (named below as *test `name`*; they print their figures with
`--nocapture`) or with throwaway probe programs that were deleted afterwards (named *probe*).

## 1. Install

| Fact | How checked |
|---|---|
| Install folder: `C:\Program Files (x86)\Steam\steamapps\common\Borderlands 2\`. It holds `WillowGame\` and `DLC\` directly | Listing |
| `steamapps\appmanifest_49520.acf` exists, with `"installdir" "Borderlands 2"` and `"buildid" "9218157"` | Read the file |
| `steamapps\libraryfolders.vdf` lists one library. Its `"apps"` block does **not** list 49520, although the game is installed there | Read the file |
| `changelist.txt` in the install folder contains `2863302` | Read the file |

Because the `"apps"` block is stale, install discovery does not use it. It looks in each library
for `appmanifest_49520.acf` (for the folder name) and then for the game folder itself.

## 2. Package files

| Fact | How checked |
|---|---|
| 1,847 `.upk` files: 914 in `WillowGame\CookedPCConsole`, 478 under `DLC\*\Compat\Content`, 455 under `DLC\*\Lic\Content` | Probe (walked both trees) |
| All 1,847 decompress and parse with 0 failures. Every one is version 832, licensee 46 | Probe, release build, 38.9 s for all |
| Layouts: 11 whole-file compressed, 1,812 chunk-table, 24 not compressed at all | Same probe |
| The 11 whole-file packages are exactly the files with a `.uncompressed_size` sidecar: `AkAudio`, `Core`, `Engine`, `GameFramework`, `GearboxFramework`, `GFxUI`, `IpDrv`, `OnlineSubsystemSteamworks`, `Startup`, `Startup_LOC_INT`, `WillowGame` | Listing, plus the probe's count |
| The `lzo1x` crate's strict decoder (end marker required, no trailing bytes, exact length) accepts every block of every package | Same probe: 0 failures |
| DLC startup packages: tag, then version 832 and licensee 46 (`C1 83 2A 9E 40 03 2E 00`), compression flags 2, cooker version 134 | Test `dlc_startup_packages_use_the_chunk_table_layout`; header fields read with a script |
| Their engine-version field is **not** uniform: it ranges from 1712560 to 1712574 across the 21 packages | Header fields read with a script |
| Chunk counts in those packages: 1 for most, 2 (Sage), 3 (Iris, Orchid), 4 (Aster), 22 (Anemone) | Same script |

`Startup.upk`:

| Fact | How checked |
|---|---|
| 17,800,929 bytes on disk. Starts with the tag and then block size `0x00020000` (whole-file layout) | Test `startup_matches_its_sidecar` |
| Decompresses to **60,134,127** bytes, equal to its sidecar | Same test |
| 28,576 names, 760 imports, 55,087 exports; 748 `WeaponNamePartDefinition` and 498 `ItemNamePartDefinition` exports; `GD_Globals.General.Globals` is a `GlobalsDefinition` | Test `startup_tables` |
| The definitions the Mac tests read have the same values (Infinity title `PartName` and `Priority` 10, Vladof `Grades[0].DisplayName`, 18 `RarityLevelColors` rows, the intro ECHO's name and base rarity 502, pistol and sniper weapon types) | Test `real_definitions` |

## 3. Base-game database

| Fact | How checked |
|---|---|
| The six README vectors resolve to the same paths, and back | Test `readme_vectors_resolve` |
| Library shapes equal the Mac's (libs / entries / located / unresolved): weapon types 39/75/33/5, weapon parts 14/1508/1406/2, item types 103/916/698/1, item parts 6/930/857/0, manufacturers 2/18/18/0, balances 141/1373/1083/6 | Test `base_library_shapes` |
| Every base library has fewer sublibraries than half its lib range (39 < 64, 14 < 32, 103 < 256, 6 < 32, 2 < 8, 141 < 512) | Same test |
| All 16 known items (2 README codes, Basic Repeater, intro ECHO, 12 items from the main character) resolve to the same name, level, tier, colour, manufacturer, category and gear class as on the Mac | Test `known_items_match_expected`. The item codes were decoded to numbers with a throwaway script following `bl2-save-format.md` §6; the decoded README values match that doc's |
| In the tables' located objects, part tables hold only `…PartDefinition` classes, balance tables only `…BalanceDefinition` classes, and item-type tables only the six item classes | Probe (class census) |
| 7 base balances are `MissionWeaponBalanceDefinition` | Test `database_classifies_known_items` |

`RarityLevelColors` on the Windows build (colour as `#RRGGBB`, then alpha):

| Levels | Colour | Rating |
|---|---|---|
| 0 | #CDC1AF, 0 | Unknown |
| 1 | #FFFFFF, 255 | Common |
| 2 | #3DD20B, 255 | Uncommon |
| 3 | #3C8EFF, 255 | Rare |
| 4 | #A83FE5, 255 | VeryRare |
| 5 | #FFB400, 255 | Legendary |
| 6 | #CA00A8, 255 | VeryRare |
| 7–10 | #FFB400, 255 | Legendary |
| 12–170 | #000000, 0 (filler) | Unknown |
| 171–175 | #CF4747, 255 | Unknown |
| 176–180 | #FFC7A7, 0 | Unknown |
| 181–499 | #FFFF0D, 0 | Unknown |
| 500 | #00FFFF, 0 | Unknown |
| 501 | #FF9AB8, 0 | Seraph |
| 502 | #FFFFFF, 0 | Unknown |
| 503 | #9132C8, 255 | Unknown |
| 504 | #00FFFF, 0 | Unknown |
| 506 | #F2FFA1, 255 | Rainbow |

Checked by a probe dump of the table and by tests `rarity_values_and_colours` and
`game_colour_table_edges`. Levels 11 and 505 have no row.

## 4. DLC folders and PackageIds

`DLC\` has 24 folders. 21 have `Compat\Content\<Name>_Startup_SF.upk` and
`Compat\Config\WillowDlc.ini`. The other three, `POCollectors`, `POGamestop` and `POPremierClub`,
have only a `Lic` folder (a `WillowDLC.ini` with a `[PromotionalUnlocks]` section, and licences).

The first `+PackageId=` line of each ini (test `dlc_sets_discovered`):

| Folder | Id | Folder | Id | Folder | Id |
|---|---|---|---|---|---|
| Orchid | 1 | Gladiolus | 8 | CommDay2013 | 18 |
| Tulip | 2 | Aster | 9 | Allium | 19 |
| Iris | 3 | Lobelia | 10 | Nasturtium | 20 |
| Sage | 4 | PN2 | 12 | Peony | 21 |
| Flax | 5 | LootChest2 | 14 | Anemone | 22 |
| Lilac | 6 | Gardenia04 | 16 | Mancana | 23 |
| Gardenia01 | 7 | Gardenia05 | 17 | Anecana | 24 |

The ini repeats the id in every section, and some sections write it as `.PackageId=`; all agree.
Many DLC folders also have a `Lic` tree with more content packages, but startup packages exist
only under `Compat`.

## 5. DLC asset-library sets

| Fact | How checked |
|---|---|
| A DLC's set is one `DownloadableAssetLibraryDefinition`. Its `PackageDef` points at a `DownloadablePackageDefinition` (`PackageId`, `DLCName`, `PackageDisplayName`), and `Libraries[0…5]` at six `PackageAssetLibrary` objects in the base game's category order | Probe dumps of Orchid, Tulip, Anemone, Gladiolus, Lobelia, Allium |
| The set's path differs between DLCs (`GD_Orchid_AssetLibraries.Inventory.Orchid_AssetLibSet`, but `GD_Anemone_AssetLibraries.Anemone_AssetLibSet`), so sets are found by class, not by path | Same |
| **13** startup packages hold a set: ids 1, 2, 3, 4, 5, 6, 8, 9, 10, 19, 20, 21, 22. The other 8 (7, 12, 14, 16, 17, 18, 23, 24) hold only a package definition | Test `dlc_sets_discovered` |
| No other package in the whole install holds a set (`WillowGame.upk` has only the class's default object) | Probe over all 1,847 packages |
| Each set's `PackageDef.PackageId` equals the id in that folder's ini | Test `dlc_sets_discovered` |
| Some packages carry copies of other DLCs' package definitions (Anemone has Lilac's and Tulip's), so the id is taken through the set's own `PackageDef` | Probe |
| A DLC library with no sublibraries has no `SublibraryLinks` property at all (Tulip's weapon types, weapon parts and manufacturers, for example) | Probe |
| 8 of the 13 DLCs (Allium, Anemone, Aster, Gladiolus, Lilac, Lobelia, Nasturtium, Peony) leave `LibraryType` off their libraries. Sublibraries do carry one (all of them in the four DLCs counted: Allium, Anemone, Aster, Peony), and wherever one is present it matches its library's category | Probe; the builder checks every `LibraryType` it finds on every import, and none is rejected |
| Every DLC `AssetPaths` entry has `IsSubobjectMask` 0 | The builder rejects a non-zero mask, and no set is skipped: test `dlc_sets_discovered` (`skipped_packages` is empty) |
| Every DLC library has fewer sublibraries than half its lib range, and no table is longer than its asset range | Test `dlc_sets_discovered` |
| 1,051 table objects are defined again by a later package. Every repeat has the same class and the same scalar properties as the first, so "first package wins" changes nothing here | Test `shared_definitions_agree_across_packages` |
| Adding the DLC packages changes nothing about set 0 or any base-game object | Test `dlc_name_parts_resolve` |
| 380 DLC parts have a `PartName` | Same test |

Coverage per set, as libs / asset entries / entries whose definition is in a startup package
(test `dlc_sets_discovered`):

| Set | Weapon types | Weapon parts | Item types | Item parts | Manufacturers | Balances |
|---|---|---|---|---|---|---|
| 0 base | 39/75/33 | 14/1508/1406 | 103/916/698 | 6/930/857 | 2/18/18 | 141/1373/1083 |
| 1 Orchid | 5/8/0 | 4/179/55 | 29/138/79 | 4/14/10 | 1/3/1 | 36/188/96 |
| 2 Tulip | 0/0/0 | 0/0/0 | 2/110/89 | 1/57/37 | 0/0/0 | 2/105/85 |
| 3 Iris | 7/27/0 | 2/25/16 | 30/44/23 | 2/66/44 | 1/3/1 | 44/112/31 |
| 4 Sage | 4/8/0 | 8/208/39 | 20/160/70 | 4/18/9 | 2/2/1 | 27/204/82 |
| 5 Flax | 0/0/0 | 1/3/0 | 9/24/17 | 1/21/17 | 0/0/0 | 11/26/17 |
| 6 Lilac | 1/1/0 | 1/9/0 | 2/95/94 | 1/39/38 | 0/0/0 | 3/91/90 |
| 8 Gladiolus | 0/0/0 | 1/40/32 | 1/9/9 | 1/20/12 | 0/0/0 | 2/19/17 |
| 9 Aster | 0/0/0 | 2/75/63 | 24/113/72 | 4/88/82 | 1/1/1 | 25/108/67 |
| 10 Lobelia | 4/7/0 | 1/17/17 | 8/40/30 | 1/19/18 | 0/0/0 | 13/33/22 |
| 19 Allium | 1/2/0 | 0/0/0 | 8/28/24 | 0/0/0 | 0/0/0 | 9/32/25 |
| 20 Nasturtium | 0/0/0 | 0/0/0 | 13/42/24 | 0/0/0 | 0/0/0 | 14/44/24 |
| 21 Peony | 0/0/0 | 0/0/0 | 6/12/12 | 0/0/0 | 0/0/0 | 6/12/12 |
| 22 Anemone | 10/18/2 | 2/303/137 | 29/171/72 | 4/720/270 | 0/0/0 | 43/280/95 |

An entry that is not "located" names an object that no startup package defines (enemy weapons,
objects in level packages, cut content). Items that refer to one resolve to unknown. Sublibrary
links that are null or point outside the startup packages (their lib index then has an empty
table): Iris 11 item types and 11 balances, Sage 2 and 2, Flax 1 and 1 (probe).

Mis-filed table entries, all in DLC (probe):

- Iris item parts list `GD_Iris_SeraphItems.Hoplite.Iris_Seraph_Shield_Juggernaut` (a
  `ShieldDefinition`) and a `…_Shield_Booster_Balance`; Iris balances list the same shield.
- Sage weapon parts list three `WeaponBalanceDefinition` objects (`Sage_Seraph_…_Balance`).
- Anemone item types list an `ObjectRedirector`, an `InventoryBalanceDefinition` and an
  `ItemPoolDefinition`.

The crate keeps a definition only when its class fits the table's category, so these resolve to
nothing instead of to the wrong kind. The base game has no such entries.

Item classes: DLC adds one gear class the base game lacks, `CrossDLCClassModDefinition` (24
objects: the Mechromancer and Psycho class mods in Aster and Lobelia). In `WillowGame.upk` its
super class is `ClassModDefinition` (probe reading the class export's super reference), so it is
treated as a class mod. Test `dlc_item_type_classes`.

## 6. Rarity

### 6.1 Base game

| Fact | How checked |
|---|---|
| Distinct rarity values on base types and parts: {1, 2, 3, 4, 5, 171, 176, 181, 502, 503, 504}; none in 6…10; none unresolvable | Test `base_rarity_census` |
| 15 parts use `ItemRarity4_VeryRare_Alien` (value 2); every one has `Alien` or `ETech` in its path | Same test |
| Of the base rarity structs, 579 set a non-zero `BaseValueConstant` and 286 set a `BaseValueAttribute`. **None sets both** | Test `base_rarity_structs_never_set_both_constant_and_attribute` |

### 6.2 DLC census

Over every type and part in the 13 DLC sets that the base game does not also list (test
`dlc_rarity_census`):

| Value | Objects | Colour row | Carried by |
|---|---|---|---|
| 1 | 105 | #FFFFFF Common | customization types and parts |
| 2 | 97 | #3DD20B Uncommon | same |
| 3 | 202 | #3C8EFF Rare | same |
| 4 | 72 | #A83FE5 VeryRare | same |
| 5 | 16 | #FFB400 Legendary | same |
| 10 | 47 | #FFB400 Legendary | mission item types, for example in Iris and Anemone |
| 181 | 1 | #FFFF0D | `GD_Flax_Items.Item.Item_Wisp` |
| 497 | 15 | #FFFF0D (181–499) | Seraph weapon barrels in Orchid (7), Sage (4), Aster (4) |
| 500 | 12 | #00FFFF | weapon bodies in Gladiolus (8) and Lobelia (4) |
| 501 | 29 | #FF9AB8 Seraph | Seraph item types, relic upgrades, shield materials, one boss weapon body |
| 502 | 4 | #FFFFFF | Anemone: two weapon barrels, a grenade material, a relic body |
| 503 | 2 | #9132C8 | Iris Torgue token, an Anemone relic body |
| 504 | 1 | #00FFFF | Iris shield-booster pickup |
| 506 | 9 | #F2FFA1 Rainbow | Anemone bodies, shield materials, two skins |

- **No value falls outside the colour table or in its filler row**, and no DLC rarity struct is
  unresolvable.
- **Seraph is 501.** Seraph items carry 501 directly. Seraph weapons reach it by sum: the barrel
  carries 497 and the body is a very-rare one (4). The Devastator's balance, for example, has
  `BaseDefinition` `GD_Weap_Pistol.A_Weapons.Pistol_Torgue_4_VeryRare` and the 497 barrel (probe).
  497 on its own lies in an unrelated row, so this is further evidence for the summing rule.
- **Pearlescent is 500.** The 12 objects that carry 500 are weapon bodies named
  `…_Body_<maker>_6…`. None has `Pearl` in its path, so the plan's test for the label would not
  have held. The link is one step away: 20 weapon part lists in those two packages allow a 500
  body; each allows only 500 bodies, its barrels carry no rarity, and every title those barrels
  list is a `Title_Pearlescent_…` name part. All 12 bodies are used that way. So level 500 is
  labelled pearlescent. (Gladiolus's tables also list a 13th title, `Title_Pearlescent_Wolverine`,
  that no part list uses.)
- **Rainbow (effervescent) is 506**, directly or as a 502 barrel plus a very-rare body.

### 6.3 A rarity struct with both a constant and an attribute

Five Anemone definitions set both `BaseValueConstant` and `BaseValueAttribute`. In all five the
attribute is `GD_Anemone_Weapons.Rarity_Item.Anemone_ItemRarity7_Effervescent`, whose constant
is 506:

| Definition | Constant | Constant + attribute | Attribute alone |
|---|---|---|---|
| `GD_Anemone_Weapons.Body.SG_Body_Torgue_7` | 506 | 1012 | 506 |
| `GD_Anemone_Weapons.SMG.SMG_Body_Tediore_6_Infection_Cleaner` | 500 | 1006 | 506 |
| `GD_Anemone_Weapons.AssaultRifle.AR_Body_Dahl_6_Worming` | 500 | 1006 | 506 |
| `GD_BanditTech_Items_Anemone.BanditTech.Skin_Effer` | 1 | 507 | 506 |
| `GD_Runner_Items_Anemone.Runner.Skin_Effer` | 1 | 507 | 506 |

Adding the two (the Swift rule, never exercised because the base game has no such struct) gives
levels the colour table does not have. Taking the attribute alone gives the Rainbow row every
time. The crate therefore reads the attribute as replacing the constant. Base-game values are
unaffected (§6.1). Checked by test `dlc_rarity_census`. **Not confirmed in game.**

## 7. DLC resolution: what is and is not established

Two hypotheses decide how a DLC item's numbers map onto these tables. Neither has been compared
with a real DLC item, so `ItemDatabase::DLC_RESOLUTION` stays `Unverified` and `info()` returns
all-unknown (never "gear") for any item whose set id is not 0, as the plan requires.
`info_with(…, DlcResolution::Verified)` resolves under the hypotheses, for checking.

- **H-set:** the item's set id is the `PackageId` of the DLC whose set holds its DLC assets.
- **H-flag:** for a set id other than 0, a reference whose lib index has its top bit set indexes
  that set's tables with the bit cleared; any other reference indexes the base tables.

Structural support found on this install:

- Base and DLC libraries all fit below the flag bit (§3, §5).
- None of the 16 known base-game items uses the flag bit (test
  `base_item_values_never_use_the_dlc_flag`).
- A set links to exactly one number, its `PackageDef.PackageId`.
- DLC balances carry `DlcItemSet`, a `DownloadableItemSetDefinition` with the same `PackageDef`
  and a separate `ContentId` (Orchid: `PackageId` 1, `ContentId` 4; Tulip: 2 and 3). A real DLC
  item whose set id equals its DLC's `PackageId` and not its `ContentId` would support H-set.
  The reverse would refute it.

Items built from the tables resolve end to end under the hypotheses (test
`constructed_dlc_items_resolve_only_when_asked_to`): the Unforgiven (set 8) as a pearlescent
Jakobs pistol, level 500, #00FFFF; the Devastator (set 1) as a Seraph Torgue pistol, level 501,
#FF9AB8; a Mechromancer class mod (set 9) as a class mod. This shows the mechanism works on real
tables. It does not show the hypotheses are right.

## 8. Timing

Windows build of the tests (`opt-level = 1`), this machine, files already in the OS cache:

| Step | Time |
|---|---|
| First import, 22 packages (fingerprint, read, build, write cache) | 0.50 s and 0.65 s on two runs |
| Second call (cache hit: fingerprint plus load) | 0.12 s and 0.29 s |
| Opening `Startup.upk` alone (read, decompress, tables) | about 0.14 s |
| Opening all 22 source packages | about 0.25 s |
| Building the database from them | about 0.07 s |

The cache file is 1,115,944 bytes and holds 14 sets and 6,268 objects. Test
`imports_real_game_within_time_limits` (limits: 30 s and 2 s). The install's package folders were
identical before and after an import (test `import_never_writes_to_the_game_folder`; folder
modification times are not compared, because NTFS refreshes them lazily when a folder is listed).

## 9. Differences from the Mac notes

| Topic | Mac notes | Windows build |
|---|---|---|
| Where the data folder is | `Borderlands2.app/Contents/GameData/` | The install folder itself |
| `Startup.upk` decompressed size | 60,134,152 | 60,134,127 (25 bytes fewer). Name, import and export counts are the same |
| Base `.upk` count | 917 | 914 |
| DLC `.upk` count | 478 | 478 under `Compat\Content`, plus 455 under `Lic\Content` |
| Engine-version header field | 1712574 in every package | 1712560 to 1712574 across the DLC startup packages. The reader ignores the field |
| Packages with no compression | "all other packages" have a chunk table | 24 packages have an empty chunk table and are stored uncompressed |
| DLC folders without a startup package | `POCollectors`, `POPremierClub` | those two and `POGamestop` |
| Steam library list | `"path"` entries in `libraryfolders.vdf` | Same file, but its `"apps"` block omits the game here |
| Rarity struct rule | constant + attribute | attribute replaces constant (§6.3). Same result on every base-game value |
| Pearlescent label | only if every 500 object has `Pearl` in its path | no path has it; the label rests on the title link (§6.2) |

The same on both: package version 832 and licensee 46; both compression layouts; the 11
whole-file packages; `Startup.upk`'s table counts; the base library shapes; the colour rows the
Mac notes quote; the README vectors; the 16 known items; the 21 DLC startup packages and all 21
`PackageId`s.

## 10. Not verified

- H-set and H-flag (§7), and therefore every DLC item's name and rarity, against the game.
- The attribute-replaces-constant rule (§6.3) in game.
- That the game shows a 500 item as pearlescent, a 501 item as Seraph and a 506 item as rainbow.
  The labels come from the data's own names; the colours from its table.
- The summing rule itself is still unconfirmed in game (spike §5). The DLC census adds evidence
  (497 + 4 = 501, 502 + 4 = 506) but no in-game comparison.
- The fixture save `win-throwaway-Save0001.sav` was not decoded here (that needs `savekit`), so
  no real Windows save item, base or DLC, has been run through the database yet.
- `Startup.upk`'s own engine-version field was not read.
- Packages other than the 22 startup packages are not used for the database. Table entries whose
  definitions live elsewhere stay unresolved (the "located" column in §5).
- Languages other than English, and any other game version or platform.

## 11. A real Windows save run through the database (2026-11-09, after §10 was written)

`win-throwaway-Save0001.sav` (a new level-1 Mechromancer made on this Windows build) was decoded
with `savekit` and every item looked up. This closes the fifth bullet of §10 and adds real-save
evidence for H-set and H-flag. It is still not an in-game comparison.

**Base-game items (set 0): all five resolve to a name, maker, category and colour.**

| Entry | Name | Maker | Category | Level → colour |
|---|---|---|---|---|
| weapon | Gearbox Rifle | Vladof | assault rifle | common, #FFFFFF |
| weapon | Gearbox Projectile Convergence | Hyperion | SMG | common, #FFFFFF |
| weapon | Gearbox Callipeen | Jakobs | sniper rifle | common, #FFFFFF |
| item | Contraband Sky Rocket | Bandit Made | grenade mod | rare, #3C8EFF |
| item | Vault Hunter's Relic | Eridian | relic | rare, #3C8EFF |

**DLC items (set 9): 12 entries, all resolve under H-set and H-flag, and the result is coherent.**
Set 9 is Aster's `PackageId`. With `DlcResolution::Verified`, the 12 entries resolve to six
`Head_…Aster` and six `Skin_…Aster` types, one head and one skin for each of the six classes
(Psycho, Siren, Soldier, Mechro, Assassin, Mercenary), all of class
`UsableCustomizationItemDefinition` with base rarity 3. For every entry the type and the balance,
which are two independent table lookups, name the same object (for example type
`GD_Psycho_Items_Aster.Psycho.Head_PsychoAster` with balance
`GD_Psycho_Items_Aster.BalanceDefs.Psycho_Head_PsychoAster`). A wrong set mapping or a wrong
index split would not produce 24 lookups that pair up this way, and "one head and one skin per
class from one DLC" is what a new character is given.

What this does and does not show:
- It supports H-set (set id = `PackageId`) and the DLC index split on real data.
- It does not show a DLC *name* or *rarity colour* matching the game: these types carry no
  `ItemName` in the startup package, so they have no display name here.
- In-game check that would settle it: this character's backpack should show exactly six heads
  and six skins from the Tiny Tina DLC, one per class.

Until that is looked at, `ItemDatabase::DLC_RESOLUTION` stays `Unverified`: DLC items are shown
with what the tables say, marked unverified, and are not offered for duplication.

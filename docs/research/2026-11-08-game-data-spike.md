# Spike: reading Borderlands 2 game data at first run (2026-11-08)

**Question.** Can Golden Key, at first run, read the user's own installed BL2 files and pull out
(a) item/weapon display names and rarity, and (b) UI art, keeping the results only on the
user's machine?

**Answer: yes for all three.** Names and rarity are straightforward. Bitmap UI art is
straightforward too. Vector UI art (inside the Scaleform movies) is not worth doing.

The work used throwaway Python 3 stdlib scripts (no third-party code). They only read the
install, read-only. Scripts and outputs are in the session scratchpad
(a scratch folder, not kept),
not in this repo. Time spent: about 1 hour.

Install examined: macOS Steam build,
`…/Borderlands 2/Borderlands2.app/Contents/GameData/WillowGame/CookedPCConsole/` plus
`GameData/DLC/*/Compat/Content/`.

---

## 1. What worked (evidence)

### 1.1 Package parsing: 100 % of packages
- I wrote a minimal LZO1X decompressor, a UE3 package header parser, and readers for the
  name, import and export tables.
- **All 917 base-game `.upk` files and all 478 DLC `.upk` files parsed**, with 0 failures.
  Every one is version **832**, licensee **46**, engine 1712574, cooker 134.
- Speed: pure Python decompressed `Startup.upk` (17.8 MB → 60.1 MB) in about 1 s. A full
  scan of all base-game packages (decompress plus tables) took about 50 s. Swift will be
  faster.

### 1.2 Names: found, in English, inside the packages themselves
- Every item/weapon name part in the base game is in **`Startup.upk`**:
  - 748 `WeaponNamePartDefinition` objects
  - 498 `ItemNamePartDefinition` objects
  - all 1 246 have a `PartName` StrProperty
- DLC name parts are in each DLC's `*_Startup_SF.upk`, for example `Anemone_Startup_SF.upk`.
  DLC adds 136 weapon name parts and 387 item name parts.
- The English strings are stored **in the package itself**, as plain ASCII FStrings.
  - `Startup_LOC_INT.upk` holds only `Texture2D` and `Font` objects (96 and 10), no strings.
  - The `Localization/INT/*.int` files don't have them either.
  - This install has only INT. Where other languages live is unknown.
- Examples:

| Object path | Class | `PartName` |
|---|---|---|
| `GD_Weap_Pistol.Name.Title_Maliwan.Title_Legendary_ThunderballFists` | WeaponNamePartDefinition | `Thunderball Fists` |
| `GD_Weap_Pistol.Name.Prefix.Prefix__Flints` | WeaponNamePartDefinition | `Flynt's` |
| `GD_GrenadeMods.Title.Title_RollingThunder` | ItemNamePartDefinition | `Rolling Thunder` |
| `GD_Weap_Pistol.Name.Title_Vladof.Title_Legendary_Infinity` | WeaponNamePartDefinition | `Infinity` |

- The balance chain works and is readable:
  - `GD_Weap_Pistol.A_Weapons_Legendary.Pistol_Maliwan_5_ThunderballFists` (WeaponBalanceDefinition)
    has `BaseDefinition` set to `…A_Weapons.Pistol_Maliwan_4_VeryRare`, which points to `…_3_Rare`, and so on.
  - It also has `WeaponPartListCollection` set to `….PartList`, which lists the barrel
    `GD_Weap_Pistol.Barrel.Pistol_Barrel_Maliwan_ThunderballFists`.
  - That barrel's `TitleList` contains the Thunderball Fists title part above. The weapon's
    display name is built as prefix `PartName` + title `PartName`.

### 1.3 Rarity: found
- Parts carry `Rarity` (an `AttributeInitializationData` struct). Its `BaseValueAttribute`
  points to one of `GD_Balance_Inventory.Rarity_Item.ItemRarity{1_Common,2_Uncommon,3_Rare,4_VeryRare,4_VeryRare_Alien,5_Legendary}`.
- Each of those has a `ConstantAttributeValueResolver` child with `ConstantValue` set to
  1, 2, 3, 4, 2 (Alien) and 5.
  - Example: the Thunderball Fists barrel has `Rarity` set to `ItemRarity5_Legendary`
    (value 5.0) and `MonetaryValueMod` set to `GD_Economy.Rarity.Att_Price_RarityMultiplier_05_Legendary`.
- Which parts carry `Rarity` (counts are numbers of parts):

| Part type | Parts with `Rarity` |
|---|---|
| Weapon `Body` parts | 21–33 per weapon type |
| Unique/legendary `Barrel` parts | 7–11 per weapon type |
| Shield `Material` | 56 |
| Class-mod `Specialization` | 20 |
| Relic `Upgrade` | 16 |
| Grenade `Damage` | 8 |

- `GD_Globals.General.Globals.RarityLevelColors` is the game's own table that maps rarity
  level to color and to the `EItemRarity` enum. It is readable:

| Level | `RarityRating` | Color (RGB) |
|---|---|---|
| 1 | Common | #FFFFFF |
| 2 | Uncommon | #3DD20B |
| 3 | Rare | #3C8EFF |
| 4 | VeryRare | #A83FE5 |
| 5 | Legendary | #FFB400 |
| 501 | Seraph | — |
| 506 | Rainbow | — |

  Color is a UE3 `FColor` stored as B,G,R,A bytes.
- **Not verified:** how the game combines the parts' `Rarity` values into the item's
  rarity level. My guess is that it adds them up: `VeryRare_Alien` = 2, which looks like
  an add-on to a body's level. Check this against known items before relying on it.

### 1.4 Bonus: the save's (lib, asset) indices resolve to object paths
- `GD_Globals.General.Globals.AssetLibraries` points to six `PackageAssetLibrary` objects:
  `GD_AssetLibraries.Inventory.AL_{WeaponTypes,WeaponParts,ItemTypes,ItemParts,Manufacturer,Balance}`.
- Each one has a `SublibraryLinks` array of `PackageAssetSublibrary` objects. Each of those
  has an `AssetPaths` array of `PathComponentNames`.
- The rule is: `lib` indexes `SublibraryLinks`, and `asset` indexes that sublibrary's
  `AssetPaths`.
- I tested it on the two test vectors in `docs/format/bl2-save-format.md` §6.5:

| Save value | (lib, asset) | Resolves to |
|---|---|---|
| balance 131111 | (128, 39) | `GD_Weap_Pistol.A_Weapons_Legendary.Pistol_Vladof_5_Infinity` |
| weapon type 1924 | (30, 4) | `GD_Weap_Pistol.A_Weapons.WeaponType_Vladof_Pistol` |
| manufacturer 133 | (1, 5) | `GD_Manufacturers.Manufacturers.Vladof` |
| item balance 27695 | (27, 47) | `GD_Artifacts.A_Item_Unique.A_VaultHunter` |
| item type 2840 | (11, 24) | `GD_Artifacts.A_Item_Unique.Artifact_VaultHunter` |
| item manufacturer 137 | (1, 9) | `GD_Manufacturers.Artifacts.Artifact_TypeA` |

- The results agree with each other (a Vladof Infinity pistol; a Vault Hunter's Relic).
  This fills the "asset library manifest" gap noted in the save-format doc §7.
- Only set 0 (the base game) was tried. DLC sets (set id ≠ 0) are presumably in the DLC
  packages' own libraries. Not verified.

### 1.5 UI art: bitmap textures extract cleanly
- UI is Scaleform. Each `SwfMovie` object (86 in `Startup.upk`) holds:
  - `RawData`: a `CFX` (zlib) GFx v9 SWF
  - `References`: the movie's bitmaps, stored as **separate `Texture2D` objects**
    (`<Movie>_I<hex id>`, `LODGroup=TEXTUREGROUP_UI`, `NeverStream=True`, pixel data inline)
- Inside the SWF, GFx tag 1009 (`DefineExternalImage2`) names each bitmap file, for example
  `SharedWillowInventory_I7.tga`. Tag 1008 (`DefineSubImage`, 26 of them in the inventory
  movie) cuts sprites out of a packed atlas.
- I extracted, as DXT1/DXT5 → PNG with my own decoder and PNG writer:

| Object | Format | Content |
|---|---|---|
| `SharedWillowInventory.SharedWillowInventory_I7` | 1024×1024 DXT5 | Inventory atlas: the gray, blue and gold item-card frames plus small inventory icons |
| `SharedWillowInventory.SharedWillowInventory_ID` | 180×68 DXT5 | Blue item-slot frame |
| `SharedWillowInventory.SharedWillowInventory_I11` | 152×44 DXT5 | White frame, which the game tints at runtime |
| `Common_GunMaterials.Logos.Logo_Bandit` | 256×256 DXT1 | Top two mips are in `Textures.tfc` (LZO); the rest are inline. This proves the `.tfc` path works too. It is a gun-skin mask, not UI art |

  PNGs: `…/scratchpad/spike/out/png/SharedWillowInventory__SharedWillowInventory_I7_1024x1024.png`
  and the others in that folder.
- **Vector art is not practical.** Much of the SWF art is vector shapes: 45 `DefineShape`,
  9 `DefineShape3`, 3 `DefineShape4` and 23 `DefineEditText` tags in the inventory movie
  alone. Manufacturer logos and weapon-type glyphs on the item card are probably vector.
  Extracting those needs a SWF shape rasteriser. The bitmaps do not.

---

## 2. Package-format facts a Swift implementation needs

Everything is little-endian.

**File wrapper (two layouts):**
- **(a) Whole-file compressed.** 11 files are like this: `Startup`, `Core`, `Engine`,
  `GFxUI`, `WillowGame`, `Startup_LOC_INT`, `AkAudio`, …. Exactly these 11 have a `.uncompressed_size` sidecar (decimal text: the total uncompressed size).
  - The first u32 is the tag `0x9E2A83C1`, but the next u32 is a block size (`0x00020000`),
    not a version.
  - Layout: one or more back-to-back "chunk streams" until EOF. Each stream is
    `tag, blockSize, compressedTotal, uncompressedTotal`, then `ceil(uncompTotal/blockSize)`
    pairs of `(compSize, uncompSize)`, then the LZO1X blocks.
  - Concatenate the output to get a normal package.
  - Detect this layout by bytes 4–5 (the u16 version) being < 500.
- **(b) Normal header with a chunk table.** All other packages.
  - Read the header through `CompressionFlags` (= 2, LZO) and the chunk array
    (`uncompOffset, uncompSize, compOffset, compSize`).
  - Each chunk points to a chunk stream as in (a). Decompress it into `uncompOffset`.
  - The bytes before the first chunk are copied as-is. All table offsets refer to the
    uncompressed layout.

**LZO1X.** Standard LZO1X-1 bitstream. Apple's Compression framework has no LZO, so you
need about 120 lines of your own decoder. The save code already needs LZO.

**Header (v832/46), in order:**
1. tag, u16 version, u16 licensee
2. i32 totalHeaderSize
3. FString folder
4. u32 packageFlags
5. nameCount/Offset, exportCount/Offset, importCount/Offset
6. dependsOffset
7. importExportGuidsOffset, importGuidCount, exportGuidCount
8. thumbnailTableOffset
9. 16-byte GUID
10. generations: i32 count × (exportCount, nameCount, netObjectCount)
11. engineVersion, cookerVersion
12. compressionFlags, chunk array

**Strings and tables:**
- **FString:** i32 length including the NUL. Negative length means UTF-16LE.
- **Name table:** FString + u64 flags.
- **FName:** i32 index + i32 number (suffix `_<number-1>` when nonzero).
- **Import** (28 B): classPackage FName, className FName, i32 outer, objectName FName.
- **Export:**
  - i32 class, i32 super, i32 outer, FName name, i32 archetype
  - u64 objectFlags, i32 serialSize, i32 serialOffset, u32 exportFlags
  - i32 netObjectCount + that many i32
  - 16-byte GUID, u32 packageFlags
  - There is no component map.
- **Object references:** >0 is export index + 1, <0 is −(import index + 1), 0 is null. Full
  paths come from walking `outer`.

**Object body:** i32 netIndex, then tagged properties until the FName `None`.
- Each tag: FName name, FName type, i32 size, i32 arrayIndex.
- `StructProperty` adds an FName struct name.
- `ByteProperty` adds an FName enum name. The value is an FName if size = 8, otherwise a u8.
- `BoolProperty` value is one u8 that is **not** counted in size.
- Arrays are i32 count + elements.
  - Object arrays are 4 bytes per element.
  - Struct arrays (for example `WeightedParts`, `AssetPaths`, `RarityLevelColors`) are
    tagged sub-properties per element.
  - The parser can't know element types without the class schema. For the handful of
    properties we need, hard-code the shapes.
- Small native structs (`Color`, `Vector`, `Guid`) are raw bytes.

**Texture2D:** tagged properties (`SizeX/Y`, `Format`=`PF_DXT1|PF_DXT5|…`,
`TextureFileCacheName`, `LODGroup`), then:
- 16 opaque bytes
- i32 mipCount
- per mip: bulk-data header (u32 flags, i32 elementCount, i32 sizeOnDisk, i32 offsetInFile),
  then the bytes inline if not stored separately, then i32 sizeX, i32 sizeY
- **Bulk-data flags:**
  - `0x1` = stored in `<TextureFileCacheName>.tfc`
  - `0x10` = LZO chunk stream at that offset
  - `0` = inline raw
- UI textures are inline, one mip, DXT5.
- DXT1/DXT5 decoding is about 80 lines. Alternatively, upload BC1/BC3 straight to Metal,
  which Apple-silicon Macs support.

**Where things live:**
- Names, balances, parts, rarity, globals and asset libraries: `Startup.upk` for the base
  game; `DLC/<name>/Compat/Content/<name>_Startup_SF.upk` for DLC.
- Some extra balances are also cooked into level packages (`*_Dynamic.upk`, `*_P.upk`).
- UI movies and textures: `Startup.upk` (`UI_*`, `Shared*`), `GD_*_Streaming_SF.upk`
  (skill trees) and per-level `*_P.upk`.

---

## 3. What blocked me / what's left open

- **Nothing blocked names, rarity or bitmap UI.**
- **Rarity combination rule not confirmed.** Which parts count, and whether they are summed,
  max'd or overridden. The color table is confirmed.
- **Name assembly rules not implemented:** prefix + title choice by `Priority`, and unique
  vs generic titles. The saved item stores prefix and title part slots directly (save-format
  doc §6.6), so for display we probably only need to look up those two parts' `PartName`.
- **DLC asset-library sets** (set id ≠ 0) are not resolved yet.
- **Vector Scaleform art** (likely manufacturer logos and weapon-type glyphs) would need an
  SWF shape renderer. Treat it as out of scope; draw our own icons for those.
- **Other languages:** only INT is installed, so I couldn't check them.
- **Windows/Linux builds** were not examined. They are the same engine version, but the
  compression layout could differ.

---

## 4. Effort estimate (production Swift, with tests)

| Piece | Estimate |
|---|---|
| Package reader: both wrapper layouts, LZO1X (shared with the save code), header, name/import/export tables, object-path index | 2–3 days |
| Tagged-property reader, plus typed extractors for name parts, balances/part lists, rarity resolvers, `RarityLevelColors`, asset libraries | 2–3 days |
| Import pipeline: find the install (Steam library folders, sandbox access via a user-picked folder plus a security-scoped bookmark), build a local cache (SQLite/JSON in Application Support), DLC packages, versioning/re-import | 2–3 days |
| Name/rarity semantics: confirm the rarity rule and name assembly against known items in real saves | 1–2 days |
| **Names + rarity total** | **about 1.5–2 weeks** |
| UI bitmap art: Texture2D reader, DXT1/5 decode (or Metal BC), PNG/`CGImage` cache, picking the needed textures and atlas sub-rects (needs a minimal SWF tag walk for tags 1008/1009) | **3–5 days** on top of the above |
| UI vector art (SWF shape rasteriser) | 2+ weeks. Not recommended |

**Main risks:**
1. Format details are empirical. The 16 opaque bytes before the mip array and the
   per-property shapes have no published spec.
2. Rarity and name-assembly semantics need checking against real items.
3. Sandboxed file access to the Steam folder needs user consent.
4. The DLC asset-library mapping is unverified.
5. Legal posture: only read the user's own install and cache locally. Never ship or upload
   extracted strings or images.

---

## 5. Resolved: rarity rule (plan 2, Task 6)

**Rule adopted (H-sum):** an item's rarity level is the item type's `BaseRarity` plus the
`Rarity` of every non-empty part slot. Part slots count only for weapons and for the gear item
classes `ShieldDefinition`, `GrenadeModDefinition`, `ClassModDefinition` and `ArtifactDefinition`.
A slot is empty when it is absent or all-ones (131071 for weapons, 65535 for items). The color is
the `RarityLevelColors` row whose `MinLevel…MaxLevel` contains the level. Any unresolvable input,
or a sum that is not an exact integer, makes the level unknown. Implemented as `RarityRule.level`,
with the per-category slot rule in `RarityRule.countsPartSlots` / `RarityRule.contributions`
(Task 7), and checked by `RarityRuleTests`.

**Status: strongly supported, not yet confirmed in game.** The census below and game-knowledge
examples (Phoenix 4 + 4 = 8, Fastball, Blood of Terramorphous) favour sum, but the known fixture
items cannot tell sum from max: each has at most one non-zero contribution except Righteous
Infinity (4 + 5), whose orange color max (5, also `#FFB400`) gives as well. Only `Startup.upk` was
censused; the DLC startup packages were not. The in-game check (Task 11) is still pending.

**Display (Task 7):** a level gets an unknown color (renders neutral) only when no row contains it
(e.g. 11) or its row is exactly `RGBA(0,0,0,0)`, the transparent-black filler (12…170). Alpha is not
an "unused" marker: rows 0, 176…502 and 504 also have alpha 0, and Seraph (501, #FF9AB8) is shown
pink in game, so those colors are used as known (alpha reported as stored). Level 6 is labeled
e-tech from the level itself, since its row is rated `RARITY_VeryRare`.

**Census (every type and part in `Startup.upk`'s asset tables):**
- Distinct non-empty rarity values: exactly {1, 2, 3, 4, 5, 171, 176, 181, 502, 503, 504}.
  - 171, 176 and 181 are buff drinks and currency, 502 the intro ECHOs, 503 Eridium, 504 shield
    boosters. 1–5 appear on parts, on customization types, on the legendary grenade-mod types (5)
    and on one storage-deck upgrade (4).
- No single object carries a value in 6…10. Only a sum can reach the table's e-tech row (6,
  #CA00A8, rated `RARITY_VeryRare`) and legendary rows (7…10, #FFB400).
- Zero `.unresolvable` rarity values on parts or types.
- All 15 parts whose rarity is `ItemRarity4_VeryRare_Alien` (value 2) are weapon barrels with
  `Alien` in their path, so a very-rare body (4) plus an alien barrel (2) gives the e-tech row.

**Why sum and not max:** max colors every known item correctly too, but it can never produce the
e-tech row (or any legendary level above 5), so it is rejected. The game data also reads as designed
for summing:
- Legendary grenades: type 5 plus a damage part. Fastball's balance allows only `Damage_Grade3…7`
  (2–4), so every roll lands at 7–9, inside the legendary row.
- Blood of Terramorphous: body 5 plus the single allowed upgrade `Upgrade_Grade15` (4) = 9.
- Legendary shields put 5 on both material and title (10, still legendary). The Phoenix shield has
  a very-rare material (4) plus a title with 4, so it sums to 8 (legendary) where max gives 4.

**Known items:** all 16 `KnownItems` rows (2 README codes, the Basic Repeater, the intro ECHO, and
12 items from Sam's main character), plus a constructed Thunderball Fists (body 4 + barrel 5 = 9 →
#FFB400), match their expected level and color row. Every non-virtual item in the three local
fixture saves computes a known level. These items are consistent with sum but do not distinguish it
from max (see Status above).

**Open (in-game, Task 11):** confirm a summed color in game: an e-tech (6) or a legendary (7…10)
item must show #CA00A8 or #FFB400. Shields to look at if one turns up: the Deadly Bloom's material
is named `Material5_Legendary_…` but carries 4 and its title carries nothing, so both sum and max
give 4 (very rare, purple). The Phoenix shield sums to 8 (legendary). The Seraph row (501, #FF9AB8) has alpha 0
but Seraph items show pink in game, so it is treated as a known color; Task 10 (DLC sets) can
confirm with a real Seraph item.

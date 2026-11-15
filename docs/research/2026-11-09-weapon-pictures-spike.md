# Spike: weapon pictures from the game's own files (2026-11-09)

**Question.** Sam asked: "I would like to see weapon pictures from the game in the UI." Can Golden
Key make such pictures from Sam's own installed game?

**Answer: yes, feasible, for every base-game weapon.** The game has no ready-made picture of a
weapon, but its 3D weapon models are in `Startup.upk`, the package the item database already
reads. A prototype reads them, puts a saved weapon together from its own parts, and draws it with
a software renderer in the app's comic style, in 25 ms (thumbnail) to 280 ms (1120×560) a picture. Colours come from the
weapon's own skin. The limits: the colour formula is an approximation (the game's shader is not in
the files), skin patterns and logos are not drawn, and DLC weapons were not rendered (they reuse
the same meshes, so the gap is small).

Facts only, each with how it was checked. What was not checked is in §9.

How the work was done: the install was only read. Checks were made with throwaway Python 3
scripts over a decompressed copy of `Startup.upk` kept in the session scratchpad (named *probe*),
with the prototype crate `crates/gamemesh` (its tests in `tests/real_game.rs` are named *test
`name`*; run them with `--nocapture` for the figures), or by looking at a rendered image (*looked*).
Extracted data and pictures are in the session scratchpad only, never in the repo.

Pictures (session scratchpad, regenerate with the command in §7):
`/tmp/claude-1000/-home-sam-code-golden-key/3de6855c-33a2-4180-99f7-db07c5324747/scratchpad/mesh-shots/`

## 1. Is there a ready-made 2D picture? No

| Fact | How checked |
|---|---|
| No package holds a picture per weapon or per item | Probe over all 1,847 packages (33.9 s, 0 failures) listing every `Texture2D` whose path contains `icon`, `itemcard`, `logo`, `manufacturer`, `element`, `inventory` or `weap_`: 432 distinct paths, none per weapon. The filter is by name, so a picture under an unrelated name would be missed |
| There is one picture **per weapon type**: `fx_shared_items.Textures.ItemCards.{AssaultRifle, repeater, RocketLauncher, Shotgun, SMG, SniperRifle, Shield}` in `Startup.upk`, 128×128 DXT5, stored inline. Beside them: `Ammo_*`, `Credits`, `Eridium_Currency`, `Health`, `sdu`, and `Buff_*` tiles (some of these only in other packages) | Probe; decoded and looked: a white line drawing of a generic gun of that type on a dark tile, tilted |
| Each `WeaponTypeDefinition` points at its tile through `PickupFlagIcon` (33 of 33 have it) | Probe (property census) |
| No per-manufacturer picture as a texture. `ManufacturerDefinition` has `FlashLabelName` (`vladof`), `IconX`, `IconY`: a frame label and a position for the Scaleform movie, so the maker marks are inside the movies | Probe |
| Skin decals exist that show maker logos: `Common_GunMaterials.Logos.*` (for example `Logo_VladofColor`, `Logo_TorgueColor`, and three-in-one textures such as `Logo_TedioreMaliwanVladof_Comp`). They are gun-skin decals, not UI art | Probe (texture list) |
| No per-element picture found as a texture | Same probe; the element marks are presumably vector art in the movies (not checked) |
| The inventory atlas `SharedWillowInventory.SharedWillowInventory_I7` (1024×1024) holds card frames and a dozen small symbols (ammo kinds, a star, a chest), no weapons | Decoded and looked |
| The inventory movie exists as `UI_Inworld.Weapon_Card`, `UI_ItemInspect.ItemInspect`, `SharedWillowInventory.SharedWillowInventory` (`SwfMovie` objects). Their bitmap references are the `<Movie>_I<hex>` textures above | Probe (class list). The vector shapes inside were not examined |

So the belief in the brief holds: the game shows the live 3D model. The per-type tiles are a cheap
fallback (seven line drawings), nothing more.

## 2. How a weapon's 3D model is defined

All in `Startup.upk`. The hint in the brief is right in every part.

| Fact | How checked |
|---|---|
| `Startup.upk` holds 17 `SkeletalMesh` objects (7.6 MB of its 60 MB) and 9 `GestaltSkeletalMeshDefinition` objects | Probe; tests `every_skeletal_mesh_in_startup_parses`, `gestalt_definitions_tile_their_meshes` |
| One gestalt definition per weapon class: `Weap_AssaultRifles.GestaltDef_AssaultRifle`, `Weap_Launchers.GestaltDef_Launcher`, `Weap_Pistol.GestaltDef_Pistol`, `Weap_Shotguns.GestaltDef_Shotgun`, `Weap_SMG.GestaltDef_SMG`, `Weap_SniperRifles.GestaltDef_SniperRifle`. Three more for items: `Item_Shields.Gestalt.GestaltDef_Shields`, `Weap_Grenades.GestaltDef_Grenades`, `Item_Artifacts.GestaltDef_Artifact` | Same |
| Every `WeaponTypeDefinition` has `GestaltMesh`, an object reference to its class's definition (33 of 33) | Probe (property census) |
| A definition has `GestaltSkeletalMesh` (the mesh, same package, named `…_GestaltSkeletalMesh`) and `GestaltInfos[0].Parts`: an array of `SkeletalMeshFragmentName`, `MaterialIndex`, `FirstIndex`, `NumPrimitives`. A fragment is a run of triangles in the mesh's index buffer. Also `GestaltSocketMappings` and `GestaltPartBounds` (not used here) | Probe; `GestaltMesh::from_export` |
| In all nine definitions the fragments are disjoint and together cover the whole index buffer | Test `gestalt_definitions_tile_their_meshes` |
| Fragment counts: assault rifle 47, launcher 40, pistol 59, shotgun 45, SMG 46, sniper 48, shields 32, grenades 15, relics 16 | Same test |
| A name can cover several runs: `Acc_Barrel_Elemental2` has 5 runs in the launcher mesh, `SR_Body_Jakobs` 3 in the sniper mesh. All runs of a name must be shown | Same test. Found when the test failed |
| A `WeaponPartDefinition` shows mesh only if `bIsGestaltMode` is true. Then `GestaltModeSkeletalMeshName` names one fragment and the static array `AdditionalGestaltModeSkeletalMeshNames[0…1]` up to two more (bodies use these for their `_Var1`, `_Var2` add-ons) | Probe over all 658 parts; `fragment_names` |
| Of the 658 weapon parts in `Startup.upk`, 428 show mesh and 230 do not. The 230 are the 211 material parts plus 19 "none" parts (`AR_Sight_None`, `AR_Stock_None`, …), which carry a fragment name but not the flag | Test `every_gestalt_part_names_fragments_that_exist`; probe |
| Every fragment name used by a gestalt-mode part exists in a gestalt definition (0 missing), for weapons and also for shield, grenade-mod and relic parts | Same test (weapons); probe (items) |
| The pieces need no placing. All fragments of a class are stored in one shared space where they already fit: every assault-rifle barrel starts at y = −17.0, stocks start between y = +11 and +14, sights sit at z ≈ 13. The muzzle points along −Y, +Z is up, X is the width | Probe (bounding box of every fragment); looked (the renders are whole guns) |
| So assembling a weapon is: type → gestalt definition; each part slot 0…7 → fragment names; draw those runs. Slot 8, the material part, has no mesh and names the skin (§5) | `WeaponSource::assemble`; test `known_weapons_assemble_from_their_own_parts` |
| `BoneControllers`, `VisibleAmmoBoneNames` and the sockets drive animation and effects (spinning barrels, visible rounds, muzzle flash). A still picture does not need them | Probe (read the properties) |
| The four fixture weapons resolve as: Gearbox Rifle = `AR_Body_Vladof`, `AR_Grip_Jakobs`, `AR_Barrel_Bandit`, `AR_Stock_Torgue`; Projectile Convergence = `SMG_Body_Hyperion`, `SMG_Grip_Bandit`, `SMG_Barrel_Tediore`, `SMG_Stock_Bandit`; Callipeen = `SR_Body_Jakobs`, `SR_Grip_Hyperion`, `SR_Barrel_Dahl`, `SR_Scope_Jakobs`, `SR_Stock_Vladof`; Infinity = `Pistol_Body_Vladof` + `_Var1` + `_Var2`, `Pistol_Grip_Hyperion`, `Pistol_Barrel_Vladof`, `Pistol_Scope_Dahl`, `Acc_Barrel_Laser` | Same test |

**DLC.**

| Fact | How checked |
|---|---|
| No DLC package holds a weapon gestalt definition. The only definitions outside `Startup.upk` are the class default in `GearboxFramework.upk` and `Lilac_Weap_BuzzAxe.GestaltDef_LilacBuzzAxe` (the Psycho's axe) | Probe over all 1,847 packages |
| The 21 DLC startup packages hold 252 weapon parts; 146 show mesh; **all of them name fragments of the base game's meshes** | Test `dlc_weapon_parts_reuse_the_base_meshes` |
| DLC skins have their own textures (`Orchid_GunMaterials`, `Sage_GunMaterials` in DLC packages), and DLC folders have their own texture caches (62 `.tfc` files under `DLC\`) | Probe; listing. Not read |

## 3. The mesh format (version 832, licensee 46)

Worked out from the bytes, starting from general knowledge of how Unreal Engine 3 lays out a
skeletal mesh (§8), and confirmed field by field: counts that match other counts, floats that are
sane, and a parse that ends where the vertex data ends. Little-endian. What follows the tagged
properties of a `SkeletalMesh`:

| Field | Size |
|---|---|
| Bounds: origin, box half-extent, sphere radius | 7 × f32 |
| Materials: count, then object references (0 = set at run time; the weapon meshes have one null entry) | i32 + 4 each |
| Origin (vector), rotation origin (rotator) | 12 + 12 |
| Reference skeleton: count, then per bone: name (FName), flags u32, rotation quaternion 4 × f32, position 3 × f32, child count i32, parent index i32, colour u32 | i32 + 52 each |
| Skeletal depth | i32 |
| LOD model count (1 in all 17 meshes) | i32 |
| **LOD 0.** Sections: count, then material index u16, chunk index u16, first index u32, triangle count u32, sorting u8 | i32 + 13 each |
| Index buffer: "needs CPU access" u32, index size u8 (2 here), then element size i32, count i32, indices | |
| Active bone indices: count, u16 each | |
| Chunks: count, then first vertex u32, two empty CPU-side vertex arrays (two i32 zeros), bone map (count, u16 each), rigid vertex count i32, soft vertex count i32, max influences i32 | |
| Size i32, vertex count i32 | |
| Required bones: count, u8 each | |
| Raw point indices: a bulk-data header (flags u32, count i32, size i32, offset i32); empty here | 16 |
| Texture-coordinate set count | i32 |
| Vertex buffer: set count u32, full-precision flag u32, "packed position" flag u32, mesh extension 3 × f32, mesh origin 3 × f32, element size i32, count i32, vertices | |
| **Vertex:** tangent (4 bytes), normal (4 bytes, each 0…255 → −1…1), 4 bone numbers (u8), 4 weights (u8), position 3 × f32, then per set a pair of half floats (or f32 with the full-precision flag) | 28 + 4 per set |
| The rest (colour and influence buffers, name map, …; 60 to 512 bytes) | not read |

| Fact | How checked |
|---|---|
| All 17 skeletal meshes in `Startup.upk` parse with this layout: the element size equals 28 + 4 × sets, the buffer's vertex count equals the declared one, every index is below the vertex count, sections cover the index buffer | Test `every_skeletal_mesh_in_startup_parses` |
| The assault-rifle section's triangle count (22,695) equals the gestalt definition's total, a value read from a different object | Probe |
| Vertex positions fall inside the stored bounds; normals are unit length except 1 to 7 vertices per weapon mesh stored with a zero normal; 0 to 10 vertices per mesh are used by no triangle | Same test |
| Texture-coordinate sets: 4 in the assault rifle, SMG and sniper meshes, 2 in pistol, shotgun and launcher. Set 0 is the texture atlas (0…1). Set 1 is a flat side projection used for patterns and decals. Sets 2 and 3 are zero or garbage | Probe (ranges per fragment) |
| The "packed position" flag is set on eight small item meshes (class mods, grenades); positions are 12 bytes regardless | Probe (element sizes 32 and 36) |
| Sizes: the six weapon meshes have 22,766 to 28,268 vertices and 20,174 to 25,227 triangles each; all nine gestalt meshes together 165,904 vertices and 429,459 indices. One assembled weapon is 2,000 to 3,700 triangles | Test output; `gmrender --bench` |
| Weapon vertices are skinned almost entirely to one bone each; the reference pose is the assembled pose, so no skinning is needed for a still | Probe (bone census per fragment); looked |

Parsing never panics on bad input: every truncation of a synthetic mesh, bad indices, bad bone
references and noise all return errors (unit tests in `skeletal.rs`).

## 4. Rendering

`gamemesh::render` is a software renderer with no dependencies: orthographic view, z-buffer,
supersampling, three light bands (a single light from the upper left), and ink outlines found
after drawing from depth steps, part boundaries, creases in the normals, and the silhouette.
Outside the silhouette it can draw a thin light rim, so a dark weapon still reads on a dark
page. The background is transparent (straight alpha). `gamemesh::png` writes 8-bit RGBA PNG
with a filter chosen per row and real deflate compression (`gamemesh::deflate`, §11.3).

Looked at (session scratchpad, default side view: the weapon's left side, muzzle to the left;
composited over the app's page colour `#1B140D` unless named otherwise):

| Picture | What it shows |
|---|---|
| `gearbox-rifle-1120x560-on-1b140d.png` | The Gearbox Rifle from its own four parts in its own skin, **yellow and black**: an AK-shaped receiver and curved magazine (Vladof body), a Bandit barrel with its fin, a Torgue stock, a Jakobs grip, no sight (the save has `AR_Sight_None`) |
| `gearbox-projectile-convergence-1120x560-on-1b140d.png` | Hyperion SMG body, Tediore barrel, Bandit grip with its magazine, Bandit wire stock |
| `gearbox-callipeen-1120x560-on-1b140d.png` | Jakobs sniper: revolver cylinder, Dahl barrel, long Jakobs scope, Vladof skeleton stock |
| `righteous-infinity-1120x560-on-1b140d.png` | The Infinity: Vladof body with both add-ons, twin barrel, Hyperion grip, Dahl red-dot sight, the laser accessory under the barrel; dark navy and pale blue. The navy barrel is told from the page by the light rim |
| `…-240x120-on-1b140d.png` (same four) | Thumbnail size. Still clearly each gun |
| `righteous-infinity-1120x560.png` and the other three without `-on-…` | The same on a transparent background, exactly what the app would get |
| `righteous-infinity-1120x560-right-on-1b140d.png` | The other facing: the right side, muzzle to the right (§11.6) |
| `righteous-infinity-1120x560-angled-on-1b140d.png` | Turned 28° and raised 14° |
| `…-1120x560-flat-on-1b140d.png` (four) | Flat zone colours only, no drawn lines |
| `wt-torgue-assaultrifle-4-…`, `wt-maliwan-launcher-5-…` | Built from part paths: the two "washed out" skins of the spike, after the tone change (§11.7) |
| `all-parts-gestaltdef-assaultrifle-on-1b140d.png` | All 47 assault-rifle fragments at once, one colour each: an unreadable pile of five rifles. This is what part selection removes |
| `sweep-gestaltdef-<class>.png` (six) | Twelve part combinations per weapon class from the sweep (§11.5) |

Plainly: the shapes are right and recognisable for all six weapon classes. Nobody has compared
any picture with the game side by side (§9).

## 5. Colour and texture

### 5.1 What the data says

| Fact | How checked |
|---|---|
| The material part (slot 8) is a `WeaponPartDefinition` with `PartType` `WP_Material` and a `Material` reference to a `MaterialInstanceConstant` under `Common_GunMaterials.Materials.<Class>.*` (211 parts carry `Material`; 95 of them state `PartType` `WP_Material`, the others leave `PartType` unset) | Probe; `Assembly::material` |
| Instances chain through `Parent`: `Mati_JakobsRare_SG` → `MasterMaterials.Jakobs.MasterMati_JakobsRare` → `Common_Materials.Weapons.Master_Gun`. 84 instances sit directly under `Master_Gun` (the masters among them), the rest under one of 39 maker-and-rarity masters. A child's value overrides its parent's | Probe; `MaterialParameters::read`; test `materials_and_textures_read` |
| Vector parameters (linear RGBA, often above 1): three colour zones with three tones each, `p_{A,B,C}Color{Hilight,Midtone,Shadow}`; `p_DColor`; decal and pattern colour, scale and position; `p_ReflectColor`; `p_EmissiveColor` | Probe (census over every `MaterialInstanceConstant` under `Common_GunMaterials`) |
| Scalar parameters: `p_HighlightsIntensity`, `p_ShadowsIntensity`, `p_DecalRotate`, `p_ReplaceDecal`, `p_ReplacePattern`, `p_UseFullColorDecal`, … | Same |
| Texture parameters: `p_Masks` (per class, `Weap_<Class>.Tex.Weap_<Class>_Comp`, 2048², DXT1), `p_Diffuse` (4096², DXT1, **shared by three classes**: `Common_GunMaterials.CompTextures.Weap_AssaultSubSniper_Comp` and `Weap_LauncherShotgunPistol_Comp`), `p_NormalScopesEmissive` (2048² normal map), `p_Pattern`, `p_Decal`, `P_SimpleReflect` | Same |
| The shared diffuse holds one greyscale drawing per channel. The static mask `p_WeapClassSelect` in each instance says which: assault rifle R, SMG G, sniper B; launcher R, shotgun G, pistol B, matching the texture names | Read from the instances' native data (`MaterialParameters::channel`) for all eight rendered weapons; looked (the blue channel of the second texture is a pistol atlas with hand-drawn lines) |
| `p_Masks` is two maps stacked. Lower half: the colour zones, almost pure red, green or blue (A, B, C). Upper half: two detail masks in red and green; the parameter node is described as "Hilight Shad" | Decoded and looked; test `materials_and_textures_read` (91 to 95 % of the lower half is one pure channel); the node description is in `Master_Gun` |
| Both halves and the diffuse are addressed by texture-coordinate set 0, the mask with v halved | Looked: the zones land on sensible parts (yellow body, black magazine and barrel) on every weapon tried. A wrong mapping gives confetti |
| **The shader itself is not in the packages.** `Master_Gun` lists 137 expressions, but only the 43 parameter nodes survive cooking; the wiring between them is gone | Probe |
| Textures: mips of 128 px and up are in `CharTextures.tfc` (808 MB) as LZO chunk streams, at the offset and size in the mip table (bulk flags `0x11`); mips of 64 px and down are inline. After the tagged properties: a 16-byte bulk header, mip count, then per mip the header (flags, byte count, size on disk, offset), inline data if any, width, height | `gamemesh::texture`; test `materials_and_textures_read` reads a 256 px mip from the cache and a 64 px mip inline |
| Elemental parts carry `MaterialVectorParameterValues` with `p_EmissiveColor` (fire: 4.02, 0.27, 0) for the glow | Probe. Not used |
| The pistol mesh has a second section (538 triangles, `Pistol_Body_Vladof_Var2`) with its own material slot | Probe. The prototype draws it with the weapon's skin |

### 5.2 What the crate does

`gamemesh::material::GunSkin` colours each drawn point from the weapon's material:

- **Flat:** the zone map picks A, B or C (or `p_DColor` where nothing is marked); the zone's
  midtone colour is used, darkened by light band.
- **Textured** (the default): as flat, then blended towards the zone's shadow and highlight
  colours by the two detail masks, then multiplied by the greyscale diffuse, which carries the
  game's hand-inked lines.

Colours above 1 are scaled back into range keeping their hue (§11.7). The mixing formula and the
tone curve are guesses made by eye. Zone layout and zone colours are the game's own.

### 5.3 Estimate

| Option | State | Remaining work |
|---|---|---|
| Flat toon colours from the material | Prototype works for the base game | 1 to 2 days: settle the tone curve against the game, a fallback when a material or texture is missing, tests |
| Real textures, as far as the prototype goes | Prototype works | Included above, plus a choice of texture size (§6) |
| Patterns and decals (camouflage, wood grain, maker logos, the Infinity's ∞ pattern) | Not started. Inputs are known: `p_Pattern`, `p_Decal`, their scale, position, rotation, channel masks, and coordinate set 1 | 3 to 5 days, and it will stay approximate without the shader. Sam's eye decides whether it is worth it |
| A faithful copy of the game's shading | Not possible from the packages alone | Would mean reading the compiled shader cache. Not recommended |

## 6. Cost at run time

Release build, this PC, files in the OS cache (`gmrender --bench`, two runs agreeing within 5 %).
"First" means first in the process: the textures are not yet in memory. A cold OS file cache was
not measured.

| Step | Time |
|---|---|
| `Pictures::open`: read `Startup.upk`, build the base item tables, take out all six weapon meshes, 658 parts, their materials and texture tables. The package is then dropped | 202 to 251 ms |
| 1120×560 textured, first: draw 246 ms + PNG 67 ms | 0.31 s |
| 1120×560 textured, again: draw 209 ms + PNG 66 ms | 0.28 s |
| 240×120 textured, first: draw 31 ms + PNG 4 ms | 35 ms |
| 240×120 textured, again: draw 23 ms + PNG 3 ms | 26 ms |
| 1120×560 flat / plain (draw only) | 118 ms / 76 ms |
| 240×120 flat / plain (draw only) | 13 ms / 9 ms |

Samples per pixel: 3×3 up to 290,000 pixels, 2×2 up to 650,000 (so 1120×560 is drawn at 2×2),
one above that; requests over 2.6 million pixels are scaled down. The renderer is
single-threaded; shading and outlining could be split over threads if a grid of thumbnails
feels slow (40 thumbnails are about one second).

**Memory.** Kept between calls: the weapon meshes, parts and tables (about 10 MB) and at most
64 MiB of decoded textures (`Pictures::TEXTURE_BUDGET`), least recently used dropped first. A
picture up to 480 px wide uses textures of at most 1024 px (4 MiB each), a larger one 2048 px
(16 MiB each); a weapon needs two. All six classes at thumbnail size fit together (32 MiB);
after drawing all six large, 64 MiB is held (test `textures_stay_within_the_budget`). While a
1120×560 picture is drawn the renderer's own buffers take about 100 MB for a moment (by
calculation: 2.5 million samples of 39 bytes; not measured).

**PNG sizes** (§11.3): 1120×560 textured 89 to 204 KB, flat 28 to 46 KB; 240×120 textured 8 to
19 KB.

**No disk cache is needed.** Nothing is written anywhere. If one is wanted later, it would hold
derived game data: local only, never committed (hard rule 4).

## 7. What was built

In the working copy only (`…/scratchpad/mesh-spike/`), nothing committed.

**Crate `crates/gamemesh`** (depends on `gamedata`, `savekit`, `lzo1x`; `#![forbid(unsafe_code)]`):

| File | Contents |
|---|---|
| `src/pictures.rs` | **The app's entry point.** `Pictures::{open, open_package, new, weapon_png, weapon_image, assembly_image, decode, style, set_style, skin_problem, texture_bytes, source}`, `TEXTURE_BUDGET`, `MAX_PIXELS`; `PictureError` with `message()`; `Style`, `Facing`, `Colouring`; `reduce_depth` |
| `src/weapon.rs` | `WeaponSource::{open, from_package, assemble, assemble_paths, gestalt, material, texture, database, package_dir, problems, weapon_types, gestalt_paths}`; `Assembly`, `AssemblyPart`, `PartLook`, `fragment_names`, `WEAPON_SLOTS`, `MATERIAL_SLOT` |
| `src/skeletal.rs` | `SkeletalMesh::{from_export, parse, bounding_box, second_uv}`; `Bone`, `Section`, `Chunk`, `Vertex` |
| `src/gestalt.rs` | `GestaltMesh::{from_export, fragments_named, indices_of}`; `Fragment` |
| `src/texture.rs` | `texture::Texture::{from_export, mip_for, decode}`; `Mip`, `Pixels::{sample, sample_linear, byte_len}`, `PixelFormat` |
| `src/material.rs` | `material::MaterialParameters::{read, vector, scalar, texture, channel}`; `GunSkin::{new, texture_paths}`, `SkinDetail` |
| `src/balances.rs` | `balances::weapon_balances`, `BalanceParts`: the parts each balance allows per slot (for the sweep) |
| `src/render.rs` | `render::{render, render_with}`; `Scene`, `Triangle`, `View`, `Image`, `Shader`, `Sample`, `PaletteShader`, `MAX_SIDE` |
| `src/picture.rs` | `picture::{weapon_scene, whole_gestalt_scene}`, `SLOT_COLOURS` |
| `src/deflate.rs`, `src/png.rs` | `deflate::{deflate, zlib}`; `png::encode_rgba` |
| `src/bin/gmrender.rs` | Developer tool: pictures from a save, a code or part paths; `--sweep`; `--bench` |
| `tests/formats.rs`, `tests/support/` | Every reader on hand-made packages, and the robustness tests (§11.4) |
| `tests/real_game.rs` | 13 tests against the install (skipped when it is absent) |

The spike's probe program (`gmprobe`) is gone.

Tests: 18 unit, 8 format and robustness, 13 install tests pass; `gamedata`'s 80 + 24 pass;
`clippy --all-targets` is clean for both crates.

**Changes outside `crates/gamemesh`:**

1. `Cargo.toml`: `"crates/gamemesh"` added to the workspace members (and `Cargo.lock` gained the
   package entry: `gamemesh` with dependencies `gamedata`, `lzo1x`, `savekit`).
2. `crates/gamedata/src/package/properties.rs`: one new public method,
   `UPackage::properties_and_native_data(index) -> Result<(PropertyList, &[u8])>`, which returns
   an export's tagged properties and the bytes after them, plus one unit test
   (`native_data_is_what_follows_the_properties`). 35 lines, nothing existing changed.

To regenerate the pictures, from the working copy:

```
CARGO_TARGET_DIR=/mnt/c/Users/<user>/AppData/Local/golden-key/target-mesh \
  scripts/cargo run -q --release -p gamemesh --bin gmrender -- "<output folder, Windows path>" \
  --save Tests/Fixtures/local/win-throwaway-Save0001.sav \
  --code 'BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)' \
  --size 1120x560 --size 240x120 --backdrop 1B140D
```

Add `--sweep` for the sweep and its contact sheets, `--bench` for the timings.

## 8. Sources

- No web page, repository or third-party source file was opened during this spike, and no code
  was copied from anywhere. The oracle repository was not consulted.
- Prior knowledge used as a starting hypothesis, then checked against the bytes: the general
  order of fields in Unreal Engine 3's skeletal mesh, texture mip and bulk-data serialisation, as
  described in Epic's public UDK-era documentation and widely repeated in community format notes;
  the S3TC (DXT1/DXT5) block layout; IEEE half floats.
- Specifications the PNG writer follows: RFC 2083 (PNG), RFC 1950 (zlib), RFC 1951 (deflate,
  stored blocks).
- Everything specific to this game (the gestalt mechanism, the stacked mask texture, the
  channel-packed diffuse, the static class mask) was found in the game's own data.

## 9. Not verified, not done

- **No picture has been compared with the game.** Shapes rest on "it is plainly that gun";
  colours rest on the data's own values and on the Gearbox guns coming out yellow and black. Sam
  should hold a few against the inventory screen.
- **Which facing the game's own inventory uses** is not known from the data (§11.6 settles left
  and right and that neither view is mirrored, not which one the game prefers).
- The colour formula (§5.2) is a guess. Patterns, decals, reflections, emissive glow (elemental
  parts) and the normal map are not drawn, so a skin whose character is its pattern (Torgue's
  chequers, Maliwan's hexagons, Jakobs wood grain) comes out plainer than in the game.
- Scope glass and similar effect meshes (`FX_WEP_Scopes.Meshes.*`, static meshes) are not drawn.
- **DLC weapons are refused** (`PictureError::DlcItem`). They need the DLC part definitions (in
  the DLC startup packages), DLC skins and DLC texture caches, and they inherit the open question
  of DLC resolution in `gamedata` (`DLC_RESOLUTION` is `Unverified`). The meshes themselves are
  the base game's (§2).
- The sweep (§11.5) checks that every allowed part assembles, draws, touches the rest and has a
  sensible size. It cannot see a part that is in the right place but looks wrong.
- Items: shields, grenade mods and relics have gestalt meshes of the same kind and their meshes
  parse, but no item was assembled or rendered. Class mods have four separate meshes and no
  gestalt.
- The static-mask block in a material instance is read by searching for the parameter's name,
  not by parsing the block.
- The bytes after the vertex buffer in a mesh, the socket and part-bounds arrays, and the
  Scaleform movies' vector art were not examined.
- Timings with a cold OS file cache (first read of `Startup.upk` after a reboot).
- Languages, other game versions and other platforms: not examined.

## 10. From here to a picture of every base-game weapon in the app

| Piece | Estimate |
|---|---|
| ~~Harden `gamemesh` for merging~~ | Done (§11) |
| Settle the look with Sam against the game: facing, angle, flat or textured, rim colour | 0.5 day |
| Wire into the app: a picture route on the window's protocol answered by a picture thread that owns one `Pictures`; an in-memory cache of finished PNGs by code and size; a placeholder while it draws and for the errors; thumbnail and detail sizes in the UI | 2 to 3 days |
| **Base-game weapons** | **about 3 to 4 days** |
| DLC weapons: part and material lookup across the DLC startup packages, DLC texture caches. Blocked on the in-game DLC check | 2 to 3 days more |
| Patterns and decals | 3 to 5 days more, approximate |
| Shields, grenade mods, relics | 2 to 3 days more (same mechanism; their materials were not looked at) |

Hard rule 4 applies to all of it: pictures are derived from Sam's own install at run time, kept
only in memory, and no game asset goes into the repo or the program.

## 11. Phase 2a: the crate made ready for the app

### 11.1 One entry point

`Pictures::open(&GameInstall)` then `pictures.weapon_png(code, width, height)` returns a PNG on a
transparent background, or a `PictureError`:

| Error | When | `message()` |
|---|---|---|
| `BadCode` | the text does not decode as an item code | "This is not a readable item code." |
| `NotAWeapon` | the code is an item, not a weapon | "Only weapons have pictures so far." |
| `NothingToDraw` | a placeholder entry (set 255), or no part with a model | "This item has no model to draw." |
| `DlcItem` | any set other than 0 | "Pictures of DLC weapons are not supported yet." |
| `UnknownParts` | the type or a part (slots 0 to 7) is in no table or not defined in `Startup.upk` | "The game files do not define some of this weapon's parts." |
| `GameData(detail)` | a mesh, definition or the package itself is missing or not as expected | "A game file is missing or not as expected, so the picture could not be made." |

Checked by test `what_cannot_be_drawn_says_why`. A missing or damaged **texture** does not fail
the picture: it is drawn in plain colours and `skin_problem()` says why (same test).
`Pictures` is `Send` (test `pictures_can_move_to_another_thread`), takes `&mut self`, and holds
no file open. Sizes are clamped, never refused (test `a_code_becomes_a_png`).

`WeaponSource` no longer keeps `Startup.upk` in memory (60 MB): everything needed is taken out
when it is opened, and `problems()` lists anything that could not be read (none on this install,
test `the_source_opens_without_problems`).

### 11.2 Transparent background and the dark page

Alpha is 0 outside the weapon, with an anti-aliased edge. On `#1B140D` a dark weapon and its
dark ink outline vanished, so the outside of the silhouette is drawn as a light rim
(`Style::rim`, default `#ECDEC4`, at least one output pixel wide) with the ink line inside it.
Looked: the Infinity's navy barrel is clearly outlined on the dark page, and the pictures still
look right on a light page. `Style::rim = None` gives ink on both sides, as in the spike.

### 11.3 PNG compression

`gamemesh::deflate`: LZ77 with hash chains (32 KiB window, lazy matching), then per-block
dynamic Huffman tables, or the fixed tables when those are smaller; std only. `gamemesh::png`
picks one of the five row filters per row by the usual smallest-sum rule.

| Fact | How checked |
|---|---|
| Round trip through an independent inflater written for the tests (stored, fixed and dynamic blocks): empty input, every length 0 to 599, runs, text, noise, repeats beyond the window, several blocks | Unit tests in `deflate.rs` |
| The test inflater reads streams it did not write (outputs of a reference zlib for `hello` and ten `a`s, embedded as bytes) | Same |
| Whole PNG files decode back to the same pixels, CRCs checked, with a decoder written for the tests | Unit tests in `png.rs` |
| A reference zlib (Python, outside the test suite) inflates the real pictures, and its own level 6 output for the same filtered data is 5 % smaller than ours, level 9 8 % smaller | Probe on the 1120×560 and 240×120 Infinity |
| Code lengths stay within 15 bits (7 for the code-length alphabet) on Fibonacci-like frequencies | Unit test |

Sizes, textured, as `weapon_png` writes them (colour kept to 6 bits a channel, alpha 8; the
texture grain hides the steps; `Style::colour_bits` changes it):

| Picture | 1120×560 | 240×120 |
|---|---|---|
| Infinity (the busiest) | 198 KB (8 bits: 288 KB; 7 bits: 238 KB) | 18 KB |
| Gearbox Rifle | 150 KB | 13 KB |
| Projectile Convergence | 155 KB | 14 KB |
| Callipeen | 89 KB | 8 KB |
| Infinity, flat colouring | 46 KB at 8 bits | 11 KB |

The spike's stored-block writer would give 2.5 MB for 1120×560 (it gave 1.28 MB for 800×400). Encoding takes 66 ms for the large
picture and 3 ms for the small one.

### 11.4 Never panicking on game data

Every reader returns an error on bad input. Reads are bounds-checked, every count is checked
against the bytes that remain before anything is allocated, sizes from files are capped (bones,
mips, mip sides and bytes, cache blocks), and outside the test modules there is no `unwrap` or
`expect`, and numbers taken from a file are checked before they are used as an index.

| Fact | How checked |
|---|---|
| A hand-made package with one object of each kind read here (mesh, gestalt definition, texture with an inline mip and one in a texture cache file, material instance with a parent and a static mask, weapon part, balance with a part list) reads correctly | Tests in `tests/formats.rs` |
| For each of its eight objects: every truncation, 400 random corruptions, every four-byte field set to four extreme values, and noise. Each damaged package that still parses (more than 5,000 do) goes through every reader, the renderer and the skin. Nothing panics | Test `truncated_and_corrupted_objects_never_panic` |
| The mesh reader alone: every truncation is an error; 20,000 random corruptions and every field set to extremes do not panic | Test `damaged_mesh_data_never_panics` |
| The texture cache file: every truncation is an error; extreme values in every field and 300 random corruptions do not panic; a cache name that tries to leave the folder is refused | Test `damaged_texture_cache_never_panics` |
| The renderer skips triangles with a non-finite corner and clamps absurd sizes | Unit test `rim_and_odd_input` |

The two asserts left are in `png::encode_rgba`, on the caller's own width, height and buffer
length, never on file data.

### 11.5 Sweep

For each of the 242 `WeaponBalanceDefinition`s in `Startup.upk`, the effective part list of each
slot was read (`balances::weapon_balances`: the nearest definition up the `BaseDefinition` chain
that enables the slot), and combinations were formed so that every part of every slot is used at
least once per balance.

| Fact | How checked |
|---|---|
| 242 balances, all with a known weapon type; 1,751 combinations; 652 of the 658 part definitions used | `gmrender --sweep` (8.7 s); test `every_balance_assembles_and_draws` |
| **All 1,751 assembled; none failed.** No part missing from `Startup.upk`, no fragment missing from a mesh, every combination has a material | Same |
| All 1,751 drew (160×80, plain colours). None empty; coverage 10 % to 46 % of the picture | `gmrender --sweep` |
| No combination has a part that does not touch the others (bounding boxes grown by 2 units) | Same |
| Sizes in model units, min / median / max. Length: assault rifle 97/106/142, launcher 182/191/220, pistol 35/46/108, SMG 73/80/100, shotgun 87/117/144, sniper 152/170/203. Height: 22 to 77 across all | Same |
| Looked at two contact sheets (pistols, launchers; twelve combinations each): whole, plausible guns in twelve different skins | `sweep-gestaltdef-pistol.png`, `sweep-gestaltdef-launcher.png` |

Nothing needed fixing. The longest "pistol" (108) is one with a blade or long barrel; the
sheets show such pistols as whole guns.

### 11.6 Orientation

**Conclusion: −X is the weapon's left side, +X its right. The default picture (muzzle to the
left) shows the left side as it is; the other facing (muzzle to the right) shows the right side
as it is. Neither is a mirror image.** Confidence: high. Three independent findings agree:

| Finding | How checked |
|---|---|
| The decal projection (texture-coordinate set 1) on the −X side runs left to right and top to bottom when the muzzle points left: u grows towards the stock on 92 to 100 % of level edges and v falls with height on 94 to 100 % of upright edges, in all six weapon meshes. So lettering in a decal reads correctly there with no flip | Test `the_default_view_is_the_unmirrored_left_side` |
| The game's material has a static switch named `sw_FlipDecalOnRightSide`. The same projection seen from +X runs right to left, which is exactly the side that needs the flip. So the game itself calls +X the right side | Probe (`Master_Gun` parameter list) plus the finding above |
| Shells leave to the right: all five assault-rifle `…_EjectPort` sockets are at X = +2.7 to +6.0. Launcher sights sit at X ≈ −20, on the left of the tube | Same test; probe |

Not settled: which facing the game's inventory screen uses. The game's own per-type tiles
(§1) draw the muzzle to the upper left, and decals are authored unflipped for that side, so
muzzle-left is the default here. The facing is a parameter (`Style::facing`).

No lettering was found in the drawn (diffuse) textures at the sizes looked at, so that check
was not possible.

### 11.7 The washed-out bright skins

Cause, from the data: in these skins the zone **midtones are neutral whites above 1**
(Torgue purple-rarity: A midtone 1.83, 1.83, 1.83; Maliwan blue-rarity: A 1.83, B 3.35), and
the colour sits in the highlight and shadow tones and in the pattern. The spike's tone curve
let 1.8 to 3.4 times the drawn grey through, which clipped to plain white and erased the lines.

Fix: every zone tone is scaled back to a peak of 1, keeping its hue, before the grey drawing is
applied. Looked: the Torgue rifle now shows its drawn lines and blue highlights on a white body
instead of a white blank.

What is left is real: these skins are white where the game would put their pattern (the Torgue
skin names `Pattern_TorgueEpic`, strength 1 on zone A; the Maliwan one `Pattern_HexScrollGrunge_Comp`).
Without the pattern layer they look plainer than in the game.

## 12. Phase 3: the pattern and decal layers

Sam's five weapons came out mostly white because the pattern layer was not drawn. The material
graph is stripped from the cooked packages, so what follows was reasoned from the parameter
names, the parameter values across all 248 gun material instances (chains merged, probe), and
the pattern textures themselves (decoded and looked at), then judged by eye.

### 12.1 Findings

| Fact | How checked |
|---|---|
| 36 distinct `p_Pattern` textures over the gun materials; 54 instances name none. The most used: `Pattern_BanditRare` (22), `Pattern_HexScrollGrunge_Comp` (17), `Pattern_JakobsUncommonWood` (12), `Pattern_CarbonHexCheck_Comp` (10), `Pattern_TorgueRare` (8), `Logo_MoxxiSwirl` (7) | Probe |
| Pattern textures are of two kinds. **Full colour** (`Pattern_JakobsCommon` wood, `VladovMetalCommon_Dif` worn dark metal, `Pattern_DahlCommon` desert camouflage, `Pattern_BanditRare` painted rust, `Pattern_TorgueEpic` red/white/black chequer, 64×16) and **packed masks** (`…_Comp`: a different grey pattern in each channel; `Pattern_HyperionStripeCommon`: one grey mask) | Decoded the inline 64 px mips and looked |
| The static mask `p_PatternChannel` says which: R, G and B on for a full-colour pattern (134 instances), a single channel for a packed one (R 68, G 10, B 10) | Probe over the instances' native data |
| `p_PatternChannelScale` (x, y, z, w) is the strength per colour zone A, B, C and the rest: Jakobs and Dahl common (1, 0, 0, 0), Vladof common (0.92, 0.85, 0.85, 0), Tediore common (0.54, 0.58, 0.64, 0.8) | Probe |
| `p_PatternScalePosition` is (tiling x, tiling y, offset x, offset y) on texture-coordinate set 1, the side projection: 4 to 17 repeats across a weapon | Probe; looked (the chequers run along the rifle, the camouflage blotches are weapon-sized) |
| `p_PatternColor` is nearly always above 1 (151 of 165). It is the colour a grey mask pattern is drawn in. For full-colour patterns it is **not** applied here: with it, Jakobs wood and Vladof metal come out pale cream; without it they are the dark wood and dark metal the game shows | Rendered both ways and looked (§12.3) |
| `p_ReplacePattern` is 1 on 11 instances, 0 or absent elsewhere: whether the pattern replaces the zone colour or multiplies it. Multiplying is the default: the Torgue purple skin's zone A is white 1.83 and its pattern colour 0.65 grey, which only makes sense as a product | Probe; reasoning |
| Decals: `p_Decal` (a logo texture, often a three-logo `…_Comp` pack), `p_DecalChannel` (which channel), `p_UseFullColorDecal` (the texture's own colours, else the channel as a mask drawn in `p_DecalColor`), `p_DecalScalePosition` (scale x, y, offset x, y on coordinate set 1, one copy, not tiled), `p_ReplaceDecal` (0.29 to 2.0; read as opacity), `p_DecalRotate` (not drawn), and the static switch `sw_FlipDecalOnRightSide` (on for 55 instances) | Probe |
| `p_HighlightsIntensity` and `p_ShadowsIntensity` read as **exponents** on the two masks, not multipliers: the Jakobs skins carry a shadow intensity of 8 with a blue-grey shadow tone, and under any multiplier reading their wood turns blue. As exponents, a high value keeps the tone to where the mask is strongest | Rendered both ways and looked |
| A switch (`sw_…`) sits in the static block as name, value, override flag, GUID: two flag words, not four | Hexdump of the Infinity's material (§5.1) |
| Four pattern textures (`Pattern_RedSpectrum` and three more 256×2 gradients) are `PF_A8R8G8B8`, uncompressed B, G, R, A | Probe; `the_source_opens_without_problems` failed on them until the format was added |

### 12.2 What is drawn now

Per colour zone: tones (midtone, shadow and highlight by their masks), then the pattern at its
strength for that zone (texture × `p_PatternColor` for a grey mask, the texture's own colours
otherwise; multiplied into the zone colour, or replacing it with `p_ReplacePattern`). The zones
are then mixed by the zone map, the decal is laid over the result where its square lands
(mirrored on the +X side when the switch says so), and the greyscale drawing is applied as
before. Skins without a pattern or decal draw exactly as before. Plain and flat colouring are
unchanged.

### 12.3 Judged by eye

Sheets on `#1B140D` in the session scratchpad,
`…/scratchpad/mesh-shots/patterns/{before,after,v2-exponent-nocolour}/`:

- `maya-weapons.png`: Sam's five. Before: three mostly white. After: the Torgue rifle has its
  red-striped racing chequer, the Tediore pistol its carbon-fibre weave, the Dahl SMG orange
  and tan camouflage with a yellow "DAHL" decal that reads the right way round, the Vladof
  pistol a small red decal on white steel, the Maliwan SMG its hexagon grunge on white and blue.
- `makers-common-pistols.png`: one common pistol per maker. After: Jakobs has a wood grip and
  steel body, Vladof worn brownish metal, Dahl desert camouflage, Torgue a grey chequer, Bandit
  painted rust; Hyperion (yellow), Tediore (white) and Maliwan (white and blue) have no pattern
  and did not change. These match the game's skins as I remember them; no side-by-side with
  the game has been made.
- `after/` is the first attempt (multiplier intensities, `p_PatternColor` on everything):
  Jakobs and Vladof stayed pale. `v2-exponent-nocolour/` is what is now in the code.

The "DAHL" lettering reading correctly in the default view is one more sign that the view is
not mirrored (§11.6).

### 12.4 Timings and sizes

Release, warm, Infinity (which has a pattern and a decal): 1120×560 draw 209 → 258 ms,
240×120 draw 23 → 29 ms. Pattern and decal textures are read at up to 256 px (a quarter MiB
each decoded); decoded textures held after the benchmark: 40 MiB of the 64 MiB budget. The
Infinity's 1120×560 PNG shrank from 198 to 162 KB (its pattern breaks up the flat areas less
than the grain did).

### 12.5 Still approximate

- The tone and layer formulas are reasoned, not read from the shader: the exponent reading
  of the intensities, the pattern multiplying the zone colour, the decal opacity, and leaving
  `p_PatternColor` off full-colour patterns are all choices judged by eye.
- `p_DecalRotate` is not applied. Decal placement follows `offset + uv × scale` on coordinate
  set 1, which puts the DAHL and Vladof decals in sensible places; other decals were not
  checked one by one.
- Reflections (`P_SimpleReflect`), emissive glow and the normal map are still not drawn.

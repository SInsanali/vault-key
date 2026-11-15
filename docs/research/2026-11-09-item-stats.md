# Item card stats: how the game computes them (2026-11-09)

**Question.** A save stores a weapon's type, balance, parts, manufacturer and level, but not its
damage, accuracy or any other number on its card. Where do those numbers come from, and can
Golden Key compute them from the user's own install?

**Answer: yes for weapons and for a shield's three main rows.** The game derives the card from
definitions in `Startup.upk` (and the DLC startup packages), by rules that are partly in its
script code (read here) and partly in native code (not read; inferred from the data and checked
against six cards other players posted). `crates/gamedata` now extracts those definitions and
computes the card: `ItemDatabase::stats`.

**Status: computed, unverified.** Nothing here has been compared with a card in the running game
on this PC. The outside comparisons are cards other players posted: two level-80 Infinity
pistols (§7.2) and four guns from two screenshots (§7.3). Five of the six are reproduced in
every row; the sixth in every row that can be read; one of them also in its five lines below
the rows (§14.3). §8 is the table for Sam to check in game. Grenade mods, class mods, relics
and the lines below a card's rows are §14.

Facts only. Each says how it was checked. What was not checked is in §12. The install was only
read. Checks were made with the crate's reader: tests in `crates/gamedata/tests/real_game_stats.rs`
(named *test `name`*; run with `--nocapture` for the figures) or throwaway probes that were
deleted afterwards (*probe*). The probes were a Rust program that dumped every export's tagged
properties as text, and a UnrealScript bytecode disassembler written for this (Python, standard
library only).

---

## 1. Reading the game's script code

The rules for *when* things are applied are UnrealScript, compiled to bytecode in `WillowGame.upk`
and `Engine.upk`. A disassembler for it was written from the public UE3 token layout.

| Fact | How checked |
|---|---|
| A `Function` export's data is: a u16 count and that many u16 values, then the net index, the name `None`, then `Next`, `Super`, `ScriptText`, `Children`, `CppText`, line, text position (i32 each), the in-memory script size, the on-disk script size, the script, then u16 native index, u8 operator precedence, u32 function flags | Probe: byte dumps of several functions; the layout parses all of them |
| Object references in bytecode are 4 bytes on disk | Same |
| This build adds tokens the stock engine does not have: `0x4C`–`0x50` are local variables by index (int, float, byte, bool, other), each followed by 4 bytes, because local variable objects are stripped from the package; `0x5E` + object is an attribute property; `0x5F` assigns one | Probe: with these, **all 11,945 functions of `WillowGame.upk` and all 5,654 of `Engine.upk` disassemble to exactly their script size** |
| Which functions are script and which native (function flag `0x400`): script are `WillowWeapon.CalculateWeaponBaseValues`, `InitializeInternal`, `InitializeFromDefinitionData`, `ApplyAllWeaponAttributeEffects`, `ApplyNamePartWeaponAttributeEffects`, the `WillowItem` equivalents, `AttributeEffect.ApplyAttributeEffects`, `WillowInventory.GetControllerPlayerExpLevelRequiredToUse`. Native are `EvaluateInitializationData`, `AddAttributeModifier`, `InitializeAttributeSlots`, `ApplyInternalSlotEffectModifiers`, `GetExpLevelFromManufacturerData`, `GetInventoryStatNumberData`, `CacheWeaponCard`, and every function of `AttributePresentationDefinition` (`RoundValue`, `RemapValue`, …) | Probe: function flags |

So: *what feeds what, and in which order* is read from script. *How a formula is evaluated, how
modifiers combine, how slots get their grade and how the card rounds* run in the executable and
are inferred (§4, §5), then checked in §7.

## 2. Where the numbers come from

### 2.1 Base values: the weapon type

`WillowWeapon.CalculateWeaponBaseValues` (script) copies the `WeaponTypeDefinition`'s properties
into the weapon's attribute properties. The ones the card uses:

| Weapon property | From the type's | Kind |
|---|---|---|
| `InstantHitDamage` | `InstantHitDamage` | value struct (§2.2), evaluated |
| `StatusEffectDamage` | `StatusEffectDamage` | value struct |
| `BaseStatusEffectChanceModifier` | `BaseStatusEffectChanceModifier` | value struct |
| `ClipSize` | `ClipSize` | int |
| `FireInterval` | `FireRate` (seconds between shots) | float |
| `ReloadTime` | `ReloadTime` | float |
| `Spread` | `Spread` | float |
| `ProjectilesPerShot` | `ProjectilesPerShot` | int |
| `ShotCost` | `ShotCost` | int |
| `PerShotAccuracyImpulse` | `PerShotAccuracyImpulse` | float |

Checked by disassembling the function. `StatusEffectChanceModifier` is not set from the type; it
keeps the weapon class's default, 1.0 (test `weapon_class_defaults`).

A property a type does not set has its class default (`WillowGame.Default__WeaponTypeDefinition`):
`FireRate` 0.4, `ReloadTime` 2.1, `Spread` 0.01, `ProjectilesPerShot` 1, `ShotCost` 1, `ClipSize` 0,
and constants 20 / 1 / 1 for the three value structs. The crate hard-codes these;
test `weapon_type_class_defaults` checks them against the install. Of the 33 base-game weapon
types, all set `ClipSize` and `Spread`, 32 set `FireRate`, 31 `ReloadTime`, 6 `ProjectilesPerShot`,
none `ShotCost` (probe census).

Typical values (probe): a Vladof assault rifle has clip 20, interval 0.115 s, reload 3.5 s,
spread 2.0, damage scale 1.08; a Jakobs sniper rifle clip 6, interval 0.3 s, reload 5.0 s,
spread 0.8, damage scale 4.0; a Bandit shotgun 6 projectiles.

### 2.2 `AttributeInitializationData`: how a number is stated

Every tunable number is a struct of four fields: `BaseValueConstant`, `BaseValueAttribute`,
`InitializationDefinition`, `BaseValueScaleConstant` (defaults 0, none, none, 1).

| Fact | How checked |
|---|---|
| The struct has exactly these four fields, with these defaults | Probe: the `ScriptStruct`'s properties and its serialised defaults in `Engine.upk` |
| A struct that is a top-level property stores only the fields that differ from the *class default's* struct; inside an array every field is written | Probe: dumps (a weapon type's `InstantHitDamage` has two fields; array elements have all four) |
| **The attribute replaces the constant; it is not added.** Formulas keep stale constants beside attributes: the shield capacity formula's `Level` has constant 85 *and* the attribute for 1.13, its `Power` constant 1 *and* the item-level attribute. Adding would give 86.13^(level+1) | Probe dump of `Init_ShieldItem_CapacityMaxValue`. Same conclusion as the rarity work (windows doc §6.3) |
| Evaluation order adopted: (attribute, else constant) → initialisation definition, if any → × scale | *Assumed* from the field names; consistent with §7.2 (the damage scale 1.08 … 4.0 multiplies the formula's result) |

### 2.3 `AttributeInitializationDefinition`: the level formula

| Fact | How checked |
|---|---|
| Fields: `BaseValueMode`, `RoundingMode`, `ValueFormula` (`bEnabled`, `Multiplier`, `Level`, `Power`, `Offset`: each a value struct), `ConditionalInitialization`, `RandomVariance`, `RangeRestriction` (`bEnableMinValueRestriction`, `MinValue`, `bEnableMaxValueRestriction`, `MaxValue`) | Probe: class properties in `Engine.upk` |
| **The formula is `Multiplier * ( Level^Power + Offset )`.** The class default object carries it as a string, with the note "Ignored if BalanceFormula is enabled; first expression set where all are true wins" on the conditional part | Test `presentation_class_defaults` (reads `ValueFormula.Formula`) |
| `EBaseValueMode`: definition sets / adds to / scales / is offset by the base value. Default: sets. `EAttributeInitializationRounding`: float, round, floor, ceil. Default: float | Probe: enum names in order; the class default sets neither |
| **Weapon damage: `Init_WeaponDamage` = 8 × 1.13 ^ level.** `Multiplier` 8, `Level` = `Att_UniversalBalanceScaler` (a constant resolver: **1.13**), `Power` = `D_Attributes.Weapon.WeaponLevel` (the weapon's `ExpLevel`), no offset | Test `stat_data_shape` |
| Its `MinValue` of 5 is stated but its restriction flag is off | Same test |
| Every one of the 33 base weapon types uses `Init_WeaponDamage` for `InstantHitDamage` (times a per-type scale, 0.75 … 35) and for `StatusEffectDamage` (scale 1) | Probe census |
| Of 147 initialisation definitions in `Startup.upk`: 107 use the formula, 39 conditional initialisation (7 of them in offset-by-base mode), 1 random variance | Probe census. The crate evaluates the formula with the sets / adds / scales modes; the rest make a value unknown |

So a weapon's base damage is `8 × 1.13^level × (type scale)`. Sam's "about 1.13 per level" is exact.

### 2.4 The level

| Fact | How checked |
|---|---|
| `InitializeFromDefinitionData` sets `GameStage` from the saved game stage, and `ExpLevel = BalanceDefinition.GetExpLevelFromManufacturerData(Manufacturer, ManufacturerGradeIndex)` | Disassembly. The function called is native |
| The level formulas read `ExpLevel`, never the game stage | `WeaponLevel` is an object-property resolver for `ExpLevel` (test `stat_data_shape`); the same for the shield's `ShieldItemLevel` (probe) |
| The level requirement shown is `max(ExpLevel − floor(PlayerUseLevelBonus), 1)` | Disassembly of `GetControllerPlayerExpLevelRequiredToUse` |
| Every base-game weapon balance has one grade per manufacturer, for stages 1…10000, with `GradeModifiers.ExpLevel` 0 | Test `weapon_balance_grades_add_no_level` (34 balances, 34 grades) |
| Shield balances have one grade with `ExpLevel` 1 | Probe |

**Adopted: `ExpLevel` = the saved grade index** (`ItemFields::grade_index`). For weapons the
grade can only add 0. It is confirmed at level 80 by §7.2. For shields the grade says 1, yet a
shield and a weapon with the same grade index show the same level requirement through the same
bonus formula, so the 1 cannot be added; this is an argument, not a check (§12).

### 2.5 Attributes

An effect names the attribute it modifies; a formula reads attributes. An `AttributeDefinition`
has a `ContextResolverChain` (whose property) and a `ValueResolverChain` (how to get the value).

| Resolver | Meaning | Used for |
|---|---|---|
| `ObjectPropertyAttributeValueResolver` (`PropertyName`) | a property of the context object | every weapon stat (`WeaponDamage` → `InstantHitDamage`, `WeaponFireInterval` → `FireInterval`, `WeaponReloadSpeed` → `ReloadTime`, `WeaponSpread` → `Spread`, `WeaponClipSize` → `ClipSize`, …), the level |
| `ConstantAttributeValueResolver` | a constant | 1.13, the shield delay base 3.5 |
| `ManufacturerAttributeValueResolver` | a value per manufacturer name, else `ValueIfNotMatched` | `Weapon_Is_Jakobs` and the like: 1 on a gun of that maker, else 0 |
| `SimpleMathValueResolver` | two values combined | `WeaponCombinedStatusEffectChanceModifier` = base chance × chance modifier |

Checked by probe dumps and by what the extraction stores (test `stat_data_shape`). The
manufacturer names (`Jakobs`, `Bandit`, …) are the manufacturer definitions' object names; the
crate matches them without case (*assumed*: the definition has no other name field except the
lower-case `FlashLabelName`; §7.2 exercises it for Vladof).

Of the 159 attributes the extracted definitions use as values, 110 are one of these four forms.
The other 49 (ammo upgrades, slot-grade resolvers, attributes with no resolver) are not used by
any weapon row.

### 2.6 Part effects

Every weapon type and part (and name part) has `WeaponAttributeEffects`: entries of
`AttributeToModify`, `ModifierType` (`MT_Scale` default, `MT_PreAdd`, `MT_PostAdd`) and
`BaseModifierValue` (a value struct). Items have `ItemAttributeEffects`. Both also have
`ExternalAttributeEffects`, which act on the carrier (accuracy pool, critical hit bonus), and
weapons have zoom variants that apply only while aiming.

| Fact | How checked |
|---|---|
| In the base game, a weapon effect's value is always a constant, or `Weapon_Is_<maker>` × a scale. No effect uses an initialisation definition | Probe census over 1,508 weapon parts and 33 types |
| What parts modify, by count: damage scale (113), magazine pre-add (79), shot cost pre-add (42), spread pre-add (38), reload scale (38), projectiles pre-add (33), status chance scale (30), …; one post-add among all weapon effects (the Infinity's magazine) | Same |
| Example: the Jakobs assault-rifle grip has reload scale `Weapon_Is_Jakobs × −0.3`, magazine pre-add `Weapon_Is_Jakobs × 2`: nothing on a gun of another maker | Probe dump |
| All 342 attributes that some definition modifies exist in the packages as attribute definitions; all 28 attributes and 37 initialisation definitions used as values were found | Test `referenced_attributes_resolve` |

### 2.7 Attribute slots (the "grade" system)

A type has `AttributeSlotEffects`: per named slot, an attribute, a modifier type, a
`BaseModifierValue` and a `PerGradeUpgrade`. Parts (and types) have `AttributeSlotUpgrades`:
`SlotName`, `GradeIncrease`, `bActivateSlot`.

| Fact | How checked |
|---|---|
| A weapon type has 11 slots (28 types) or 12 (the 5 launchers): `WeaponDamage` (+3% per grade on a Vladof rifle), `WeaponFireRate` (−3% fire interval), `WeaponMagSize` (+5%), `WeaponReloadSpeed` (−5% reload time), `WeaponSpread` (−5%), three accuracy-pool slots, two impulse slots, `WeaponZoomFOV`, and on launchers `WeaponRocketSpeed`. All are scale modifiers with base 0 | Probe census |
| Parts move slots by whole grades, both ways: the Jakobs AR grip is damage +4, fire rate −3, reload −1 | Probe dump |
| Slot effects are applied by native code, after the part effects (§3) | Disassembly of `InitializeInternal` |
| **Adopted rule:** a slot's grade is the sum of every `GradeIncrease` for it over the type and all parts; a slot is in effect only if some upgrade has `bActivateSlot`; the modifier is `BaseModifierValue + PerGradeUpgrade × grade`; `MinimumGrade` / `MaximumGrade` clamp the grade when their flags are set | *Assumed* from the field names. §7.2 confirms the arithmetic for a weapon (the Vladof grip's slots). The activation rule matters for shields, where a slot's base value is not 0 (a resistance slot adds −1 only when a part activates it) |
| The struct's defaults enforce a minimum grade of 0; weapon types switch that off explicitly | Probe: struct defaults; dumps |
| `AttributeSlotMaxActivated` is 19 by default and no gear type has that many slots | Test `presentation_class_defaults`; probe (only storage-deck upgrades set it, to 2) |

This is the only "grade" that feeds the numbers. The saved *manufacturer grade index* is the
level (§2.4); a `ManufacturerDefinition`'s `Grades` only hold display names.

### 2.8 Balance definitions

A balance's part list decides which parts a drop can roll; a save already holds the parts. The
balance contributes to the card only through the level (§2.4). Nothing else of it is read.

## 3. Order of application (script)

`WillowWeapon.InitializeFromDefinitionData` → `InitializeInternal`, for a weapon built from saved
data (`bSelectRandomPartsOnInitialization` false), disassembled:

1. `ExpLevel` and `GameStage` are set (§2.4).
2. `CalculateWeaponBaseValues` (§2.1). The damage formula is evaluated here, with the level.
3. `InitializeAttributeSlots(true)` (native): slot grades, name parts included.
4. `ApplyAllWeaponAttributeEffects(true)`: the type's `WeaponAttributeEffects`, then each part's,
   name parts included. Each effect's value is evaluated when it is applied
   (`AttributeEffect.ApplyAttributeEffects`, script) and added to the attribute as a modifier.
5. `ApplyInternalSlotEffectModifiers` (native): the slot effects.
6. `CacheWeaponCard` (native).

External effects (`ApplyAllExternalAttributeEffects`) are applied to the carrier when the weapon
is equipped. They change the player's accuracy pool and critical hits, not the card's rows.

`WillowItem.InitializeInternal` has the same shape with `ItemAttributeEffects`.

## 4. How modifiers combine (native; inferred, then checked)

**value = (base + Σ pre-add) × (1 + Σ positive scale) ÷ (1 + Σ |negative scale|) + Σ post-add**

Positive and negative scale modifiers are summed separately. A negative scale divides.

| Evidence | How checked |
|---|---|
| A plain sum cannot be right: sights put a `FootSpeed` scale of −2.0 on the player while aiming (a sum gives −1 × speed), and the Infinity barrel puts a scale of −3.0 on the accuracy pool's maximum | Probe dumps |
| The Infinity's "one-round magazine" is a magazine scale of −1000 plus a post-add of 1: the magazine ÷ 1001, plus 1, floored, is 1 (1.036 before the floor on the gun of §7.2). That idiom only makes sense with division, and with the post-add applied last | Probe dump; test `known_item_cards` (magazine 1) |
| **Two posted cards match only this rule.** With the element's −20% damage scale beside the positive ones, the rule gives 277805 and 282918, the posted numbers. Netting the scales first gives 292462 and 298598 | §7.2, test `published_infinity_cards` |

Sam's remembered formula, (base + Σ pre-add) × (1 + Σ scale) + Σ post-add, is right while every
scale is positive.

**Precision.** The game's properties are 32-bit floats, but two posted cards (§7.3) show values
that pure 32-bit arithmetic misses by one in the last digit: a magazine of 24 × 1.25 ÷ 1.2 shown
as 25 (32-bit: 24.999998, floored to 24), and a reload time of 3.0 × 1.05 shown as 3.2 (32-bit:
3.1499999, printed 3.1). Both come out right when each expression is evaluated in 64 bits and
only its result is rounded to 32, which is what a 32-bit x87 program does. The crate now
computes that way: the level formula, a slot's value, the modifier formula, and each
presentation step are each one 64-bit expression stored as a 32-bit float. That this is the
game's mechanism is an inference from those two cases (§12).

`ClipSize`, `ProjectilesPerShot` and `ShotCost` are integer attribute properties; the crate
keeps the modified value as a float and lets the card's floor rounding make it whole.

## 5. The card

### 5.1 Rows: `UIStatList`

A type's `UIStatList` names the attributes of the card's top section, in order. Each entry has
an `AttributeStyle`, and optionally a `SupplementalAttributeToAppend` with a condition.

| Fact | How checked |
|---|---|
| **Every weapon type, of every kind, lists the same seven rows:** `WeaponDamage` (with `WeaponProjectilesPerShot` appended when it is greater than 1), `WeaponSpread`, `WeaponFireInterval`, `WeaponReloadSpeed`, `WeaponClipSize`, `WeaponStatusEffectDamage`, `WeaponCombinedStatusEffectChanceModifier`. All are `ATTRSTYLE_JustNumber` | Test `stat_data_shape` (all 35 located weapon types, DLC included) |
| A shield lists `ShieldMaxValue`, `ShieldOnIdleRegenerationRate`, `ShieldOnIdleRegenerationDelay` as plain numbers, then its special effect, mostly as `ATTRSTYLE_ModifierText` | Probe census of the 33 shield types |

### 5.2 Presentation: `AttributePresentationDefinition`

`GD_Globals.General.Globals` names seven lists of presentations: a default one and an override
per kind (weapon, item, shield, grenade mod, class mod, artifact). A presentation names its
attribute and says how to show it. For a card of some kind, the crate takes the first entry for
the attribute in that kind's override list, else in the default list (*assumed* lookup order).

| Row | Presentation (default list unless noted) | Shown as |
|---|---|---|
| Damage | `AttrPresent_WeaponDamage`: `ATTRROUNDING_IntCeil` | rounded **up** to a whole number |
| Accuracy | `AttrPresent_WeaponSpread`: remapping enabled, input 0…15 → output 100…0, `ATTRROUNDING_Float` | **100 × (1 − spread ÷ 15)**, one decimal |
| Fire Rate | `AttrPresent_WeaponFireRate`: `bDisplayAsInverse`, float | **1 ÷ fire interval**, one decimal |
| Reload Speed | `AttrPresent_WeaponReloadSpeed`: float, `bBiggerIsBetter` false | the reload time in seconds, one decimal |
| Magazine Size | `AttrPresent_WeaponClipSize`: `ATTRROUNDING_IntFloor` | rounded down |
| "x7" after damage | `AttrPresent_WeaponProjectilesPerShot`: prefix `x`, floor | `x` + projectile count |
| Elemental damage | weapon override list, by damage type: "Burn / Corrode / Shock Damage / sec.", float | damage per second, one decimal |
| Elemental chance | weapon override list, by damage type: "Ignite / Corrode / Electrocute / Slag Chance", remapping 0…5 → 0…100 (slag: 0…3.33), float, suffix `%` | percent, one decimal |
| Capacity, Recharge Rate | `GD_AttributePresentation.Shields.…` | whole numbers |
| Recharge Delay | same, float with precision 2 | two decimals |

Checked by probe dumps of the presentations and by the extracted data (tests `known_item_cards`,
`stat_data_shape`). Class defaults (`RoundingMode` round-to-nearest, `FloatPrecision` 1, no
remapping, not inverse): test `presentation_class_defaults`.

The remapping, inversion and rounding functions are native. Adopted: remap linearly and clamp to
the range; float rows print with `FloatPrecision` decimals, rounded to nearest. Rounding to
nearest (not truncation) is confirmed by §7.2: the Dva Infinity's fire rate is 8.96 before
rounding and its card says 9.0. `bDisplayAsPercentage` (default true) is not applied to these
rows: it is for modifier text, and would turn damage 12.5 into 1250.

A presentation's `Suffix` follows the number: the elemental chance prints as "15.6%" (seen in
§7.3). The crate appends it to the row's display text.

**Labels.** The crate labels a row with its presentation's `Description`. For the five main rows
that is the card's wording. For the elemental rows it is not: the data says "Burn Damage / sec."
and "Ignite Chance", the card in §7.3 says "Burn Dmg/Sec" and "Chance to Ignite". Those two
strings are not in any tagged property of `Startup.upk` or `WillowGame.upk`, in any localisation
or configuration text file of the install, or in the executable (probe searches); where the card
takes them from was not found. The labels for corrosive, shock and slag were not seen.

The accuracy divisor is **15** in this game. (A wiki page gives spread ÷ 12 for the first game.)

### 5.3 Elemental rows

The two status rows use `WeaponStatusEffectAttributePresentationDefinition`: a table from damage
type to presentation. A damage type with no entry gets no row.

| Fact | How checked |
|---|---|
| The damage-per-second table has incendiary, corrosive and shock. The chance table has those and slag (`Amp`). Neither has explosive or normal | Probe dump |
| A weapon's damage type is its elemental part's `CustomDamageTypeDefinition`, else the type's `DefaultDamageTypeDefinition`. In the base game only `WP_Elemental` parts set one (6 parts for each of 4 elements); 25 types default to normal and 8 to explosive | Probe census |
| Damage per second = `StatusEffectDamage` = 8 × 1.13^level, times the parts' scales. Chance = base chance (0.3 on a Vladof rifle) × chance modifier (1, times the parts' scales), shown as a share of 5 | §2.1, §2.3, §5.2 |

So a non-elemental gun shows five rows, a fire, corrosive or shock gun seven, a slag gun six.

### 5.4 Other text on the card

`CustomPresentations` on types and parts are extra lines. An entry with no attribute and
`bDontDisplayNumber` is fixed text: red flavour text (`TextColor` #DC4646, priority 20, on title
parts), "Sustained fire increases accuracy." (Hyperion types), and so on. Of 851 such objects in
`Startup.upk`, 548 are localisation keys (`$WillowGame.…`, mostly skin descriptions), 273 are
literal text and 30 show a number (probe census).

The crate returns the literal fixed lines, markup (`[skill]…[-skill]`) removed. The lines the
game composes from numbers ("+8% Critical Hit Damage", "Consumes 2 ammo per shot.") are §14.

## 6. What the card shows per kind, and what is computed

| Kind | Rows the game lists | Computed |
|---|---|---|
| Pistol, SMG, assault rifle, sniper rifle, shotgun, launcher | Damage (with "xN" for several projectiles), Accuracy, Fire Rate, Reload Speed, Magazine Size; plus elemental damage per second and chance when elemental | **All of them**, for every base-game type and part (§7.1) |
| Shield | Capacity, Recharge Rate, Recharge Delay; a special row per shield kind | All four (§14; the special row where its attribute resolves) |
| Grenade mod | Grenade Damage (with "xN" child grenades), Blast Radius, Fuse Time | All (§14) |
| Class mod, relic | Stat bonuses as modifier rows ("+26% Shield Recharge Rate") | All whose attribute resolves (§14) |
| Lines below the rows | §5.4, §14 | Fixed text, bonus lines, skill lines, damage-type lines; not zoom lines |

## 7. Verification

### 7.1 Internal consistency

| Check | Result | How |
|---|---|---|
| Every base weapon type × every base weapon part, one part at a time, level 30 | 49,764 cards, 233,376 rows computed, 0 unknown except the 102 table entries whose definitions are in no startup package (windows doc §5: "located" column). 2,823 rows (1.2%) lie on a rounding boundary (§12) | Test `every_weapon_part_evaluates` |
| The same for each DLC set (under the unverified DLC hypotheses) | All rows compute, except unlocated parts and 23 pairings where a part drives another kind's fire interval to zero or below | Same test |
| Every shield type × every base item part | 28,281 cards; capacity, rate and delay always compute | Test `every_shield_part_evaluates` |
| No definition has anything the extraction could not read | 4,501 definitions, 0 with problems | Test `stat_data_shape` |
| References resolve | §2.6 | Test `referenced_attributes_resolve` |

The pairings are not all ones the game rolls (a pistol grip on a launcher). The point is that
nothing any base-game part or type refers to is beyond the calculation.

### 7.2 Two posted cards, reproduced

A player posted two cards in April 2014, when the level cap with Overpower levels was 80
([Steam discussion](https://steamcommunity.com/app/49520/discussions/0/558753804482920402)):

| Posted | Damage | Accuracy | Fire Rate |
|---|---|---|---|
| Vengeful Infinity (elemental) | 277805 | 89.6 | 10.4 |
| Dva Infinity (elemental) | 282918 | 89.6 | 9.0 |

The post gives no level or parts. A search over every body, grip, sight, accessory and element
for the Infinity, levels 30…90 (probe), finds all three numbers of each card at exactly one
level, **80** (for each of the four elements), with the **Vladof grip**, any elemental part, the legendary body, and the
accessory each prefix belongs to (`Tech_3_Firerate` for Vengeful; for Dva any accessory that
leaves these three rows alone, `Laser_Double_DvaInfinity` among them). The sight does not affect
these rows. Test `published_infinity_cards` builds both and asserts the six numbers.

Unrounded, the crate computes 277804.75 / 89.6296 / 10.4000 and 282917.72 / 89.6296 / 8.9600.

What this one comparison covers: the formula up to level 80 (1.13^80 in 32-bit floats, to six
digits); level = grade index; the type's damage scale; slot grades and per-grade values; the
manufacturer-matched grip effects; the separate treatment of negative scale (§4); accuracy as
100 × (1 − spread ÷ 15); fire rate as the inverse of the interval; rounding up for damage (or
to nearest: these two do not tell them apart; §7.3 does) and rounding to nearest for one-decimal rows.

What it is not: a check made on this PC, or of this game version. The numbers are a player's
report from 2014.

Other searches found no usable card for the fixture's guns or the Basic Repeater; the wiki
could not be fetched.

### 7.3 Four cards from two screenshots, by inverse search

Sam supplied two inventory screenshots found on the web (other players' guns; 500 pixels wide,
so digits are about five pixels tall). Each shows two cards. For each card a probe enumerated
every part combination the gun's balance allows, computed the card at the level requirement
shown, and compared every row. The part lists were read from the balance's
`WeaponPartListCollection`, taking the slots it does not enable from its `BaseDefinition`.

| Card as read | Balance, level | Combinations (consistent with the name) | Reproduce every row |
|---|---|---|---|
| **Gearbox Projectile Convergence**: 10 / 93.0 / 10.4 / 3.0 / 25, "Sustained fire increases accuracy" | `SMG_Gearbox_1`, 1 | 75 (75) | **5**: Bandit grip + Maliwan barrel, with each of the 5 stocks |
| **Inflammatory Phobia** (green, level requirement 4): 21 / 93.3 / 2.5 / 2.1 / 6, Burn Dmg/Sec 15.7, Chance to Ignite 15.6% | `Pistol_Maliwan_2_Uncommon`, 4 | 18,432 (576) | **18** of the 576: Dahl grip + Hyperion barrel + fire, no accessory or the bayonet, with each of the 9 sights |
| **Grim TMP** (white, level requirement 2): 15 / 81.8 / 8.0 / 2.4 / 20 | `Pistol_Vladof`, 2 | 30 (5) | **1**: Torgue grip + Dahl barrel |
| **redy Magamum!** (white, level requirement 30): 525 / 84.0 / 4.0 / 3.2 / two digits that cannot be read | `Pistol_Bandit`, 30 | 40 (1: the name fixes the Tediore grip and the Torgue barrel) | That one combination computes 525 / 84.0 / 4.0 / 3.2 / 26 |

Order of the numbers: damage, accuracy, fire rate, reload speed, magazine size. The first three
cards were found with the 32-bit arithmetic of the first version except for the Convergence's
magazine (computed 24, card 25); the fourth card's reload was computed 3.1 (card 3.2). After
the precision change of §4 all of these agree. Test `posted_cards` builds the four guns.

What the search shows:

- **The Gearbox guns do not have fixed parts.** `SMG_Gearbox_1` fixes only the body and the
  material; grip, barrel and stock roll from the common Hyperion SMG lists. The posted
  Convergence differs from Sam's (Bandit grip, Tediore barrel) in its barrel, and the
  computed cards differ in the same two rows as the real ones: accuracy 93.0 against 92.3,
  reload 3.0 against 2.7.
- **Names pin parts.** "redy" is the Bandit prefix for a Tediore grip and "Magamum!" the Bandit
  title for a Torgue barrel; "Grim" the Vladof prefix for a Torgue grip; "Phobia" the Maliwan
  title for a Hyperion barrel and "Inflammatory" its fire prefix. Where the name left parts
  open, the rows picked exactly one grip and barrel, and the remaining freedom is in parts that
  do not touch the rows (sights, stocks).
- **Level requirement = item level** for the three cards that show one (2, 4, 30).
- **Damage rounds up.** The Magamum's damage is 524.34 before rounding and its card says 525.
- **The elemental rows are right in value**: burn damage per second 15.65 → 15.7 and ignite
  chance 15.6, with a "%" after the chance. The card's wording differs from the data's (§5.2).
- **Fixed text matches**: the crate returns "High elemental effect chance." and "Deals bonus
  elemental damage." for the Phobia, and the card shows both. The card's other three lines
  ("+8% Critical Hit Damage", "Consumes 2 ammo per shot", "Highly effective vs Flesh") are of
  the kind the crate does not produce (§5.4).
- **The fourth card's magazine** is the one number not confirmed. The crate computes
  30 ÷ 1.14 = 26.3 → 26. The screenshot shows two digits about as wide as "26"; they were first
  read as "13", then as "31". No part combination of the common Bandit pistol gives either.

Do the cards include the player's own bonuses (Badass Rank, skills)? The script caches the card
when the weapon is initialised (`InitializeInternal` → `CacheWeaponCard`, native), before the
weapon has an owner; whether native code refreshes it later was not determined. The Badass Rank
skill does modify weapon attributes such as the fire interval (`GD_Challenges.BadassSkill`), so
a card that showed live values would move with it. The Convergence and the Phobia are equipped
in the screenshots and match the bare calculation in every row; a 1% fire-rate bonus would have
shown (10.43 → 10.5). So either those players had no such bonus or the card leaves it out. This
does not settle which.

**Verdict.** Evidence for the engine: three cards reproduced in all 17 rows and the fourth in
the four rows that can be read, across an SMG and three makes of pistol at levels 1, 2, 4 and
30, each by a part combination its balance and its name allow. One engine rule changed because
of it (precision, §4). These are still other people's screenshots, at low resolution.

### 7.4 Worked example: Gearbox Rifle, level 1

Vladof assault-rifle type; Gearbox body, Jakobs grip, Bandit barrel, Torgue stock, no sight.

| Row | Base | Modifiers | Value | Card |
|---|---|---|---|---|
| Damage | 8 × 1.13 × 1.08 = 9.7632 | body +10%; damage slot grade 4 (grip) + 2 (barrel) = 6 × 3% = +18% | × 1.28 = 12.497 | 13 |
| Accuracy | spread 2.0 | spread slot grade −3 (barrel) × −5% = +15% | 2.3 → 100 × (1 − 2.3 ÷ 15) = 84.667 | 84.7 |
| Fire Rate | interval 0.115 | body +20%; fire-rate slot grade −3 (grip) × −3% = +9% | 0.14835 → 6.741 | 6.7 |
| Reload Speed | 3.5 | reload slot grade −1 (grip) × −5% = +5% | 3.675 | 3.7 |
| Magazine Size | 20 | the grip's +2 applies only to Jakobs guns | 20 | 20 |

## 8. For Sam to check in game

The three weapons on the throwaway character (`win-throwaway-Save0001.sav`), all level 1. These
are what the crate computes (test `fixture_weapon_cards`). **Not yet compared.**

| Weapon | Damage | Accuracy | Fire Rate | Reload Speed | Magazine Size | Other text |
|---|---|---|---|---|---|---|
| Gearbox Rifle | 13 | 84.7 | 6.7 | 3.7 | 20 | |
| Gearbox Projectile Convergence | 10 | 92.3 | 10.4 | 2.7 | 25 | "Sustained fire increases accuracy." |
| Gearbox Callipeen | 37 | 94.4 | 0.8 | 5.0 | 6 | |

Before rounding: Rifle 12.497 / 84.667 / 6.741 / 3.675 / 20; Convergence 9.313 / 92.333 / 10.435
/ 2.700 / 25; Callipeen 36.773 / 94.400 / 0.769 / 5.000 / 6.

The Convergence row changed after §7.3 (magazine 24 → 25). Another player's Convergence with
the same grip shows 25, by the same computation: 24 × 1.25 ÷ 1.2. The crate still marks that
row approximate, because it sits exactly on the rounding boundary.

What Sam's check adds to the posted cards:

- It is the first comparison on this PC and this game version, with guns whose parts are known
  from the save and not inferred from a card.
- **Damage 13 and 10** (not 12 and 9): rounding up, already seen once in §7.3.
- **Convergence magazine 25**: the precision rule of §4, on a second copy of the gun.
- An assault rifle and a sniper rifle: the posted cards cover only pistols and an SMG.
- Whether the cards change with Badass Rank (compare with the rank's bonuses switched off and
  on in the Badass Rank menu, if the profile has any).

Further checks, if a gun turns up: the labels of a corrosive, shock or slag weapon's extra rows
(§5.2); any shotgun ("x" projectiles after the damage); any shield (§9).

## 9. Shields

Same machinery (`WillowItem.InitializeInternal`), with one difference: a shield's rows are
attributes of the *carrier's* shield resource pool, fed by the shield's external effects.

| Fact | How checked |
|---|---|
| A shield type's `ExternalAttributeEffects` pre-add the capacity (`Init_ShieldItem_CapacityMaxValue`: balance multiplier × 0.8 × 1.13^level, minimum 1), the recharge rate (0.16 × the capacity formula) and the delay (the constant 3.5) | Probe dumps |
| Its slots `Capacity` (+3% per grade), `RechargeRate` (+3%), `RechargeDelay` (−4%) are external scale slots; the type itself activates them at grade 0 and the parts (body, battery, capacitor, material) move them | Probe dumps |
| The item-level attribute reads `ExpLevel` through an inventory context; on the item's own card the script passes the item as the context | Probe; disassembly of `ApplyAllExternalAttributeEffects` (the item is the override context source) |

The crate computes the three rows as "pool starts at 0, plus the shield's external effects and
external slot effects". Example (test `known_item_cards`): a level-5 Quick Shield computes as
capacity 121, recharge rate 23, recharge delay 2.24. **No shield card has been compared with
anything** (§12).

## 10. Grenade mods, class mods and relics

Done in §14. What this section listed as missing (the item contexts, the slot-grade resolver,
the modifier-text rules, conditional initialisation for skill points) is now in the crate.

## 11. Implementation (`crates/gamedata`)

- **Extraction** (`extract/stats.rs`): for every type and part in the asset tables, its effects,
  slot effects and upgrades, `UIStatList`, damage type, fixed text, and for weapon types the
  base values; then every attribute and initialisation definition those use as values, followed
  recursively; then the presentation lists, filtered to attributes some card shows. Anything
  unreadable is recorded on the definition and makes the numbers that depend on it unknown.
- **Database**: `ItemDatabase::stat_data` (a `StatsData`): 4,501 definitions, 346 attributes, 45
  initialisation definitions, 7 presentation lists (160 default entries), the damage types'
  lines. The cache schema is now **4** (3 added the stat data, 4 the lines); the cache file is
  about 2.3 MB. An older cache is rebuilt (the schema is part of the fingerprint).
- **Calculation** (`database/stats.rs`): `ItemDatabase::stats(&ItemRefs, ItemLevel) -> ItemStats`
  and `stats_with(…, DlcResolution)`. A row is `Computed { value, display, supplement,
  approximate }` or `Unknown { reason }`; `lines` holds every line below the rows (`CardLine`:
  kind, text, note, attribute, colour, priority). DLC-set items are unavailable under the
  current `DLC_RESOLUTION`, like names.
- Hard-coded class defaults (ten numbers from `Default__WeaponTypeDefinition`, one from
  `Default__WillowWeapon`, two from `Default__AttributePresentationDefinition`, the slot limit)
  are checked against the install by three tests.

## 12. Not verified

- **Anything in the running game on this PC.** §8 is the first check to make.
- The modifier rule (§4), the slot rule (§2.7), the evaluation order of a value struct (§2.2)
  and the presentation functions (§5.2) are native code. They are inferred from the data and
  agree with the six posted cards of §7.2 and §7.3; they were not read.
- **Float rounding at boundaries.** That the game evaluates in more than 32 bits and rounds on
  storing (§4) is inferred from two cards. Its exact order of operations is not known. A value
  that lands within four units in the last place of a rounding boundary is still marked
  approximate (1.2% of rows in the census; the Convergence's magazine is one). Whether the
  integer properties (magazine, projectiles) truncate before the card floors them is the same
  question.
- The magazine of the fourth card in §7.3 (computed 26; not readable).
- Whether a card includes the player's Badass Rank or skill bonuses (§7.3).
- `ExpLevel` = grade index for **shields** and other items (§2.4), and for a grade index of 0
  (the fixture's grenade mod has one).
- Every shield number (§9), and the assumption that a card shows the pool's value from zero.
- The labels of the elemental rows: the card's wording differs from the data's and its source
  was not found (§5.2). Only the incendiary pair was seen.
- The form of the projectile supplement. The data gives the prefix `x` and the count; whether
  the card prints "13 x7", "13x7" or "13 ×7" was not seen.
- How the card picks a presentation when both lists have one (§5.2), beyond the cases used.
- The clamping of a remapped value outside its input range (a spread above 15).
- DLC items: computed only under the unverified H-set and H-flag hypotheses (windows doc §7).
  DLC parts use the same structures; 23 odd pairings give a non-positive fire interval.
- Parts whose definitions live in level packages (the 102 unlocated base weapon-part entries):
  a weapon with one has every row unknown.
- Effects applied by behaviours at run time (a Fibber barrel's `Behavior_AttributeEffect`, skill
  effects of unique guns, anything that changes while firing) are not on the static card here;
  whether the game's card includes any of them was not checked.
- The line rules of §14.2 beyond the Phobia's five lines (§14.3); every grenade mod, class mod
  and relic number (§14.4); whether a card line about a smaller-is-better attribute has its
  sign flipped; the wording of a line that stands for an attribute ("Burst fire while
  zoomed." is shown with a note); zoom lines.
- Luneshine, Overpower level scaling of the level requirement, and any other game version.

## 13. Sam's hints, checked

| Hint | Verdict |
|---|---|
| final ≈ (base + Σ pre-adds) × (1 + Σ scales) + Σ post-adds | Right for positive scales. Negative scales divide: ÷ (1 + Σ of their sizes) (§4) |
| damage scales by a fixed ratio per level, about 1.13 | Exactly 1.13: `Att_UniversalBalanceScaler`, raised to the level, times 8 and the type's scale (§2.3) |
| `AttributeDefinition` / `AttributeInitializationDefinition` / `AttributeInitializationData` | All three exist and work as §2.2, §2.3, §2.5 describe |
| accuracy and reload are transformed for display | Accuracy is (100 × (1 − spread ÷ 15)). Fire rate is too (1 ÷ interval). Reload is not: the card shows the reload time in seconds |

Sources: the game's own packages on this PC; the Steam discussion linked in §7.2; a web search
that surfaced the Borderlands wiki's accuracy page (first game, spread ÷ 12; not used).

## 14. Lines below the rows, and the other item kinds (phase 3)

### 14.1 Where the lines come from

| Fact | How checked |
|---|---|
| Types and parts carry `WeaponCardAttributes`: entries of `Attribute` and `PriorityIncrease`. 212 entries in `Startup.upk`, over 12 attributes: recoil impulse (40), zoom FOV (34), shot cost (34), spread (28), damage (21), reload (17), critical hit bonus (17), magazine (13), fire interval (12), melee damage (10), projectile speed (2), put-down time (1) | Probe census |
| A part's effect on the carrier is an `ExternalAttributeEffects` entry: the Hyperion pistol barrel adds 0.15 to `PlayerCriticalHitBonus`, a bayonet 0.5 to `InstigatedMeleeDamageModifier`; the part lists that attribute for the card | Probe dumps |
| Slot effects have `bIncludeInFunStats`: a class mod's `Skill1…3` slots and a grenade's `ChildCount` slot set it | Probe dumps |
| `CustomPresentations` entries with no attribute and no number are fixed text (§5.4); a damage type's `WeaponCardPresentations` are more of them ("Highly effective vs Flesh.", orange; "Deals bonus explosive damage.") | Probe dumps |
| An `AttributePresentationDefinition` also holds the wording of a *modifier*: `bDisplayAsPercentage` (default true), `bDontDisplayPlusSign`, `SignStyle`, `bUseCustomNumberPlacement` with `$NUMBER$` in the description, `Prefix`/`Suffix` with `bInclude…InTranslation`, `BasePriority`, `TextColor` | Class properties and defaults (test `presentation_class_defaults`) |
| The carrier's base values: `WillowPlayerController.CurrentInstantHitCriticalHitBonus` is 2.0; every `Controller.Instigated…Modifier` (melee, each element's damage and status, grenade, rocket, …) is 1.0 | Test `player_and_grenade_mod_class_defaults` |
| A grenade mod's own attribute properties (`WillowGrenadeMod.GrenadeDamage`, `BlastRadius`, `FuseTime`, `NumberOfChildProjectiles`, status effect ones) have no class default: they start at 0 and the definitions' `ItemAttributeEffects` pre-add everything (`Init_GrenadeModDamage` for the damage, 550 for a standard radius) | Same test; probe dumps |
| `AttributeSlotEffectAttributeValueResolver` (`SlotName`) is an attribute that reads a slot's grade (`GrenadeModIsIncendiary` and the like) | Probe dumps |
| `ConditionalInitialization`: `Init_BaseSkillPointCalculation` gives a class mod's base skill points by item level: 4 at 50 and above, 3 at 38, 2 at 25, 1 at 12, else 1; a skill slot adds 0.34 per grade and the presentation floors | Probe dump; the formula note of §2.3 says the conditional is used only when the formula is off |

### 14.2 The rule adopted for a bonus line

The card composes a line for each attribute a type or part lists for it (and for each slot
marked for the card), from **the item's whole change of that attribute**, worded by the
attribute's presentation:

1. Change: with a percentage presentation, the relative change `(with − without) ÷ without`,
   where `without` is the attribute's base (the weapon's own property, 0 for a resource pool,
   the carrier's class default). A change made only by scale modifiers needs no base. With a
   plain presentation, the difference; a pure add needs no base either.
2. Number: times 100 for a percentage; remapped if the presentation says so (shot cost: 0…1000
   → 1…1001, so +1 prints as 2); rounded as the presentation says (nearest whole number by
   default); sign by `SignStyle`, the plus sign unless suppressed; "%" for a percentage.
3. Text: `Description` with `$NUMBER$` replaced when the presentation places the number, else
   prefix, number, suffix and description joined with spaces ("Consumes 2 ammo per shot.",
   "+8% Critical Hit Damage"). Markup removed.
4. Order: priority = presentation `BasePriority` + the part's `PriorityIncrease`, highest first;
   among equals, parts (slot order) before the type, then the damage type's lines.

Skill lines are the slot's own modifier (not the item's total), floored: "+2 Immolate Skill".
Modifier-text rows (class mods, relics, a shield's special row) use the same number rule and
are left out when the item does not modify the attribute (a relic lists every stat it could
raise).

Lines the crate does not compose, with the reason it gives: zoom lines (the presentation is
inverted and remapped; the posted zoom factors fit `tan(35°) ÷ tan(zoomed FOV ÷ 2)` with the
default 70° field of view, but that was not confirmed), lines about an attribute with no
presentation in the lists (projectile speed, put-down time), and lines about an attribute whose
base is unknown. Lines about an attribute where smaller is better (recoil, reload, recharge
delay) are composed but carry a note: whether the card flips their sign was not established.

### 14.3 Check: the Phobia's five lines

The posted "Inflammatory Phobia" (§7.3) shows, below its rows: "High Elemental Effect chance",
"+8% Critical Hit Damage", "Consumes 2 ammo per shot", "Deals bonus elemental damage", "Highly
effective vs Flesh". The crate composes exactly these five, in this order (test `posted_cards`):

- the material's fixed text (priority 3);
- +0.15 on the carrier's critical hit bonus, base 2.0: 7.5% → "+8%" (priority 0 + 0.5); the
  base of 2.0 is what makes it 8 and not 15;
- the type's +1 shot cost remapped to 2 (0 + 0.5; the barrel's line comes before the type's);
- the type's fixed text (0.25);
- the damage type's line (0.15).

This is the one outside check of the line rules. Everything in §14.2 beyond what it exercises
(whole-item change against a part's own change, the sign of smaller-is-better attributes,
skill-point rounding, the order of a definition's own lines among themselves) is assumed.

### 14.4 The other item kinds

Grenade mods, class mods and relics go through the same machinery as shields (§9): the item's
contexts (`InventoryAttributeContextResolver`, `EquippedInventoryAttributeContextResolver`),
its external effects and slot effects, the kind's presentation list. The owner's items
(`mac-main-Save0001.sav`, test `maya_fixture_cards`), **computed, not compared**:

| Item | Rows | Lines |
|---|---|---|
| Unyielding Banshee Class Mod, level 12 | Shield Recharge Rate +26%, Shield Recharge Delay −19.35% (noted), Max Health −75 | +2 Immolate Skill |
| Relentless Matriarch Class Mod, 12 | Max Health +69, Magazine Size +16.8% | +2 Sustenance Skill |
| Bullet Witch Class Mod, 12 | Corrode Chance +18.4%, Corrode Damage +21% | +2 Kinetic Reflection Skill |
| Warder Class Mod, 8 (white) | Shield Capacity +57 | |
| Lobbed MIRV, 12 | Grenade Damage 469 x8, Blast Radius 412, Fuse Time 0.6 | Spawns 8 child grenades upon detonating. / Breaks into a number of smaller child grenades that explode on impact. |
| Explosive Fastball, 8 | Grenade Damage 681, Blast Radius 85, Fuse Time 0.6 | Forget the curveball Ricky, give him the heater. (red) |
| Snapping Turtle Shield, 8 | Capacity 272, Recharge Rate 31, Recharge Delay 2.30, Max Health −93 | High capacity, reduces max health. |
| Contraband Sky Rocket (throwaway save), grade 0 | Blast Radius 550, Fuse Time 1.6 (its type lists no damage row) | WARNING: Not intended for indoor use. (red) / Damage is based on player level. |
| Vault Hunter's Relic, 1 | Rare Loot Chance: unknown (its attribute has no value resolver) | Increased chance of enemies dropping rare loot. |

Census (test `item_kinds_and_lines_census`, every base type of the kind with every base item
part): grenade mods 12 types, all rows compute; class mods 44 types, all rows compute; relics
30 types, 20 unknown rows of 1,125 (attributes with no resolver, or read through the pawn);
weapons 1,221 lines without text of 39,145, 1,122 of them zoom lines.

//! The definitions an item card's numbers are computed from, as extracted from the game's
//! packages: each type's and part's attribute effects, the attribute and initialisation
//! definitions those refer to, and the presentation rules the card applies.
//!
//! These mirror the game's own structures (research doc `2026-11-09-item-stats.md`):
//! `AttributeInitializationData` is [`ValueData`], `AttributeEffectData` is
//! [`AttributeEffect`], `AttributeSlotEffectData` is [`SlotEffect`], `AttributeSlotUpgradeData`
//! is [`SlotUpgrade`], `UIStatData` is [`UiStat`], `AttributeInitializationDefinition` is
//! [`Initialization`], `AttributeDefinition` is [`AttributeSource`] and
//! `AttributePresentationDefinition` is [`Presentation`]. Object references are kept as object
//! paths.

use std::collections::HashMap;

use super::Rgba;

/// An `AttributeInitializationData` struct: a number given as a constant, an attribute's
/// value, or an initialisation definition's result, times a scale.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueData {
    /// `BaseValueConstant`.
    pub constant: f32,
    /// `BaseValueAttribute`: when set, its value is used in place of the constant.
    pub attribute: Option<String>,
    /// `InitializationDefinition`.
    pub initialization: Option<String>,
    /// `BaseValueScaleConstant`.
    pub scale: f32,
}

impl ValueData {
    /// A plain constant with scale 1.
    pub fn constant(value: f32) -> Self {
        Self {
            constant: value,
            attribute: None,
            initialization: None,
            scale: 1.0,
        }
    }
}

impl Default for ValueData {
    /// The struct's own defaults: constant 0, no references, scale 1.
    fn default() -> Self {
        Self::constant(0.0)
    }
}

/// `EModifierType`: how a modifier combines with an attribute's base value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierKind {
    Scale,
    PreAdd,
    PostAdd,
}

/// One entry of `WeaponAttributeEffects`, `ItemAttributeEffects` or `ExternalAttributeEffects`.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeEffect {
    /// `AttributeToModify`.
    pub attribute: String,
    pub kind: ModifierKind,
    /// `BaseModifierValue`.
    pub value: ValueData,
}

/// One entry of a type's `AttributeSlotEffects`: a modifier whose size grows with the slot's
/// grade, which the parts raise or lower through [`SlotUpgrade`]s.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotEffect {
    pub slot: String,
    /// `bExternalSlot`: the modifier applies to the wearer, not to the item itself.
    pub external: bool,
    pub attribute: String,
    pub kind: ModifierKind,
    /// `BaseModifierValue`.
    pub base: ValueData,
    /// `PerGradeUpgrade`.
    pub per_grade: ValueData,
    /// `MinimumGrade` when `bEnforceMinimumGrade` is set.
    pub minimum_grade: Option<i32>,
    /// `MaximumGrade` when `bEnforceMaximumGrade` is set.
    pub maximum_grade: Option<i32>,
    /// `bIncludeInFunStats`: the card shows this slot's modifier as a line of its own (a class
    /// mod's skill bonuses, a grenade's child count).
    pub fun_stats: bool,
}

/// One entry of a type's or part's `AttributeSlotUpgrades`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotUpgrade {
    pub slot: String,
    pub grade_increase: i32,
    /// `bActivateSlot`.
    pub activates: bool,
}

/// `EComparisonOperator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Comparison {
    EqualTo,
    NotEqualTo,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

/// An `AttributeExpressionData`: `attribute <comparison> (other attribute, or constant)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    /// `AttributeOperand1`.
    pub attribute: String,
    pub comparison: Comparison,
    /// `AttributeOperand2`; when set it is compared against in place of the constant
    /// (`OPERAND_PreferAttribute`).
    pub other_attribute: Option<String>,
    /// `ConstantOperand2`.
    pub constant: f32,
}

/// One entry of a type's or part's `WeaponCardAttributes`: an attribute the card gets a bonus
/// line for ("+8% Critical Hit Damage"), with how much the line's priority is raised.
#[derive(Debug, Clone, PartialEq)]
pub struct CardAttribute {
    pub attribute: String,
    pub priority_increase: f32,
}

/// One entry of a definition's `CustomPresentations` (or a damage type's
/// `WeaponCardPresentations`): a presentation that is a card line of its own, naming an
/// attribute or not.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomPresentation {
    pub attribute: Option<String>,
    pub presentation: Presentation,
}

/// One entry of a type's `UIStatList`: a row of the card's top section.
#[derive(Debug, Clone, PartialEq)]
pub struct UiStat {
    /// `Attribute`; `None` when the entry names none.
    pub attribute: Option<String>,
    /// `SupplementalAttributeToAppend`, shown after the value (the "x7" of a shotgun) when
    /// `condition` holds.
    pub supplemental: Option<String>,
    /// `SupplementalAttributeExpression`, when it names an attribute.
    pub condition: Option<Condition>,
    /// The entry shows the attribute's value as a plain number (`ATTRSTYLE_JustNumber`, not
    /// `bDisplayAsModifierOnly`). Other styles are not rendered by this database.
    pub just_number: bool,
}

/// A fixed line of text on the card (red flavour text and the like), from a
/// `CustomPresentations` entry that shows no number.
#[derive(Debug, Clone, PartialEq)]
pub struct CardText {
    /// The text with the game's `[tag]…[-tag]` markup removed.
    pub text: String,
    /// `TextColor`, when the entry sets one.
    pub color: Option<Rgba>,
    /// `BasePriority`: the card orders its lines by it, highest first.
    pub priority: f32,
}

/// What a `WeaponTypeDefinition` gives a weapon before any modifier, by the name of the weapon
/// property it initialises (`WillowWeapon.CalculateWeaponBaseValues`).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponBase {
    /// `ClipSize`.
    pub clip_size: i32,
    /// `FireRate`: seconds between shots (the weapon's `FireInterval`).
    pub fire_interval: f32,
    /// `ReloadTime`, in seconds.
    pub reload_time: f32,
    /// `Spread`.
    pub spread: f32,
    /// `ProjectilesPerShot`.
    pub projectiles_per_shot: i32,
    /// `ShotCost`: ammunition used per shot.
    pub shot_cost: i32,
    /// `PerShotAccuracyImpulse`.
    pub per_shot_accuracy_impulse: f32,
    /// `AutomaticBurstCount`.
    pub automatic_burst_count: i32,
    /// `InstantHitDamage`.
    pub damage: ValueData,
    /// `StatusEffectDamage`.
    pub status_effect_damage: ValueData,
    /// `BaseStatusEffectChanceModifier`.
    pub base_status_effect_chance: ValueData,
    /// `DefaultDamageTypeDefinition`.
    pub damage_type: Option<String>,
}

/// What the database keeps of a type or part for computing item cards.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatDefinition {
    /// Weapon types only.
    pub weapon: Option<WeaponBase>,
    /// `WeaponAttributeEffects` or `ItemAttributeEffects`: modifiers on the item itself.
    pub effects: Vec<AttributeEffect>,
    /// `ExternalAttributeEffects`: modifiers on whoever carries the item.
    pub external_effects: Vec<AttributeEffect>,
    /// `AttributeSlotEffects` (types only).
    pub slot_effects: Vec<SlotEffect>,
    /// `AttributeSlotUpgrades`.
    pub slot_upgrades: Vec<SlotUpgrade>,
    /// `UIStatList` (types only).
    pub ui_stats: Vec<UiStat>,
    /// A part's `CustomDamageTypeDefinition`.
    pub damage_type: Option<String>,
    /// Fixed text lines from `CustomPresentations`.
    pub texts: Vec<CardText>,
    /// `CustomPresentations` entries that are not fixed text (they show a number or need a
    /// localisation lookup), which [`StatDefinition::texts`] leaves out.
    pub texts_omitted: u32,
    /// Every `CustomPresentations` entry, for the card's lines.
    pub presentations: Vec<CustomPresentation>,
    /// `WeaponCardAttributes`.
    pub card_attributes: Vec<CardAttribute>,
    /// Anything in the definition that could not be read as expected. A definition with
    /// problems makes every number that depends on it unknown.
    pub problems: Vec<String>,
}

/// How an attribute definition gets its value (its `ValueResolverChain`).
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeSource {
    /// A single `ConstantAttributeValueResolver`.
    Constant(f32),
    /// A single `ObjectPropertyAttributeValueResolver`: the named property of the object the
    /// context resolver finds. `context` is the class of the first context resolver (empty when
    /// the chain is empty), `resource` the resource a resource-pool context names.
    Property {
        context: String,
        resource: Option<String>,
        property: String,
    },
    /// A single `ManufacturerAttributeValueResolver`: the value of the first case whose name is
    /// the item's manufacturer, otherwise `otherwise`.
    Manufacturer {
        cases: Vec<(String, ValueData)>,
        otherwise: ValueData,
    },
    /// A single `SimpleMathValueResolver` that multiplies two values.
    Product(ValueData, ValueData),
    /// A single `AttributeSlotEffectAttributeValueResolver`: the grade of the item's named
    /// attribute slot.
    SlotGrade(String),
    /// Anything else, with what was found.
    Unsupported(String),
}

/// `EBaseValueMode`: what an initialisation definition does with the value it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BaseValueMode {
    Sets,
    Adds,
    Scales,
    OffsetBy,
}

/// `EAttributeInitializationRounding`, also used by presentations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rounding {
    Float,
    IntRound,
    IntFloor,
    IntCeil,
}

/// A `BalanceFormula`: `Multiplier * (Level ^ Power + Offset)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    pub multiplier: ValueData,
    pub level: ValueData,
    pub power: ValueData,
    pub offset: ValueData,
}

/// One entry of a `ConditionalInitialization` list: a value that applies when all of its
/// conditions hold.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionalCase {
    pub conditions: Vec<Condition>,
    /// `BaseValueIfTrue`.
    pub value: ValueData,
}

/// `ConditionalInitialization`: the first case whose conditions all hold wins, else the
/// default. Used only when the formula is disabled (the game's own note).
#[derive(Debug, Clone, PartialEq)]
pub struct Conditional {
    pub cases: Vec<ConditionalCase>,
    /// `DefaultBaseValue`.
    pub default: ValueData,
}

/// An `AttributeInitializationDefinition`.
#[derive(Debug, Clone, PartialEq)]
pub struct Initialization {
    pub mode: BaseValueMode,
    pub rounding: Rounding,
    /// `ValueFormula`, when enabled.
    pub formula: Option<Formula>,
    /// `ConditionalInitialization`, when enabled and the formula is not.
    pub conditional: Option<Conditional>,
    /// `RangeRestriction.MinValue`, when its restriction is enabled.
    pub minimum: Option<ValueData>,
    /// `RangeRestriction.MaxValue`, when its restriction is enabled.
    pub maximum: Option<ValueData>,
    /// Set when the definition uses something this database does not evaluate (random
    /// variance, or neither a formula nor a conditional).
    pub unsupported: Option<String>,
}

/// `AttributePresentationValueRemappingData`: a linear map from an input range to an output
/// range.
#[derive(Debug, Clone, PartialEq)]
pub struct Remap {
    pub input_min: ValueData,
    pub input_max: ValueData,
    pub output_min: ValueData,
    pub output_max: ValueData,
}

/// `EAttributeValueSignStyle`: how a modifier's sign is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignStyle {
    AsIs,
    Positive,
    Negative,
    AsIsAnyStyle,
    OppositeSign,
}

/// An `AttributePresentationDefinition`: how the card shows an attribute's value, and how it
/// words a bonus line for a modifier of the attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    /// `Description`: the row's label, or a line's wording (`$NUMBER$` marks where the number
    /// goes when `custom_placement` is set).
    pub label: Option<String>,
    /// `NoConstraintText`: the wording of a line that shows no number.
    pub no_constraint_text: Option<String>,
    /// `BasePriority`: lines are ordered by it, highest first.
    pub base_priority: f32,
    /// `bDisplayAsPercentage`: a modifier is shown as a percentage.
    pub percentage: bool,
    /// `bDisplayPercentAsFloat`.
    pub percent_as_float: bool,
    /// The opposite of `bDontDisplayPlusSign`.
    pub plus_sign: bool,
    /// The opposite of `bDontDisplayNumber`.
    pub display_number: bool,
    /// `bUseCustomNumberPlacement`.
    pub custom_placement: bool,
    /// `bBiggerIsBetter`.
    pub bigger_is_better: bool,
    pub sign: SignStyle,
    /// `TextColor` when `bEnableTextColor`, as the card colours the line.
    pub color: Option<Rgba>,
    /// `bIncludePrefixInTranslation` and `bIncludeSuffixInTranslation`.
    pub include_prefix: bool,
    pub include_suffix: bool,
    /// `bDisplayAsInverse`: show 1 / value (fire interval as fire rate).
    pub inverse: bool,
    /// `RemappingData`, when `bValueRemappingEnabled` (spread as accuracy).
    pub remap: Option<Remap>,
    pub rounding: Rounding,
    /// `FloatPrecision`: digits after the decimal point for [`Rounding::Float`].
    pub precision: i32,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

/// A presentation list entry.
#[derive(Debug, Clone, PartialEq)]
pub enum PresentationRule {
    Plain(Box<Presentation>),
    /// A `WeaponStatusEffectAttributePresentationDefinition`: the presentation depends on the
    /// weapon's damage type, and a damage type with no entry shows no row.
    ByDamageType(Vec<(String, Presentation)>),
}

/// The presentation lists of `GD_Globals.General.Globals`: the default one and the overrides
/// per kind of item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PresentationList {
    Default,
    Weapon,
    Item,
    Shield,
    GrenadeMod,
    ClassMod,
    Artifact,
}

impl PresentationList {
    pub const ALL: [PresentationList; 7] = [
        Self::Default,
        Self::Weapon,
        Self::Item,
        Self::Shield,
        Self::GrenadeMod,
        Self::ClassMod,
        Self::Artifact,
    ];

    /// The property of `GD_Globals.General.Globals` that names the list.
    pub const fn globals_property(self) -> &'static str {
        match self {
            Self::Default => "DefaultAttributePresentation",
            Self::Weapon => "WeaponAttributePresentationOverride",
            Self::Item => "ItemAttributePresentationOverride",
            Self::Shield => "ShieldAttributePresentationOverride",
            Self::GrenadeMod => "GrenadeModAttributePresentationOverride",
            Self::ClassMod => "ClassModAttributePresentationOverride",
            Self::Artifact => "ArtifactAttributePresentationOverride",
        }
    }
}

/// One presentation list's entries for the attributes that some `UIStatList` shows, in list
/// order: the attribute's path and its rule.
pub type PresentationEntries = Vec<(String, PresentationRule)>;

/// Everything extracted for computing item cards.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatsData {
    /// By object path, for the types and parts in the asset tables.
    pub definitions: HashMap<String, StatDefinition>,
    /// By object path, for every attribute the definitions, formulas and presentations refer to
    /// as a value.
    pub attributes: HashMap<String, AttributeSource>,
    /// By object path.
    pub initializations: HashMap<String, Initialization>,
    /// In [`PresentationList::ALL`] order; a list the game data does not have is absent.
    pub presentations: Vec<(PresentationList, PresentationEntries)>,
    /// By object path: a damage type's `WeaponCardPresentations`, the lines a weapon of that
    /// damage type carries ("Highly effective vs Flesh.").
    pub damage_types: HashMap<String, Vec<CustomPresentation>>,
}

impl StatsData {
    /// The rule a card of `kind` uses for `attribute`: the kind's override list first, then the
    /// default list; in each, the first entry for the attribute.
    pub fn presentation(
        &self,
        kind: PresentationList,
        attribute: &str,
    ) -> Option<&PresentationRule> {
        [kind, PresentationList::Default].iter().find_map(|list| {
            self.presentations
                .iter()
                .find(|(l, _)| l == list)
                .and_then(|(_, entries)| entries.iter().find(|(a, _)| a == attribute))
                .map(|(_, rule)| rule)
        })
    }
}

//! Computes the numbers on an item's card (damage, accuracy, fire rate, …) from its type,
//! parts, manufacturer and level, the way the game derives them from its definitions.
//!
//! **Status: computed, unverified.** The rules below were established from the game's data and
//! script code (research doc `docs/research/2026-11-09-item-stats.md`). No result has been
//! compared with a card in the running game by the project's owner yet; the outside checks are
//! cards other players posted, which the calculation reproduces (research doc §7.2, §7.3).
//! Parts of the calculation run in the game's native code, which was not read; they are marked
//! *assumed* here and in the research doc.
//!
//! The calculation, for a weapon:
//! 1. Base values come from the weapon type (`WillowWeapon.CalculateWeaponBaseValues`): damage
//!    from an initialisation formula of the level, the rest plain numbers.
//! 2. Modifiers are collected from the type's and every part's attribute effects, then from the
//!    type's slot effects at the grade the parts give each slot.
//! 3. An attribute's value is `(base + Σ pre-add) × (1 + Σ positive scale) ÷
//!    (1 + Σ |negative scale|) + Σ post-add`. This runs in native code that was not read; the
//!    rule reproduces two player-posted level-80 cards to all six digits (research doc §7).
//! 4. Each `UIStatList` row is shown through the attribute's presentation: remapping (spread to
//!    accuracy), inversion (fire interval to fire rate), rounding, and a suffix ("%").
//!
//! A shield goes the same way (`WillowItem.InitializeInternal`), except that its rows are
//! attributes of the carrier's shield pool: they start from 0 and take the shield's external
//! effects and external slot effects.
//!
//! Anything that cannot be evaluated makes the rows that depend on it unknown, with the reason.
//! Nothing is guessed.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use super::stat_data::{
    AttributeSource, BaseValueMode, CardText, Comparison, Condition, CustomPresentation,
    Initialization, ModifierKind, Presentation, PresentationList, PresentationRule, Rounding,
    SignStyle, SlotEffect, StatDefinition, StatsData, ValueData,
};
use super::{AssetCategory, DlcResolution, ItemDatabase, ItemRefs, ObjectInfo, Rgba, PART_SLOTS};

/// How deep attribute and formula references may nest before evaluation gives up.
const MAX_DEPTH: usize = 24;
/// Relative distance from a rounding boundary within which a displayed value is reported as
/// approximate: the game's own 32-bit float arithmetic may land on the other side. Four units
/// in the last place of a 32-bit float, which covers a few operations done in another order.
const BOUNDARY_TOLERANCE: f64 = 4.0 * f32::EPSILON as f64;

/// The class default of `WillowWeapon.StatusEffectChanceModifier`, which no script sets from
/// the weapon type. Checked against the game by the test `weapon_class_defaults`.
pub(crate) const WEAPON_STATUS_EFFECT_CHANCE_MODIFIER: f32 = 1.0;

/// The context resolver classes that mean "the weapon itself".
const WEAPON_CONTEXTS: &[&str] = &["WeaponAttributeContextResolver"];
/// The context resolver classes that find an item in the carrier's inventory. On an item's own
/// card the item is the context (the script passes it as the override context source).
const ITEM_CONTEXTS: &[&str] = &[
    "InventoryAttributeContextResolver",
    "EquippedInventoryAttributeContextResolver",
];
/// Context resolver classes that mean the carrier (the player or their controller). A bonus
/// line about such an attribute needs the carrier's base value, which [`player_base`] knows
/// for the properties items modify.
const PLAYER_CONTEXTS: &[&str] = &[
    "PlayerControllerAttributeContextResolver",
    "ControllerAttributeContextResolver",
];
/// The carrier's base value of a property that items modify and cards show as bonus lines, from
/// the class defaults: `WillowPlayerController.CurrentInstantHitCriticalHitBonus` and
/// `CurrentMeleeCriticalHitBonus` are 2.0, and every `Controller.Instigated…Modifier` (melee,
/// each element's damage and status damage and chance, grenade, rocket, …) is 1.0. Anything
/// else is not known. Checked against the game by the test
/// `player_and_grenade_mod_class_defaults`.
pub(crate) fn player_base(property: &str) -> Option<f32> {
    match property {
        "CurrentInstantHitCriticalHitBonus" | "CurrentMeleeCriticalHitBonus" => Some(2.0),
        p if p.starts_with("Instigated") && p.ends_with("Modifier") => Some(1.0),
        _ => None,
    }
}
/// A grenade mod's own attribute properties (`WillowGrenadeMod`): all of them start at 0 and
/// are built up by the definitions' effects. Checked by the test `grenade_mod_class_defaults`.
pub(crate) const GRENADE_MOD_PROPERTIES: &[&str] = &[
    "GrenadeDamage",
    "BlastRadius",
    "FuseTime",
    "NumberOfChildProjectiles",
    "StatusEffectDamage",
    "BaseStatusEffectChanceModifier",
    "StatusEffectChanceModifier",
];
/// The context resolver class of resource-pool attributes (shield capacity and the like).
const POOL_CONTEXT: &str = "ResourcePoolAttributeContextResolver";
/// The resource whose pool a shield's card shows.
const SHIELD_RESOURCE: &str = "D_Resources.Shield";
/// The class default of `WillowInventoryDefinition.AttributeSlotMaxActivated`. No gear type
/// has that many slot effects, so the limit never binds; more active slots than this are
/// reported instead of guessed at.
const SLOT_MAX_ACTIVATED: usize = 19;

/// An item's level fields as a save stores them (`ItemFields::grade_index` and
/// `ItemFields::game_stage` in `savekit`).
///
/// The game derives the level its formulas use (`ExpLevel`) from the manufacturer grade index
/// and the balance definition, in native code. This database takes the grade index itself as
/// that level (*assumed*; every base-game weapon balance's grade adds 0 to it). The game stage
/// is carried for completeness and is not used by any formula evaluated so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ItemLevel {
    pub grade_index: Option<u32>,
    pub game_stage: Option<u32>,
}

impl ItemLevel {
    /// Both fields set to `level`, as the game writes them for ordinary items.
    pub const fn new(level: u32) -> Self {
        Self {
            grade_index: Some(level),
            game_stage: Some(level),
        }
    }
}

/// Which card row a [`StatRow`] is, for callers that want to treat known rows specially.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatKind {
    Damage,
    Accuracy,
    FireRate,
    ReloadSpeed,
    MagazineSize,
    /// Elemental damage per second (burn, corrode, electrocute).
    ElementalDamage,
    /// Chance to apply the elemental effect, in percent.
    ElementalChance,
    ShieldCapacity,
    ShieldRechargeRate,
    ShieldRechargeDelay,
    Other,
}

/// The value of one card row.
#[derive(Debug, Clone, PartialEq)]
pub enum StatValue {
    Computed {
        /// The number after the card's presentation rules (remapping, inversion, rounding).
        value: f64,
        /// The number as the card prints it, e.g. `"13"`, `"84.7"` or `"15.6%"`.
        display: String,
        /// Text the card appends after the number, e.g. `"x7"` for a shotgun's pellets.
        supplement: Option<String>,
        /// `None` for a full calculation. Otherwise why the result may differ from the card:
        /// it rests on a rule that is not established, or lies on a rounding boundary.
        approximate: Option<String>,
    },
    /// The row exists on the card but its value could not be computed.
    Unknown { reason: String },
}

/// One row of an item card's top section.
#[derive(Debug, Clone, PartialEq)]
pub struct StatRow {
    /// The object path of the attribute the row shows.
    pub attribute: String,
    /// The row's label from the game's presentation data, e.g. `"Damage"`.
    pub label: Option<String>,
    pub value: StatValue,
}

impl StatRow {
    pub fn kind(&self) -> StatKind {
        match self.attribute.as_str() {
            "D_Attributes.Weapon.WeaponDamage" => StatKind::Damage,
            "D_Attributes.Weapon.WeaponSpread" => StatKind::Accuracy,
            "D_Attributes.Weapon.WeaponFireInterval" => StatKind::FireRate,
            "D_Attributes.Weapon.WeaponReloadSpeed" => StatKind::ReloadSpeed,
            "D_Attributes.Weapon.WeaponClipSize" => StatKind::MagazineSize,
            "D_Attributes.Weapon.WeaponStatusEffectDamage" => StatKind::ElementalDamage,
            "D_Attributes.Weapon.WeaponCombinedStatusEffectChanceModifier" => {
                StatKind::ElementalChance
            }
            "D_Attributes.ShieldResourcePool.ShieldMaxValue" => StatKind::ShieldCapacity,
            "D_Attributes.ShieldResourcePool.ShieldOnIdleRegenerationRate" => {
                StatKind::ShieldRechargeRate
            }
            "D_Attributes.ShieldResourcePool.ShieldOnIdleRegenerationDelay" => {
                StatKind::ShieldRechargeDelay
            }
            _ => StatKind::Other,
        }
    }

    /// The row's text as the card prints it (number plus any supplement); `None` when unknown.
    pub fn text(&self) -> Option<String> {
        match &self.value {
            StatValue::Computed {
                display,
                supplement: Some(supplement),
                ..
            } => Some(format!("{display} {supplement}")),
            StatValue::Computed { display, .. } => Some(display.clone()),
            StatValue::Unknown { .. } => None,
        }
    }

    /// Whether the row is a full calculation: known, and not marked approximate.
    pub fn is_exact(&self) -> bool {
        matches!(
            self.value,
            StatValue::Computed {
                approximate: None,
                ..
            }
        )
    }
}

/// What kind of line a [`CardLine`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardLineKind {
    /// Fixed text (red flavour text, "Sustained fire increases accuracy.").
    Text,
    /// A line the card composes from a number ("+8% Critical Hit Damage", "Consumes 2 ammo per
    /// shot.", "+4 Immolate Skill").
    Bonus,
}

/// One line of the card below its rows.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLine {
    pub kind: CardLineKind,
    /// The line as the card prints it (markup removed); `None` when it could not be composed.
    pub text: Option<String>,
    /// `None` for a full result. Otherwise why there is no text, or, when there is, why it may
    /// differ from the card (a rule that is not established, a rounding boundary).
    pub note: Option<String>,
    /// The attribute a bonus line is about.
    pub attribute: Option<String>,
    /// The line's colour when the game sets one (red text, elemental lines); `None` for the
    /// card's plain colour.
    pub color: Option<Rgba>,
    /// The priority the card orders lines by, highest first.
    pub priority: f32,
}

/// The computed card of an item. **Computed, unverified:** see the module documentation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemStats {
    /// Why no rows could be produced at all (the item does not resolve, or its kind is not
    /// computed). `None` when `rows` is the card's list of rows.
    pub unavailable: Option<String>,
    /// The card's top-section rows, in the game's order.
    pub rows: Vec<StatRow>,
    /// Fixed text lines of the card (red flavour text and the like), highest priority first.
    /// The same lines appear in `lines` with [`CardLineKind::Text`]; this list is kept for
    /// callers of the first version.
    pub texts: Vec<CardText>,
    /// Every line below the rows, fixed and composed, in the card's order (highest priority
    /// first). A line that could not be composed is present with `text` `None` and a reason.
    pub lines: Vec<CardLine>,
}

impl ItemStats {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            unavailable: Some(reason.into()),
            ..Self::default()
        }
    }

    /// The first row of `kind`.
    pub fn row(&self, kind: StatKind) -> Option<&StatRow> {
        self.rows.iter().find(|row| row.kind() == kind)
    }
}

/// A number, and whether it is still exactly what the data states: no arithmetic that rounds
/// has touched it.
///
/// The game keeps its values in 32-bit floats but evaluates each expression at higher
/// precision and rounds only when it stores the result (it is a 32-bit x87 program). Doing the
/// same here, 64-bit arithmetic rounded to 32 bits per expression, reproduces two posted cards
/// that pure 32-bit arithmetic gets wrong by one in the last shown digit (research doc §7.3).
/// The game's exact order of operations is still not known in every detail, so a value that
/// has been through arithmetic can differ from the game's in its last bits; that matters only
/// when it then lies on a rounding boundary of the card.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Num {
    v: f32,
    exact: bool,
}

impl Num {
    const fn exact(v: f32) -> Self {
        Self { v, exact: true }
    }

    /// One expression of two stored values: evaluated in 64 bits, stored in 32.
    fn map2(self, other: Num, f: impl Fn(f64, f64) -> f64) -> Num {
        Num {
            v: f(f64::from(self.v), f64::from(other.v)) as f32,
            exact: false,
        }
    }

    fn map(self, f: impl Fn(f64) -> f64) -> Num {
        Num {
            v: f(f64::from(self.v)) as f32,
            exact: false,
        }
    }
}

type Val = Result<Num, String>;

/// The modifiers collected for one attribute.
#[derive(Debug, Clone, Default)]
struct Stack {
    pre_add: f64,
    positive_scale: f64,
    negative_scale: f64,
    post_add: f64,
    /// A modifier's value could not be evaluated.
    error: Option<String>,
}

impl Stack {
    fn add(&mut self, kind: ModifierKind, value: Val) {
        let n = match value {
            Ok(n) => n,
            Err(e) => {
                self.error.get_or_insert(e);
                return;
            }
        };
        let value = f64::from(n.v);
        match kind {
            ModifierKind::PreAdd => self.pre_add += value,
            ModifierKind::PostAdd => self.post_add += value,
            ModifierKind::Scale if value >= 0.0 => self.positive_scale += value,
            ModifierKind::Scale => self.negative_scale -= value,
        }
    }

    /// `(base + pre-add) × (1 + positive scale) ÷ (1 + |negative scale|) + post-add`.
    /// Positive and negative scale modifiers are summed apart and never netted.
    fn apply(&self, base: Num) -> Val {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let (pos, neg) = (self.positive_scale, self.negative_scale);
        let whole = |x: f64| x.fract() == 0.0;
        let value = (f64::from(base.v) + self.pre_add) * (1.0 + pos) / (1.0 + neg) + self.post_add;
        Ok(Num {
            v: value as f32,
            exact: base.exact
                && pos == 0.0
                && neg == 0.0
                && whole(f64::from(base.v))
                && whole(self.pre_add)
                && whole(self.post_add),
        })
    }
}

/// A base value of the item itself, by the name of the property an attribute reads.
enum Base<'a> {
    Number(f32),
    Data(&'a ValueData),
    Unknown(String),
}

/// One item being evaluated.
struct Card<'a> {
    data: &'a StatsData,
    list: PresentationList,
    /// The context resolver classes that refer to the item itself.
    own_contexts: &'static [&'static str],
    /// The resource whose pool attributes the card shows as the item's own contribution: they
    /// start from 0 and take the item's external modifiers. `None` for weapons.
    pool: Option<&'static str>,
    /// The manufacturer definition's object name, e.g. `Jakobs`.
    manufacturer: Option<&'a str>,
    properties: HashMap<&'static str, Base<'a>>,
    stacks: HashMap<&'a str, Stack>,
    /// Each attribute slot's grade and whether some upgrade activates it.
    grades: HashMap<&'a str, (i32, bool)>,
    /// The slot effects whose modifier the card shows as a line of its own, with the modifier.
    fun_slots: Vec<(&'a SlotEffect, Val)>,
    /// Set when a definition of the item could not be read: every value is then unknown.
    poison: Option<String>,
    depth: Cell<usize>,
}

/// A bonus line's number, before presentation.
struct Modifier {
    /// The item's change of the attribute: a fraction of the base when the presentation shows a
    /// percentage, else the plain difference.
    value: f32,
    exact: bool,
}

/// The last component of an object path.
fn short(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

impl<'a> Card<'a> {
    fn guarded<T>(&self, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if self.depth.get() >= MAX_DEPTH {
            return Err("the definitions refer to each other too deeply".into());
        }
        self.depth.set(self.depth.get() + 1);
        let out = f();
        self.depth.set(self.depth.get() - 1);
        out
    }

    /// Evaluates an `AttributeInitializationData`: the attribute's value in place of the
    /// constant when one is named, passed through the initialisation definition when one is
    /// named, times the scale.
    fn value(&self, data: &ValueData) -> Val {
        self.guarded(|| {
            let base = || match &data.attribute {
                Some(attribute) => self.attribute(attribute),
                None => Ok(Num::exact(data.constant)),
            };
            let value = match &data.initialization {
                None => base()?,
                Some(path) => {
                    let definition = self.data.initializations.get(path).ok_or_else(|| {
                        format!(
                            "initialisation definition {} is not in the database",
                            short(path)
                        )
                    })?;
                    self.initialize(definition, path, base)?
                }
            };
            Ok(if data.scale == 1.0 {
                value
            } else {
                value.map(|v| v * f64::from(data.scale))
            })
        })
    }

    /// `Multiplier × (Level ^ Power + Offset)`, combined with the base value by the
    /// definition's mode, then range-restricted and rounded.
    fn initialize(&self, definition: &Initialization, path: &str, base: impl Fn() -> Val) -> Val {
        if let Some(what) = &definition.unsupported {
            return Err(format!(
                "{} uses {what}, which is not modelled",
                short(path)
            ));
        }
        let mut value = if let Some(formula) = &definition.formula {
            let multiplier = self.value(&formula.multiplier)?;
            let level = self.value(&formula.level)?;
            let power = self.value(&formula.power)?;
            let offset = self.value(&formula.offset)?;
            // One expression: evaluated in 64 bits, stored in 32.
            let widen = |n: Num| f64::from(n.v);
            Num {
                v: (widen(multiplier) * (widen(level).powf(widen(power)) + widen(offset))) as f32,
                exact: false,
            }
        } else if let Some(conditional) = &definition.conditional {
            // The first case whose conditions all hold wins, else the default.
            let mut chosen = &conditional.default;
            for case in &conditional.cases {
                let mut all = true;
                for condition in &case.conditions {
                    all &= self.holds(condition)?;
                }
                if all {
                    chosen = &case.value;
                    break;
                }
            }
            self.value(chosen)?
        } else {
            return Err(format!("{} has no formula", short(path)));
        };
        value = match definition.mode {
            BaseValueMode::Sets => value,
            BaseValueMode::Adds => base()?.map2(value, |b, v| b + v),
            BaseValueMode::Scales => base()?.map2(value, |b, v| b * v),
            BaseValueMode::OffsetBy => {
                return Err(format!(
                    "{} offsets its base value, which is not modelled",
                    short(path)
                ));
            }
        };
        if let Some(minimum) = &definition.minimum {
            value = value.map2(self.value(minimum)?, f64::max);
        }
        if let Some(maximum) = &definition.maximum {
            value = value.map2(self.value(maximum)?, f64::min);
        }
        Ok(match definition.rounding {
            Rounding::Float => value,
            Rounding::IntRound => value.map(f64::round),
            Rounding::IntFloor => value.map(f64::floor),
            Rounding::IntCeil => value.map(f64::ceil),
        })
    }

    /// An attribute's current value: its source's value with the item's modifiers applied.
    fn attribute(&self, path: &str) -> Val {
        if let Some(reason) = &self.poison {
            return Err(reason.clone());
        }
        self.guarded(|| {
            let source = self
                .data
                .attributes
                .get(path)
                .ok_or_else(|| format!("attribute {} is not in the database", short(path)))?;
            let modified = |base: Num| match self.stacks.get(path) {
                Some(stack) => stack.apply(base),
                None => Ok(base),
            };
            match source {
                AttributeSource::Constant(value) => Ok(Num::exact(*value)),
                AttributeSource::Property {
                    context,
                    resource,
                    property,
                } => {
                    if context == POOL_CONTEXT && self.pool.is_some() {
                        if resource.as_deref() != self.pool {
                            return Err(format!(
                                "{} belongs to another resource pool",
                                short(path)
                            ));
                        }
                        // Without the item the pool is empty: the card shows what the item
                        // adds.
                        return modified(Num::exact(0.0));
                    }
                    if PLAYER_CONTEXTS.contains(&context.as_str()) {
                        // The carrier's attribute: known only where the class default is.
                        let base = player_base(property).map(Num::exact).ok_or_else(|| {
                            format!(
                                "{} is the carrier's {property}, whose base value is not known",
                                short(path)
                            )
                        })?;
                        return modified(base);
                    }
                    if !self.own_contexts.contains(&context.as_str()) {
                        return Err(format!(
                            "{} reads {property} through {}, which is not part of the item",
                            short(path),
                            if context.is_empty() {
                                "no context"
                            } else {
                                context
                            }
                        ));
                    }
                    modified(self.property(property)?)
                }
                AttributeSource::SlotGrade(slot) => {
                    Ok(Num::exact(match self.grades.get(slot.as_str()) {
                        Some(&(grade, true)) => grade as f32,
                        _ => 0.0,
                    }))
                }
                AttributeSource::Manufacturer { cases, otherwise } => {
                    let manufacturer = self
                        .manufacturer
                        .ok_or("the item's manufacturer is not known")?;
                    // Names are compared as the engine compares names: without case.
                    let case = cases
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(manufacturer));
                    self.value(case.map_or(otherwise, |(_, value)| value))
                }
                AttributeSource::Product(left, right) => {
                    if self.stacks.contains_key(path) {
                        return Err(format!(
                            "{} is computed from other attributes but also has modifiers",
                            short(path)
                        ));
                    }
                    Ok(self.value(left)?.map2(self.value(right)?, |a, b| a * b))
                }
                AttributeSource::Unsupported(what) => Err(format!(
                    "{} is resolved by {what}, which is not modelled",
                    short(path)
                )),
            }
        })
    }

    /// The base value of one of the item's own properties.
    fn property(&self, name: &str) -> Val {
        match self.properties.get(name) {
            Some(Base::Number(value)) => Ok(Num::exact(*value)),
            Some(Base::Data(data)) => self.value(data),
            Some(Base::Unknown(reason)) => Err(reason.clone()),
            None => Err(format!("the item's {name} is not modelled")),
        }
    }

    fn add_modifier(&mut self, attribute: &'a str, kind: ModifierKind, value: Val) {
        self.stacks.entry(attribute).or_default().add(kind, value);
    }

    /// Adds a definition's own effects (`WeaponAttributeEffects`) and, when `external` is set,
    /// its external effects, which act on the carrier.
    fn add_effects(&mut self, definition: &'a StatDefinition, external: bool) {
        for effect in &definition.effects {
            let value = self.value(&effect.value);
            self.add_modifier(&effect.attribute, effect.kind, value);
        }
        if !external {
            return;
        }
        for effect in &definition.external_effects {
            // An external effect reaches the item's own attributes only through the carrier,
            // never directly, so one that names an own attribute does not touch the card.
            if self.is_own_attribute(&effect.attribute) {
                continue;
            }
            let value = self.value(&effect.value);
            self.add_modifier(&effect.attribute, effect.kind, value);
        }
    }

    /// Whether the attribute reads one of the item's own properties.
    fn is_own_attribute(&self, attribute: &str) -> bool {
        matches!(
            self.data.attributes.get(attribute),
            Some(AttributeSource::Property { context, .. })
                if self.own_contexts.contains(&context.as_str())
        )
    }

    /// Adds the type's slot effects at the grade the definitions' upgrades give each slot. A
    /// slot that no upgrade activates has no effect.
    fn add_slot_effects(&mut self, item_type: &'a StatDefinition, all: &[&'a StatDefinition]) {
        let mut grades: HashMap<&'a str, (i32, bool)> = HashMap::new();
        for definition in all {
            for upgrade in &definition.slot_upgrades {
                let entry = grades.entry(upgrade.slot.as_str()).or_default();
                entry.0 = entry.0.saturating_add(upgrade.grade_increase);
                entry.1 |= upgrade.activates;
            }
        }
        if grades.values().filter(|(_, active)| *active).count() > SLOT_MAX_ACTIVATED {
            self.poison
                .get_or_insert("more attribute slots are active than the game allows".into());
        }
        self.grades = grades;
        for effect in &item_type.slot_effects {
            let Some(&(grade, true)) = self.grades.get(effect.slot.as_str()) else {
                continue;
            };
            let mut grade = grade;
            if let Some(minimum) = effect.minimum_grade {
                grade = grade.max(minimum);
            }
            if let Some(maximum) = effect.maximum_grade {
                grade = grade.min(maximum);
            }
            let value = self.value(&effect.base).and_then(|base| {
                let per_grade = self.value(&effect.per_grade)?;
                Ok(base.map2(per_grade, |b, p| b + p * f64::from(grade)))
            });
            if effect.fun_stats {
                self.fun_slots.push((effect, value.clone()));
            }
            self.add_modifier(&effect.attribute, effect.kind, value);
        }
    }

    fn holds(&self, condition: &Condition) -> Result<bool, String> {
        let left = self.attribute(&condition.attribute)?;
        let right = match &condition.other_attribute {
            Some(attribute) => self.attribute(attribute)?,
            None => Num::exact(condition.constant),
        };
        let compare = |a: f32, b: f32| match condition.comparison {
            Comparison::EqualTo => a == b,
            Comparison::NotEqualTo => a != b,
            Comparison::LessThan => a < b,
            Comparison::LessThanOrEqual => a <= b,
            Comparison::GreaterThan => a > b,
            Comparison::GreaterThanOrEqual => a >= b,
        };
        Ok(compare(left.v, right.v))
    }

    /// Applies a presentation's remapping and inversion to one number.
    fn transform(&self, presentation: &Presentation, x: f32) -> Result<(f32, bool), String> {
        let mut y = x;
        let mut clamped = false;
        if let Some(remap) = &presentation.remap {
            let bound = |data: &ValueData| self.value(data).map(|n| n.v);
            let (in_min, in_max) = (bound(&remap.input_min)?, bound(&remap.input_max)?);
            let (out_min, out_max) = (bound(&remap.output_min)?, bound(&remap.output_max)?);
            if in_max == in_min {
                return Err("the presentation's input range is empty".into());
            }
            let widen = f64::from;
            let t = (widen(y) - widen(in_min)) / (widen(in_max) - widen(in_min));
            clamped = !(0.0..=1.0).contains(&t);
            y = (widen(out_min) + t.clamp(0.0, 1.0) * (widen(out_max) - widen(out_min))) as f32;
        }
        if presentation.inverse {
            if y <= 0.0 {
                return Err("the value to invert is not positive".into());
            }
            y = (1.0 / f64::from(y)) as f32;
        }
        if !y.is_finite() {
            return Err("the value is not finite".into());
        }
        Ok((y, clamped))
    }

    /// The number as the card prints it, and why it may be off (`None` for a full
    /// calculation).
    fn present(
        &self,
        presentation: &Presentation,
        number: Num,
    ) -> Result<(f64, String, Option<String>), String> {
        let (y, clamped) = self.transform(presentation, number.v)?;
        let transformed = presentation.remap.is_some() || presentation.inverse;
        let (value, display, on_boundary) = rounded(presentation, y, number.exact && !transformed);
        let approximate = if clamped {
            Some(
                "the value lies outside the presentation's input range; clamping is assumed"
                    .to_string(),
            )
        } else if on_boundary {
            Some(
                "the value lies on a rounding boundary; the card may show the neighbouring value"
                    .to_string(),
            )
        } else {
            None
        };
        Ok((value, display, approximate))
    }

    /// The text appended after a row's number, when the row's condition holds.
    fn supplement(
        &self,
        attribute: &str,
        condition: Option<&Condition>,
    ) -> Result<Option<String>, String> {
        if let Some(condition) = condition {
            if !self.holds(condition)? {
                return Ok(None);
            }
        }
        let Some(PresentationRule::Plain(presentation)) =
            self.data.presentation(self.list, attribute)
        else {
            return Err(format!("{} has no presentation", short(attribute)));
        };
        let (_, display, approximate) = self.present(presentation, self.attribute(attribute)?)?;
        if let Some(reason) = approximate {
            return Err(reason);
        }
        Ok(Some(format!(
            "{}{display}{}",
            presentation.prefix.as_deref().unwrap_or(""),
            presentation.suffix.as_deref().unwrap_or("")
        )))
    }

    /// The card's rows for the type's `UIStatList`. `damage_type` selects the presentation of
    /// rows that depend on it; such a row is absent when the damage type has none.
    fn rows(
        &self,
        item_type: &StatDefinition,
        damage_type: Result<Option<&str>, String>,
    ) -> Vec<StatRow> {
        let mut rows = Vec::new();
        for stat in &item_type.ui_stats {
            let Some(attribute) = &stat.attribute else {
                continue;
            };
            let unknown = |label: Option<String>, reason: String| StatRow {
                attribute: attribute.clone(),
                label,
                value: StatValue::Unknown { reason },
            };
            let presentation = match self.data.presentation(self.list, attribute) {
                None => {
                    rows.push(unknown(
                        None,
                        format!("{} has no presentation in the database", short(attribute)),
                    ));
                    continue;
                }
                Some(PresentationRule::Plain(presentation)) => presentation,
                Some(PresentationRule::ByDamageType(by_type)) => match &damage_type {
                    Err(reason) => {
                        rows.push(unknown(None, reason.clone()));
                        continue;
                    }
                    Ok(damage_type) => {
                        let found = damage_type.and_then(|damage_type| {
                            by_type.iter().find(|(path, _)| path == damage_type)
                        });
                        match found {
                            Some((_, presentation)) => presentation,
                            // This damage type has no such row (a non-elemental weapon).
                            None => continue,
                        }
                    }
                },
            };
            let label = presentation.label.clone();
            if !stat.just_number {
                // The row shows the item's modifier of the attribute ("+34%"), like a line,
                // and only when the item has one: a relic lists every stat it could raise.
                let value = match self.modifier(attribute, presentation) {
                    Ok(Some(modifier)) => self.modifier_number(presentation, &modifier),
                    Ok(None) => continue,
                    Err(reason) => Err(reason),
                };
                rows.push(match value {
                    Ok((value, display, approximate)) => StatRow {
                        attribute: attribute.clone(),
                        label,
                        value: StatValue::Computed {
                            value,
                            display,
                            supplement: None,
                            approximate,
                        },
                    },
                    Err(reason) => unknown(label, reason),
                });
                continue;
            }
            let shown = self
                .attribute(attribute)
                .and_then(|number| self.present(presentation, number));
            let (value, mut display, mut approximate) = match shown {
                Ok(shown) => shown,
                Err(reason) => {
                    rows.push(unknown(label, reason));
                    continue;
                }
            };
            // The presentation's suffix follows the number: the "%" of an elemental chance.
            if let Some(suffix) = &presentation.suffix {
                display.push_str(suffix);
            }
            let supplement = match &stat.supplemental {
                None => None,
                Some(supplemental) => {
                    match self.supplement(supplemental, stat.condition.as_ref()) {
                        Ok(text) => text,
                        Err(reason) => {
                            approximate.get_or_insert(format!(
                                "whether the card appends {} could not be determined: {reason}",
                                short(supplemental)
                            ));
                            None
                        }
                    }
                }
            };
            rows.push(StatRow {
                attribute: attribute.clone(),
                label,
                value: StatValue::Computed {
                    value,
                    display,
                    supplement,
                    approximate,
                },
            });
        }
        rows
    }
}

impl<'a> Card<'a> {
    /// The item's change of an attribute, for a bonus line or a modifier-text row: `None` when
    /// the item has no modifier on it. With a percentage presentation it is a fraction of the
    /// value without the item; a change made only by scale modifiers needs no base value for
    /// that, one with adds needs the base, which is known for the item's own properties, for
    /// its resource pool (0) and for the carrier's attributes that [`player_base`] knows.
    fn modifier(
        &self,
        attribute: &str,
        presentation: &Presentation,
    ) -> Result<Option<Modifier>, String> {
        if let Some(reason) = &self.poison {
            return Err(reason.clone());
        }
        let Some(stack) = self.stacks.get(attribute) else {
            return Ok(None);
        };
        if let Some(e) = &stack.error {
            return Err(e.clone());
        }
        if presentation.inverse {
            return Err(format!(
                "how the card words a change of {} (shown inverted) is not established",
                short(attribute)
            ));
        }
        let pure_scale = stack.pre_add == 0.0 && stack.post_add == 0.0;
        if presentation.percentage {
            if presentation.remap.is_some() {
                return Err(format!(
                    "how the card words a change of {} (shown remapped) is not established",
                    short(attribute)
                ));
            }
            if pure_scale {
                // (base × factor − base) ÷ base, whatever the base.
                let factor = (1.0 + stack.positive_scale) / (1.0 + stack.negative_scale);
                return Ok(Some(Modifier {
                    value: (factor - 1.0) as f32,
                    exact: false,
                }));
            }
            let base = self.base_of(attribute)?;
            if base.v == 0.0 {
                return Err(format!(
                    "a percentage change of {} needs a base value, which is 0",
                    short(attribute)
                ));
            }
            let with = stack.apply(base)?;
            return Ok(Some(Modifier {
                value: ((f64::from(with.v) - f64::from(base.v)) / f64::from(base.v)) as f32,
                exact: false,
            }));
        }
        // A plain difference. Without scale modifiers the base cancels out.
        let base = match self.base_of(attribute) {
            Ok(base) => base,
            Err(_) if stack.positive_scale == 0.0 && stack.negative_scale == 0.0 => Num::exact(0.0),
            Err(reason) => return Err(reason),
        };
        let with = stack.apply(base)?;
        Ok(Some(Modifier {
            value: (f64::from(with.v) - f64::from(base.v)) as f32,
            exact: with.exact,
        }))
    }

    /// An attribute's value without the item: its base.
    fn base_of(&self, attribute: &str) -> Val {
        let source = self
            .data
            .attributes
            .get(attribute)
            .ok_or_else(|| format!("attribute {} is not in the database", short(attribute)))?;
        match source {
            AttributeSource::Property {
                context,
                resource,
                property,
            } => {
                if context == POOL_CONTEXT
                    && self.pool.is_some()
                    && resource.as_deref() == self.pool
                {
                    return Ok(Num::exact(0.0));
                }
                if PLAYER_CONTEXTS.contains(&context.as_str()) {
                    return player_base(property).map(Num::exact).ok_or_else(|| {
                        format!(
                            "{} is the carrier's {property}, whose base value is not known",
                            short(attribute)
                        )
                    });
                }
                if self.own_contexts.contains(&context.as_str()) {
                    return self.property(property);
                }
                Err(format!(
                    "{} is read through {context}, which is not part of the item",
                    short(attribute)
                ))
            }
            _ => Err(format!(
                "{} is not a property, so it has no base to change",
                short(attribute)
            )),
        }
    }

    /// A modifier's number as the card prints it ("+8%", "2", "-15%"), with the rounded value
    /// and any reason it may be off.
    fn modifier_number(
        &self,
        presentation: &Presentation,
        modifier: &Modifier,
    ) -> Result<(f64, String, Option<String>), String> {
        let mut x = f64::from(modifier.value);
        if presentation.percentage {
            x *= 100.0;
        }
        let mut clamped = false;
        if let Some(remap) = &presentation.remap {
            let bound = |data: &ValueData| self.value(data).map(|n| f64::from(n.v));
            let (in_min, in_max) = (bound(&remap.input_min)?, bound(&remap.input_max)?);
            let (out_min, out_max) = (bound(&remap.output_min)?, bound(&remap.output_max)?);
            if in_max == in_min {
                return Err("the presentation's input range is empty".into());
            }
            let t = (x - in_min) / (in_max - in_min);
            clamped = !(0.0..=1.0).contains(&t);
            x = out_min + t.clamp(0.0, 1.0) * (out_max - out_min);
        }
        let shown = match presentation.sign {
            SignStyle::AsIs | SignStyle::AsIsAnyStyle => x,
            SignStyle::Positive => x.abs(),
            SignStyle::Negative => -x.abs(),
            SignStyle::OppositeSign => -x,
        };
        let (value, digits, on_boundary) = rounded(presentation, shown as f32, modifier.exact);
        let sign = if value > 0.0 && presentation.plus_sign {
            "+"
        } else {
            ""
        };
        let display = format!(
            "{sign}{digits}{}",
            if presentation.percentage { "%" } else { "" }
        );
        let approximate = if clamped {
            Some(
                "the value lies outside the presentation's input range; clamping is assumed".into(),
            )
        } else if !presentation.bigger_is_better {
            Some(
                "the attribute is one where smaller is better; whether the card flips the sign \
                 of its change is not established"
                    .into(),
            )
        } else if on_boundary {
            Some(
                "the value lies on a rounding boundary; the card may show the neighbouring value"
                    .into(),
            )
        } else {
            None
        };
        Ok((value, display, approximate))
    }

    /// The wording of a line around its number: `Description` with `$NUMBER$` replaced when the
    /// presentation places the number itself, else prefix, number, suffix and description.
    fn line_text(presentation: &Presentation, number: &str) -> String {
        let label = presentation.label.as_deref().unwrap_or("");
        let text = if presentation.custom_placement && label.contains("$NUMBER$") {
            label.replace("$NUMBER$", number)
        } else {
            let mut parts: Vec<&str> = Vec::new();
            if presentation.include_prefix {
                parts.extend(presentation.prefix.as_deref());
            }
            parts.push(number);
            if presentation.include_suffix {
                parts.extend(presentation.suffix.as_deref());
            }
            parts.push(label);
            parts.join(" ")
        };
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// A line for a presentation that names an attribute: the item's modifier of it, worded by
    /// the presentation. `Ok(None)` when the item does not modify the attribute (no line).
    fn bonus_line(
        &self,
        attribute: &str,
        presentation: &Presentation,
        priority: f32,
    ) -> Result<Option<CardLine>, String> {
        let Some(modifier) = self.modifier(attribute, presentation)? else {
            return Ok(None);
        };
        if modifier.value == 0.0 {
            return Ok(None);
        }
        Ok(Some(self.worded_line(
            attribute,
            presentation,
            &modifier,
            priority,
        )))
    }

    fn worded_line(
        &self,
        attribute: &str,
        presentation: &Presentation,
        modifier: &Modifier,
        priority: f32,
    ) -> CardLine {
        let (text, note) = if !presentation.display_number {
            (
                presentation
                    .label
                    .clone()
                    .or_else(|| presentation.no_constraint_text.clone()),
                None,
            )
        } else {
            match self.modifier_number(presentation, modifier) {
                Ok((_, number, approximate)) => {
                    (Some(Self::line_text(presentation, &number)), approximate)
                }
                Err(reason) => (None, Some(reason)),
            }
        };
        CardLine {
            kind: CardLineKind::Bonus,
            text,
            note,
            attribute: Some(attribute.to_string()),
            color: presentation.color,
            priority,
        }
    }

    /// Every line below the rows: fixed text, the lines for the attributes the definitions list
    /// for the card, the slot effects shown as lines, and the damage type's lines. Highest
    /// priority first; among equals, parts before the type, then the damage type.
    fn lines(
        &self,
        item_type: &'a StatDefinition,
        parts: &[&'a StatDefinition],
        damage_type: Option<&str>,
    ) -> Vec<CardLine> {
        let mut lines: Vec<CardLine> = Vec::new();
        let mut lined: HashSet<&str> = HashSet::new();
        let custom = |lines: &mut Vec<CardLine>, entry: &'a CustomPresentation| {
            let p = &entry.presentation;
            match &entry.attribute {
                None => {
                    let text = p.label.clone().or_else(|| p.no_constraint_text.clone());
                    let note = match &text {
                        Some(text) if text.starts_with('$') => {
                            Some("the text is a localisation key the database does not hold".into())
                        }
                        Some(_) if p.display_number => {
                            Some("the line shows a number but names no attribute".into())
                        }
                        Some(_) => None,
                        None => Some("the presentation has no text".into()),
                    };
                    lines.push(CardLine {
                        kind: CardLineKind::Text,
                        text: if note.is_none() { text } else { None },
                        note,
                        attribute: None,
                        color: p.color,
                        priority: p.base_priority,
                    });
                }
                Some(attribute) => {
                    let line = if p.display_number {
                        self.bonus_line(attribute, p, p.base_priority)
                            .transpose()
                            .map(|r| {
                                r.unwrap_or_else(|reason| CardLine {
                                    kind: CardLineKind::Bonus,
                                    text: None,
                                    note: Some(reason),
                                    attribute: Some(attribute.clone()),
                                    color: p.color,
                                    priority: p.base_priority,
                                })
                            })
                    } else {
                        // Text that stands for an attribute: shown when the attribute's value is
                        // not 0, which is assumed, not established.
                        match self.attribute(attribute) {
                            Ok(n) if n.v == 0.0 => None,
                            Ok(_) => Some(CardLine {
                                kind: CardLineKind::Text,
                                text: p.label.clone().or_else(|| p.no_constraint_text.clone()),
                                note: Some(format!(
                                    "the line stands for {}, which is not 0 here; when the card \
                                     shows such a line is not established",
                                    short(attribute)
                                )),
                                attribute: Some(attribute.clone()),
                                color: p.color,
                                priority: p.base_priority,
                            }),
                            Err(reason) => Some(CardLine {
                                kind: CardLineKind::Text,
                                text: None,
                                note: Some(reason),
                                attribute: Some(attribute.clone()),
                                color: p.color,
                                priority: p.base_priority,
                            }),
                        }
                    };
                    lines.extend(line);
                }
            }
        };
        let definitions = parts.iter().copied().chain(std::iter::once(item_type));
        for definition in definitions {
            for card_attribute in &definition.card_attributes {
                let attribute = card_attribute.attribute.as_str();
                if !lined.insert(attribute) {
                    continue;
                }
                let Some(PresentationRule::Plain(presentation)) =
                    self.data.presentation(self.list, attribute)
                else {
                    lines.push(CardLine {
                        kind: CardLineKind::Bonus,
                        text: None,
                        note: Some(format!(
                            "{} has no presentation in the database",
                            short(attribute)
                        )),
                        attribute: Some(attribute.to_string()),
                        color: None,
                        priority: card_attribute.priority_increase,
                    });
                    continue;
                };
                let priority = presentation.base_priority + card_attribute.priority_increase;
                match self.bonus_line(attribute, presentation, priority) {
                    Ok(Some(line)) => lines.push(line),
                    Ok(None) => {}
                    Err(reason) => lines.push(CardLine {
                        kind: CardLineKind::Bonus,
                        text: None,
                        note: Some(reason),
                        attribute: Some(attribute.to_string()),
                        color: presentation.color,
                        priority,
                    }),
                }
            }
            for entry in &definition.presentations {
                custom(&mut lines, entry);
            }
        }
        for (slot, value) in &self.fun_slots {
            let attribute = slot.attribute.as_str();
            let Some(PresentationRule::Plain(presentation)) =
                self.data.presentation(self.list, attribute)
            else {
                lines.push(CardLine {
                    kind: CardLineKind::Bonus,
                    text: None,
                    note: Some(format!(
                        "{} has no presentation in the database",
                        short(attribute)
                    )),
                    attribute: Some(attribute.to_string()),
                    color: None,
                    priority: 0.0,
                });
                continue;
            };
            // The slot's own modifier, not the whole item's.
            let line = match value {
                Ok(n) if n.v == 0.0 => None,
                Ok(n) => {
                    let modifier = Modifier {
                        value: n.v,
                        exact: n.exact,
                    };
                    let mut line = self.worded_line(
                        attribute,
                        presentation,
                        &modifier,
                        presentation.base_priority,
                    );
                    if presentation.percentage && !matches!(slot.kind, ModifierKind::Scale) {
                        line.text = None;
                        line.note = Some(format!(
                            "a percentage for an added {} needs a base value",
                            short(attribute)
                        ));
                    }
                    Some(line)
                }
                Err(reason) => Some(CardLine {
                    kind: CardLineKind::Bonus,
                    text: None,
                    note: Some(reason.clone()),
                    attribute: Some(attribute.to_string()),
                    color: presentation.color,
                    priority: presentation.base_priority,
                }),
            };
            lines.extend(line);
        }
        if let Some(entries) = damage_type.and_then(|path| self.data.damage_types.get(path)) {
            for entry in entries {
                custom(&mut lines, entry);
            }
        }
        lines.sort_by(|a, b| b.priority.total_cmp(&a.priority));
        lines
    }
}

/// Rounds a presented number the way its presentation says and prints it. The flag tells
/// whether the number lies within float error of a rounding boundary; a number that is `exact`
/// (straight from the data) has no float error, so only a true tie counts for it.
fn rounded(presentation: &Presentation, y: f32, exact: bool) -> (f64, String, bool) {
    let y = f64::from(y);
    let tolerance = if exact {
        0.0
    } else {
        BOUNDARY_TOLERANCE * y.abs().max(1.0)
    };
    let near = |distance: f64| distance.abs() <= tolerance;
    match presentation.rounding {
        Rounding::Float => {
            let precision = presentation.precision.clamp(0, 6) as usize;
            let display = format!("{y:.precision$}");
            let scale = 10f64.powi(precision as i32);
            let fraction = (y * scale).rem_euclid(1.0);
            let value = display.parse().unwrap_or(y);
            (value, display, near((fraction - 0.5) / scale))
        }
        Rounding::IntRound => {
            let value = y.round();
            (value, format!("{value:.0}"), near(y.rem_euclid(1.0) - 0.5))
        }
        Rounding::IntFloor | Rounding::IntCeil => {
            let value = if presentation.rounding == Rounding::IntFloor {
                y.floor()
            } else {
                y.ceil()
            };
            let fraction = y.rem_euclid(1.0);
            // A whole number is its own floor and ceiling; it is a boundary only when float
            // error could have pushed it across.
            (
                value,
                format!("{value:.0}"),
                !exact && (near(fraction) || near(1.0 - fraction)),
            )
        }
    }
}

impl ItemDatabase {
    /// The item's card under [`ItemDatabase::DLC_RESOLUTION`]: items of a DLC set come back
    /// unavailable until DLC resolution has been verified in game.
    ///
    /// **Computed, unverified:** until the results have been compared with cards in the running
    /// game, treat them as provisional. The rules, and which of them are assumed, are in
    /// `docs/research/2026-11-09-item-stats.md`. Weapons and a shield's capacity, recharge rate
    /// and recharge delay are computed; other kinds of item come back unavailable.
    ///
    /// Rows are in the card's order. A row whose inputs cannot be evaluated is
    /// [`StatValue::Unknown`] with the reason, never a guess; a row that may differ from the
    /// card (a rounding boundary) says so in `approximate`.
    pub fn stats(&self, item: &ItemRefs, level: ItemLevel) -> ItemStats {
        self.stats_with(item, level, Self::DLC_RESOLUTION)
    }

    /// [`ItemDatabase::stats`] with an explicit DLC policy. With [`DlcResolution::Verified`],
    /// items of a DLC set are resolved through the unverified H-set and H-flag hypotheses.
    pub fn stats_with(&self, item: &ItemRefs, level: ItemLevel, dlc: DlcResolution) -> ItemStats {
        if item.is_virtual() {
            return ItemStats::unavailable("a placeholder item has no card");
        }
        let Some(set) = item.set else {
            return ItemStats::unavailable("the item has no set id");
        };
        if set != 0 && dlc == DlcResolution::Unverified {
            return ItemStats::unavailable("DLC items are not resolved until verified in game");
        }
        let path = |category: AssetCategory, value: Option<u32>| {
            self.path_with(category, value?, set, dlc)
        };
        let Some(type_path) = path(AssetCategory::types(item.is_weapon), item.item_type) else {
            return ItemStats::unavailable("the item's type is not in the asset tables");
        };
        let Some(item_type) = self.stat_data.definitions.get(type_path) else {
            return ItemStats::unavailable(format!(
                "the type {} has no stat definition in the database",
                short(type_path)
            ));
        };
        let weapon = item_type.weapon.as_ref().filter(|_| item.is_weapon);
        let class_name = match self.object(type_path) {
            Some(ObjectInfo::ItemType { class_name, .. }) if !item.is_weapon => class_name.as_str(),
            _ => "",
        };
        // Which list of presentations the card uses, and whether its rows are the carrier's
        // shield pool.
        let (list, is_shield) = match class_name {
            _ if weapon.is_some() => (PresentationList::Weapon, false),
            "ShieldDefinition" => (PresentationList::Shield, true),
            "GrenadeModDefinition" => (PresentationList::GrenadeMod, false),
            "ClassModDefinition" | "CrossDLCClassModDefinition" => {
                (PresentationList::ClassMod, false)
            }
            "ArtifactDefinition" => (PresentationList::Artifact, false),
            _ => {
                return ItemStats::unavailable(
                    "only weapon, shield, grenade mod, class mod and relic cards are computed",
                )
            }
        };
        let is_item = weapon.is_none();

        let mut card = Card {
            data: &self.stat_data,
            list,
            own_contexts: if is_item {
                ITEM_CONTEXTS
            } else {
                WEAPON_CONTEXTS
            },
            pool: is_shield.then_some(SHIELD_RESOURCE),
            manufacturer: path(AssetCategory::Manufacturers, item.manufacturer).map(short),
            properties: HashMap::new(),
            stacks: HashMap::new(),
            grades: HashMap::new(),
            fun_slots: Vec::new(),
            poison: None,
            depth: Cell::new(0),
        };
        if class_name == "GrenadeModDefinition" {
            for property in GRENADE_MOD_PROPERTIES {
                card.properties.insert(property, Base::Number(0.0));
            }
        }
        let level = match level.grade_index {
            Some(level) => Base::Number(level as f32),
            None => Base::Unknown("the item's level is not known".into()),
        };
        card.properties.insert("ExpLevel", level);
        if let Some(weapon) = weapon {
            card.properties.extend([
                ("InstantHitDamage", Base::Data(&weapon.damage)),
                (
                    "StatusEffectDamage",
                    Base::Data(&weapon.status_effect_damage),
                ),
                (
                    "BaseStatusEffectChanceModifier",
                    Base::Data(&weapon.base_status_effect_chance),
                ),
                (
                    "StatusEffectChanceModifier",
                    Base::Number(WEAPON_STATUS_EFFECT_CHANCE_MODIFIER),
                ),
                ("ClipSize", Base::Number(weapon.clip_size as f32)),
                ("FireInterval", Base::Number(weapon.fire_interval)),
                ("ReloadTime", Base::Number(weapon.reload_time)),
                ("Spread", Base::Number(weapon.spread)),
                (
                    "ProjectilesPerShot",
                    Base::Number(weapon.projectiles_per_shot as f32),
                ),
                ("ShotCost", Base::Number(weapon.shot_cost as f32)),
                (
                    "PerShotAccuracyImpulse",
                    Base::Number(weapon.per_shot_accuracy_impulse),
                ),
                (
                    "AutomaticBurstCount",
                    Base::Number(weapon.automatic_burst_count as f32),
                ),
            ]);
        }

        // The parts, in slot order. A non-empty slot must resolve to a part with a definition.
        let parts_category = AssetCategory::parts(item.is_weapon);
        let mut parts: Vec<&StatDefinition> = Vec::new();
        let mut poison = None;
        for index in 0..PART_SLOTS {
            let Some(value) = item.parts[index].filter(|&v| v != parts_category.all_ones()) else {
                continue;
            };
            let definition = path(parts_category, Some(value))
                .and_then(|part_path| self.stat_data.definitions.get(part_path));
            match definition {
                Some(definition) => parts.push(definition),
                None => {
                    poison.get_or_insert(format!(
                        "part slot {index} refers to a part that is not in the database"
                    ));
                }
            }
        }
        let mut all = vec![item_type];
        all.extend(&parts);
        if let Some(problem) = all.iter().find_map(|d| d.problems.first()) {
            poison.get_or_insert(format!("a definition could not be read: {problem}"));
        }

        // `WillowWeapon.InitializeInternal` and `WillowItem.InitializeInternal`: the type's and
        // parts' effects, then slot effects. External effects act on the carrier; they are
        // collected too, since the card's rows (a shield's pool, a class mod's bonuses) and its
        // bonus lines (a bayonet's melee damage) show them. They modify the carrier's
        // attributes, never the weapon's own, so the weapon rows are untouched.
        for definition in &all {
            card.add_effects(definition, true);
        }
        card.add_slot_effects(item_type, &all);
        if card.poison.is_none() {
            card.poison = poison;
        }

        // A part's custom damage type replaces the type's default.
        let mut custom: Vec<&str> = parts
            .iter()
            .filter_map(|part| part.damage_type.as_deref())
            .collect();
        custom.dedup();
        let damage_type = match custom[..] {
            [] => Ok(weapon.and_then(|w| w.damage_type.as_deref())),
            [one] => Ok(Some(one)),
            _ => Err("several parts set a damage type".to_string()),
        };

        let mut texts: Vec<CardText> = all.iter().flat_map(|d| d.texts.iter().cloned()).collect();
        texts.sort_by(|a, b| b.priority.total_cmp(&a.priority));
        let lines = card.lines(item_type, &parts, damage_type.clone().ok().flatten());
        ItemStats {
            unavailable: None,
            rows: card.rows(item_type, damage_type),
            texts,
            lines,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::stat_data::{
        AttributeEffect, CardAttribute, Formula, Remap, SlotEffect, SlotUpgrade, UiStat, WeaponBase,
    };
    use crate::database::{AssetSet, Rgba};

    const W: u32 = 131071;
    const I: u32 = 65535;
    const DAMAGE: &str = "A.Damage";
    const SPREAD: &str = "A.Spread";
    const INTERVAL: &str = "A.Interval";
    const RELOAD: &str = "A.Reload";
    const CLIP: &str = "A.Clip";
    const PELLETS: &str = "A.Pellets";
    const DOT: &str = "A.Dot";
    const CHANCE: &str = "A.Chance";
    const SHOT_COST: &str = "A.ShotCost";
    const CRIT: &str = "D_Attributes.GameplayAttributes.PlayerCriticalHitBonus";
    const MELEE: &str = "D_Attributes.DamageSourceModifiers.InstigatedMeleeDamageModifier";
    const CAPACITY: &str = "A.Capacity";
    const DELAY: &str = "A.Delay";

    fn constant(v: f32) -> ValueData {
        ValueData::constant(v)
    }

    fn effect(attribute: &str, kind: ModifierKind, value: ValueData) -> AttributeEffect {
        AttributeEffect {
            attribute: attribute.into(),
            kind,
            value,
        }
    }

    fn scale(attribute: &str, v: f32) -> AttributeEffect {
        effect(attribute, ModifierKind::Scale, constant(v))
    }

    fn upgrade(slot: &str, grade: i32) -> SlotUpgrade {
        SlotUpgrade {
            slot: slot.into(),
            grade_increase: grade,
            activates: true,
        }
    }

    fn property(context: &str, name: &str) -> AttributeSource {
        AttributeSource::Property {
            context: context.into(),
            resource: None,
            property: name.into(),
        }
    }

    fn pool(name: &str) -> AttributeSource {
        AttributeSource::Property {
            context: POOL_CONTEXT.into(),
            resource: Some(SHIELD_RESOURCE.into()),
            property: name.into(),
        }
    }

    fn plain(label: &str, rounding: Rounding) -> Presentation {
        Presentation {
            label: Some(label.into()),
            no_constraint_text: None,
            base_priority: 0.0,
            percentage: true,
            percent_as_float: false,
            plus_sign: true,
            display_number: true,
            custom_placement: false,
            bigger_is_better: true,
            sign: SignStyle::AsIs,
            color: None,
            include_prefix: true,
            include_suffix: true,
            inverse: false,
            remap: None,
            rounding,
            precision: 1,
            prefix: None,
            suffix: None,
        }
    }

    fn stat(attribute: &str) -> UiStat {
        UiStat {
            attribute: Some(attribute.into()),
            supplemental: None,
            condition: None,
            just_number: true,
        }
    }

    fn table(paths: &[&str]) -> Vec<Vec<Option<String>>> {
        vec![paths.iter().map(|p| Some(p.to_string())).collect()]
    }

    /// A small game. Weapon type 0 is a rifle: damage `8 × 1.13^level × 1.08`, clip 20, fire
    /// interval 0.115 s, reload 3.5 s, spread 2, one projectile. Its slots: `Damage` (+3% per
    /// grade) and `Spread` (−5% per grade, minimum grade −2).
    ///
    /// Weapon parts: 0 body (+10% damage, +20% interval), 1 grip (damage slot +4, spread slot
    /// −3; +2 clip on a Jakobs gun), 2 fire element (−20% damage, fire damage type), 3 barrel
    /// (+4 pellets, −1000 clip scale, +1 clip post-add), 4 a part with no stat definition,
    /// 5 a part whose definition has a problem, 6 a part with an effect it cannot evaluate,
    /// 7 a title with red text, 8 a slag element, 9 a Hyperion-style barrel (+0.15 critical hit
    /// bonus on the carrier, listed for the card), 10 a bayonet (+0.5 melee on the carrier,
    /// listed for the card, with a fixed text line). The type lists the shot cost for the card
    /// and adds 1 to it.
    ///
    /// Item type 0 is a shield: capacity `10 × level` pre-add, delay 3.5 pre-add; slots
    /// `Capacity` (+3% per grade) and `Delay` (−4% per grade). Item part 0 is a battery
    /// (capacity slot +5, delay slot +10). Item type 1 is a relic.
    fn synthetic() -> ItemDatabase {
        use AssetCategory::*;
        let mut base = AssetSet::new(None);
        base.set_tables(WeaponTypes, table(&["T.Rifle"]));
        base.set_tables(
            WeaponParts,
            table(&[
                "P.Body",
                "P.Grip",
                "P.Fire",
                "P.Barrel",
                "P.Missing",
                "P.Broken",
                "P.Odd",
                "P.Title",
                "P.Slag",
                "P.Hyperion",
                "P.Bayonet",
            ]),
        );
        base.set_tables(ItemTypes, table(&["I.Shield", "I.Relic"]));
        base.set_tables(ItemParts, table(&["IP.Battery"]));
        base.set_tables(Manufacturers, table(&["GD_M.M.Jakobs", "GD_M.M.Vladof"]));
        let mut dlc = AssetSet::new(Some("Tulip".into()));
        dlc.set_tables(WeaponParts, table(&["P.Body"]));
        let mut db = ItemDatabase::new("synthetic");
        db.sets.insert(0, base);
        db.sets.insert(2, dlc);
        let item_type = |class: &str| ObjectInfo::ItemType {
            class_name: class.into(),
            item_name: None,
            name_is_full: false,
            base_rarity: crate::database::RarityValue::None,
        };
        db.objects
            .insert("I.Shield".into(), item_type("ShieldDefinition"));
        db.objects
            .insert("I.Relic".into(), item_type("UsableItemDefinition"));

        let s = &mut db.stat_data;
        let mut define = |path: &str, definition: StatDefinition| {
            s.definitions.insert(path.to_string(), definition);
        };
        define(
            "T.Rifle",
            StatDefinition {
                weapon: Some(WeaponBase {
                    clip_size: 20,
                    fire_interval: 0.115,
                    reload_time: 3.5,
                    spread: 2.0,
                    projectiles_per_shot: 1,
                    shot_cost: 1,
                    per_shot_accuracy_impulse: 0.0,
                    automatic_burst_count: 0,
                    damage: ValueData {
                        initialization: Some("Init.Damage".into()),
                        scale: 1.08,
                        ..constant(20.0)
                    },
                    status_effect_damage: ValueData {
                        initialization: Some("Init.Damage".into()),
                        ..constant(1.0)
                    },
                    base_status_effect_chance: constant(0.3),
                    damage_type: Some("D.Normal".into()),
                }),
                slot_effects: vec![
                    SlotEffect {
                        slot: "Damage".into(),
                        external: false,
                        attribute: DAMAGE.into(),
                        kind: ModifierKind::Scale,
                        base: constant(0.0),
                        per_grade: constant(0.03),
                        minimum_grade: None,
                        maximum_grade: None,
                        fun_stats: false,
                    },
                    SlotEffect {
                        slot: "Spread".into(),
                        external: false,
                        attribute: SPREAD.into(),
                        kind: ModifierKind::Scale,
                        base: constant(0.0),
                        per_grade: constant(-0.05),
                        minimum_grade: Some(-2),
                        maximum_grade: None,
                        fun_stats: false,
                    },
                    // A slot nothing activates, whose base value would otherwise show.
                    SlotEffect {
                        slot: "Unused".into(),
                        external: false,
                        attribute: CLIP.into(),
                        kind: ModifierKind::PreAdd,
                        base: constant(100.0),
                        per_grade: constant(0.0),
                        minimum_grade: None,
                        maximum_grade: None,
                        fun_stats: false,
                    },
                ],
                effects: vec![effect(SHOT_COST, ModifierKind::PreAdd, constant(1.0))],
                card_attributes: vec![CardAttribute {
                    attribute: SHOT_COST.into(),
                    priority_increase: 0.5,
                }],
                ui_stats: vec![
                    UiStat {
                        supplemental: Some(PELLETS.into()),
                        condition: Some(Condition {
                            attribute: PELLETS.into(),
                            comparison: Comparison::GreaterThan,
                            other_attribute: None,
                            constant: 1.0,
                        }),
                        ..stat(DAMAGE)
                    },
                    stat(SPREAD),
                    stat(INTERVAL),
                    stat(RELOAD),
                    stat(CLIP),
                    stat(DOT),
                    stat(CHANCE),
                ],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Body",
            StatDefinition {
                effects: vec![scale(DAMAGE, 0.1), scale(INTERVAL, 0.2)],
                // A weapon's external effects act on its carrier, not on its card.
                external_effects: vec![scale(DAMAGE, 5.0)],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Grip",
            StatDefinition {
                effects: vec![effect(
                    CLIP,
                    ModifierKind::PreAdd,
                    ValueData {
                        attribute: Some("A.IsJakobs".into()),
                        scale: 2.0,
                        ..constant(0.0)
                    },
                )],
                slot_upgrades: vec![upgrade("Damage", 4), upgrade("Spread", -3)],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Fire",
            StatDefinition {
                effects: vec![scale(DAMAGE, -0.2)],
                damage_type: Some("D.Fire".into()),
                ..StatDefinition::default()
            },
        );
        define(
            "P.Hyperion",
            StatDefinition {
                external_effects: vec![effect(CRIT, ModifierKind::PreAdd, constant(0.15))],
                card_attributes: vec![CardAttribute {
                    attribute: CRIT.into(),
                    priority_increase: 0.5,
                }],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Bayonet",
            StatDefinition {
                external_effects: vec![effect(MELEE, ModifierKind::PreAdd, constant(0.5))],
                card_attributes: vec![CardAttribute {
                    attribute: MELEE.into(),
                    priority_increase: 0.5,
                }],
                presentations: vec![CustomPresentation {
                    attribute: None,
                    presentation: Presentation {
                        base_priority: 3.0,
                        display_number: false,
                        ..plain("Stabby.", Rounding::IntRound)
                    },
                }],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Slag",
            StatDefinition {
                damage_type: Some("D.Slag".into()),
                ..StatDefinition::default()
            },
        );
        define(
            "P.Barrel",
            StatDefinition {
                effects: vec![
                    effect(PELLETS, ModifierKind::PreAdd, constant(4.0)),
                    scale(CLIP, -1000.0),
                    effect(CLIP, ModifierKind::PostAdd, constant(1.0)),
                ],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Broken",
            StatDefinition {
                problems: vec!["WeaponAttributeEffects: not an array".into()],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Odd",
            StatDefinition {
                effects: vec![effect(
                    RELOAD,
                    ModifierKind::Scale,
                    ValueData {
                        attribute: Some("A.Unsupported".into()),
                        ..constant(0.0)
                    },
                )],
                ..StatDefinition::default()
            },
        );
        define(
            "P.Title",
            StatDefinition {
                texts: vec![
                    CardText {
                        text: "Second".into(),
                        color: None,
                        priority: 1.0,
                    },
                    CardText {
                        text: "Red text".into(),
                        color: Some(Rgba::new(220, 70, 70, 255)),
                        priority: 20.0,
                    },
                ],
                ..StatDefinition::default()
            },
        );
        define(
            "I.Shield",
            StatDefinition {
                external_effects: vec![
                    effect(
                        CAPACITY,
                        ModifierKind::PreAdd,
                        ValueData {
                            attribute: Some("A.ItemLevel".into()),
                            scale: 10.0,
                            ..constant(0.0)
                        },
                    ),
                    effect(DELAY, ModifierKind::PreAdd, constant(3.5)),
                ],
                slot_effects: vec![
                    SlotEffect {
                        slot: "Capacity".into(),
                        external: true,
                        attribute: CAPACITY.into(),
                        kind: ModifierKind::Scale,
                        base: constant(0.0),
                        per_grade: constant(0.03),
                        minimum_grade: None,
                        maximum_grade: None,
                        fun_stats: false,
                    },
                    SlotEffect {
                        slot: "Delay".into(),
                        external: true,
                        attribute: DELAY.into(),
                        kind: ModifierKind::Scale,
                        base: constant(0.0),
                        per_grade: constant(-0.04),
                        minimum_grade: None,
                        maximum_grade: None,
                        fun_stats: false,
                    },
                ],
                slot_upgrades: vec![upgrade("Capacity", 0), upgrade("Delay", 0)],
                ui_stats: vec![
                    stat(CAPACITY),
                    stat(DELAY),
                    UiStat {
                        just_number: false,
                        ..stat("A.Special")
                    },
                    stat("A.OtherPool"),
                ],
                ..StatDefinition::default()
            },
        );
        define(
            "IP.Battery",
            StatDefinition {
                slot_upgrades: vec![upgrade("Capacity", 5), upgrade("Delay", 10)],
                ..StatDefinition::default()
            },
        );
        define("I.Relic", StatDefinition::default());

        let weapon = |name: &str| property(WEAPON_CONTEXTS[0], name);
        s.attributes.extend(
            [
                (DAMAGE, weapon("InstantHitDamage")),
                (SPREAD, weapon("Spread")),
                (INTERVAL, weapon("FireInterval")),
                (RELOAD, weapon("ReloadTime")),
                (CLIP, weapon("ClipSize")),
                (PELLETS, weapon("ProjectilesPerShot")),
                (DOT, weapon("StatusEffectDamage")),
                ("A.Level", weapon("ExpLevel")),
                ("A.BaseChance", weapon("BaseStatusEffectChanceModifier")),
                ("A.ChanceModifier", weapon("StatusEffectChanceModifier")),
                (
                    CHANCE,
                    AttributeSource::Product(
                        ValueData {
                            attribute: Some("A.BaseChance".into()),
                            ..constant(0.0)
                        },
                        ValueData {
                            attribute: Some("A.ChanceModifier".into()),
                            ..constant(0.0)
                        },
                    ),
                ),
                ("A.Scaler", AttributeSource::Constant(1.13)),
                (
                    "A.IsJakobs",
                    AttributeSource::Manufacturer {
                        cases: vec![("Jakobs".into(), constant(1.0))],
                        otherwise: constant(0.0),
                    },
                ),
                (
                    "A.Unsupported",
                    AttributeSource::Unsupported("value resolver WeirdResolver".into()),
                ),
                (CAPACITY, pool("MaxValue")),
                (DELAY, pool("OnIdleRegenerationDelay")),
                ("A.Special", property("SomethingElse", "X")),
                (
                    "A.OtherPool",
                    AttributeSource::Property {
                        context: POOL_CONTEXT.into(),
                        resource: Some("D_Resources.Health".into()),
                        property: "MaxValue".into(),
                    },
                ),
                ("A.ItemLevel", property(ITEM_CONTEXTS[0], "ExpLevel")),
                (SHOT_COST, weapon("ShotCost")),
                (
                    CRIT,
                    property(PLAYER_CONTEXTS[0], "CurrentInstantHitCriticalHitBonus"),
                ),
                (
                    MELEE,
                    property(PLAYER_CONTEXTS[1], "InstigatedMeleeDamageModifier"),
                ),
            ]
            .map(|(path, source)| (path.to_string(), source)),
        );
        s.initializations.insert(
            "Init.Damage".into(),
            Initialization {
                mode: BaseValueMode::Sets,
                rounding: Rounding::Float,
                conditional: None,
                formula: Some(Formula {
                    multiplier: constant(8.0),
                    // A leftover constant beside the attribute, as the game's data has.
                    level: ValueData {
                        attribute: Some("A.Scaler".into()),
                        ..constant(85.0)
                    },
                    power: ValueData {
                        attribute: Some("A.Level".into()),
                        ..constant(0.0)
                    },
                    offset: constant(0.0),
                }),
                minimum: None,
                maximum: None,
                unsupported: None,
            },
        );
        s.damage_types.insert(
            "D.Fire".into(),
            vec![CustomPresentation {
                attribute: None,
                presentation: Presentation {
                    base_priority: 0.15,
                    display_number: false,
                    color: Some(Rgba::new(255, 128, 0, 255)),
                    ..plain("Highly effective vs Flesh.", Rounding::IntRound)
                },
            }],
        );
        let accuracy = Presentation {
            remap: Some(Remap {
                input_min: constant(0.0),
                input_max: constant(15.0),
                output_min: constant(100.0),
                output_max: constant(0.0),
            }),
            ..plain("Accuracy", Rounding::Float)
        };
        let fire_rate = Presentation {
            inverse: true,
            ..plain("Fire Rate", Rounding::Float)
        };
        let pellets = Presentation {
            prefix: Some("x".into()),
            ..plain("Projectiles Fired", Rounding::IntFloor)
        };
        let ignite = Presentation {
            remap: Some(Remap {
                input_min: constant(0.0),
                input_max: constant(5.0),
                output_min: constant(0.0),
                output_max: constant(100.0),
            }),
            suffix: Some("%".into()),
            ..plain("Ignite Chance", Rounding::Float)
        };
        let rule = |attribute: &str, presentation: Presentation| {
            (
                attribute.to_string(),
                PresentationRule::Plain(Box::new(presentation)),
            )
        };
        s.presentations = vec![
            (
                PresentationList::Default,
                vec![
                    rule(DAMAGE, plain("Damage", Rounding::IntCeil)),
                    rule(SPREAD, accuracy),
                    rule(INTERVAL, fire_rate),
                    rule(RELOAD, plain("Reload Speed", Rounding::Float)),
                    rule(CLIP, plain("Magazine Size", Rounding::IntFloor)),
                    rule(PELLETS, pellets),
                    rule(DOT, plain("Status Effect Damage", Rounding::IntRound)),
                    rule(CAPACITY, plain("Capacity", Rounding::IntRound)),
                    rule(
                        DELAY,
                        Presentation {
                            precision: 2,
                            ..plain("Recharge Delay", Rounding::Float)
                        },
                    ),
                    rule("A.Special", plain("Special", Rounding::Float)),
                    rule("A.OtherPool", plain("Health", Rounding::Float)),
                    rule(CRIT, plain("Critical Hit Damage", Rounding::IntRound)),
                    rule(MELEE, plain("Melee Damage", Rounding::IntRound)),
                    rule(
                        SHOT_COST,
                        Presentation {
                            percentage: false,
                            plus_sign: false,
                            prefix: Some("Consumes".into()),
                            suffix: Some("ammo per shot.".into()),
                            remap: Some(Remap {
                                input_min: constant(0.0),
                                input_max: constant(1000.0),
                                output_min: constant(1.0),
                                output_max: constant(1001.0),
                            }),
                            label: None,
                            ..plain("", Rounding::IntRound)
                        },
                    ),
                ],
            ),
            (
                PresentationList::Weapon,
                vec![
                    (
                        DOT.to_string(),
                        PresentationRule::ByDamageType(vec![(
                            "D.Fire".into(),
                            plain("Burn Damage / sec.", Rounding::Float),
                        )]),
                    ),
                    (
                        CHANCE.to_string(),
                        PresentationRule::ByDamageType(vec![
                            ("D.Fire".into(), ignite.clone()),
                            (
                                "D.Slag".into(),
                                Presentation {
                                    label: Some("Slag Chance".into()),
                                    ..ignite
                                },
                            ),
                        ]),
                    ),
                ],
            ),
        ];
        db
    }

    fn weapon(parts: &[u32], manufacturer: u32) -> ItemRefs {
        let mut slots = [Some(W); PART_SLOTS];
        for (slot, &value) in slots.iter_mut().zip(parts) {
            *slot = Some(value);
        }
        ItemRefs {
            is_weapon: true,
            set: Some(0),
            item_type: Some(0),
            balance: None,
            manufacturer: Some(manufacturer),
            parts: slots,
        }
    }

    fn shield(parts: &[u32]) -> ItemRefs {
        let mut slots = [Some(I); PART_SLOTS];
        for (slot, &value) in slots.iter_mut().zip(parts) {
            *slot = Some(value);
        }
        ItemRefs {
            is_weapon: false,
            set: Some(0),
            item_type: Some(0),
            balance: None,
            manufacturer: Some(0),
            parts: slots,
        }
    }

    /// Label and printed text of each row; `?reason` for an unknown row, `~` after an
    /// approximate one.
    fn card(stats: &ItemStats) -> Vec<(String, String)> {
        assert_eq!(stats.unavailable, None);
        stats
            .rows
            .iter()
            .map(|row| {
                let text = match &row.value {
                    StatValue::Unknown { reason } => format!("?{reason}"),
                    StatValue::Computed { approximate, .. } => format!(
                        "{}{}",
                        row.text().unwrap(),
                        if approximate.is_some() { "~" } else { "" }
                    ),
                };
                (row.label.clone().unwrap_or_default(), text)
            })
            .collect()
    }

    fn texts(stats: &ItemStats) -> Vec<String> {
        card(stats).into_iter().map(|(_, text)| text).collect()
    }

    const JAKOBS: u32 = 0;
    const VLADOF: u32 = 1;

    #[test]
    fn bare_weapon_shows_its_type_values() {
        let db = synthetic();
        let stats = db.stats(&weapon(&[], VLADOF), ItemLevel::new(1));
        // 8 × 1.13 × 1.08 = 9.76 → 10; 100 × (1 − 2/15) = 86.7; 1 / 0.115 = 8.7.
        assert_eq!(
            card(&stats),
            [
                ("Damage", "10"),
                ("Accuracy", "86.7"),
                ("Fire Rate", "8.7"),
                ("Reload Speed", "3.5"),
                ("Magazine Size", "20"),
            ]
            .map(|(l, t)| (l.to_string(), t.to_string()))
        );
        assert!(stats.rows.iter().all(StatRow::is_exact));
        assert_eq!(stats.rows[0].kind(), StatKind::Other); // paths here are not the game's
        assert!(stats.texts.is_empty());
    }

    #[test]
    fn damage_follows_the_level_formula() {
        let db = synthetic();
        let damage = |level: u32| {
            db.stats(&weapon(&[], VLADOF), ItemLevel::new(level)).rows[0]
                .text()
                .unwrap()
        };
        // ceil(8 × 1.13^level × 1.08)
        assert_eq!(damage(0), "9");
        assert_eq!(damage(2), "12");
        assert_eq!(damage(10), "30");
        assert_eq!(damage(50), "3895");
        // The game stage is not used; only the grade index is.
        let level = ItemLevel {
            grade_index: Some(10),
            game_stage: Some(99),
        };
        assert_eq!(
            db.stats(&weapon(&[], VLADOF), level).rows[0]
                .text()
                .unwrap(),
            "30"
        );
    }

    #[test]
    fn modifiers_combine_as_pre_add_scale_post_add() {
        // (base + pre-adds) × (1 + positive scales) ÷ (1 + |negative scales|) + post-adds.
        let mut stack = Stack::default();
        for (kind, value) in [
            (ModifierKind::PreAdd, 2.0),
            (ModifierKind::Scale, 0.5),
            (ModifierKind::Scale, 0.25),
            (ModifierKind::Scale, -0.25),
            (ModifierKind::Scale, -0.75),
            (ModifierKind::PostAdd, 1.0),
            (ModifierKind::Scale, 0.0),
        ] {
            stack.add(kind, Ok(Num::exact(value)));
        }
        let out = stack.apply(Num::exact(10.0)).unwrap();
        assert_eq!(out.v, (10.0 + 2.0) * 1.75 / 2.0 + 1.0);
        assert!(!out.exact);
        // Scales are never netted: +0.2 and −0.2 give ×1.2 ÷ 1.2, and −1 halves.
        let mut stack = Stack::default();
        stack.add(ModifierKind::Scale, Ok(Num::exact(0.5)));
        stack.add(ModifierKind::Scale, Ok(Num::exact(-0.5)));
        assert_eq!(stack.apply(Num::exact(8.0)).unwrap().v, 8.0);
        let mut stack = Stack::default();
        stack.add(ModifierKind::Scale, Ok(Num::exact(-1.0)));
        assert_eq!(stack.apply(Num::exact(8.0)).unwrap().v, 4.0);
        // Whole-number additions keep a value exact; any scale does not.
        let mut stack = Stack::default();
        stack.add(ModifierKind::PreAdd, Ok(Num::exact(2.0)));
        assert!(stack.apply(Num::exact(6.0)).unwrap().exact);
        // A modifier that cannot be evaluated makes the attribute unknown.
        stack.add(ModifierKind::Scale, Err("why".into()));
        assert_eq!(stack.apply(Num::exact(6.0)), Err("why".into()));
    }

    #[test]
    fn parts_and_slots_modify_the_card() {
        let db = synthetic();
        // Body: +10% damage, +20% fire interval. Grip: damage slot grade 4 (+12%), spread slot
        // grade −3, held at its minimum −2 (−5% × −2 = +10%).
        let stats = db.stats(&weapon(&[0, 1], VLADOF), ItemLevel::new(1));
        // 9.7632 × 1.22 = 11.91 → 12; spread 2.2 → 85.3; 1 / 0.138 = 7.2; clip 20 (not Jakobs).
        assert_eq!(texts(&stats), ["12", "85.3", "7.2", "3.5", "20"]);
        // The same grip on a Jakobs gun adds 2 to the magazine.
        let stats = db.stats(&weapon(&[0, 1], JAKOBS), ItemLevel::new(1));
        assert_eq!(texts(&stats)[4], "22");
        // The unused slot's base value (100) never shows, and the body's external effect
        // (+500% damage) does not touch the weapon's own card.
        let stats = db.stats(&weapon(&[0], VLADOF), ItemLevel::new(1));
        assert_eq!(texts(&stats), ["11", "86.7", "7.2", "3.5", "20"]);
    }

    #[test]
    fn negative_scale_divides() {
        let db = synthetic();
        // Body +10% and fire element −20%: 9.7632 × 1.1 ÷ 1.2 = 8.95 → 9 (netting would give
        // 9.7632 ÷ 1.1 = 8.88 → 9 as well, so compare the numbers).
        let stats = db.stats(&weapon(&[0, W, W, W, W, 2], VLADOF), ItemLevel::new(30));
        let StatValue::Computed { value, .. } = stats.rows[0].value else {
            panic!("damage is unknown");
        };
        let base = 8.0 * 1.13f64.powi(30) * 1.08;
        assert_eq!(value, (base * 1.1 / 1.2).ceil());
        assert_ne!(value, (base / 1.1).ceil());
        // The barrel: −1000 scale and +1 post-add leave a magazine of 1, and 5 pellets.
        let stats = db.stats(&weapon(&[W, W, 3], VLADOF), ItemLevel::new(1));
        assert_eq!(texts(&stats), ["10 x5", "86.7", "8.7", "3.5", "1"]);
    }

    #[test]
    fn elemental_rows_depend_on_the_damage_type() {
        let db = synthetic();
        // Fire: damage per second 8 × 1.13^10 = 27.2, chance 0.3 × 1 → 6.0 of 0…5 → 0…100.
        let stats = db.stats(&weapon(&[W, W, W, W, W, 2], VLADOF), ItemLevel::new(10));
        let rows = card(&stats);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[5], ("Burn Damage / sec.".into(), "27.2".into()));
        assert_eq!(rows[6], ("Ignite Chance".into(), "6.0%".into()));
        // Slag has a chance row and no damage row.
        let stats = db.stats(&weapon(&[W, W, W, W, W, 8], VLADOF), ItemLevel::new(10));
        let rows = card(&stats);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[5], ("Slag Chance".into(), "6.0%".into()));
        // Two parts that disagree about the damage type: the rows exist but are unknown.
        let stats = db.stats(&weapon(&[W, W, W, W, W, 2, 8], VLADOF), ItemLevel::new(10));
        let rows = card(&stats);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[5].1, "?several parts set a damage type");
        assert_eq!(rows[4].1, "20");
    }

    #[test]
    fn unknown_inputs_make_rows_unknown() {
        let db = synthetic();
        // No level: damage is unknown, the rest is not.
        let stats = db.stats(&weapon(&[], VLADOF), ItemLevel::default());
        let rows = texts(&stats);
        assert_eq!(rows[0], "?the item's level is not known");
        assert_eq!(rows[1..], ["86.7", "8.7", "3.5", "20"]);
        // A part that is not in the database, or whose definition could not be read: every row.
        for part in [4, 5, 77] {
            let stats = db.stats(&weapon(&[part], VLADOF), ItemLevel::new(1));
            let rows = texts(&stats);
            assert_eq!(rows.len(), 5);
            assert!(rows.iter().all(|row| row.starts_with('?')), "{rows:?}");
        }
        assert_eq!(
            texts(&db.stats(&weapon(&[4], VLADOF), ItemLevel::new(1)))[0],
            "?part slot 0 refers to a part that is not in the database"
        );
        // An effect whose value cannot be evaluated: only the attribute it modifies.
        let stats = db.stats(&weapon(&[6], VLADOF), ItemLevel::new(1));
        let rows = texts(&stats);
        assert_eq!(
            rows[3],
            "?Unsupported is resolved by value resolver WeirdResolver, which is not modelled"
        );
        assert_eq!(rows[4], "20");
        // No manufacturer: the grip's conditional effect cannot be evaluated.
        let mut item = weapon(&[1], VLADOF);
        item.manufacturer = None;
        let rows = texts(&db.stats(&item, ItemLevel::new(1)));
        assert_eq!(rows[4], "?the item's manufacturer is not known");
        assert_eq!(rows[3], "3.5");
    }

    #[test]
    fn unavailable_items() {
        let db = synthetic();
        let reason = |item: &ItemRefs| db.stats(item, ItemLevel::new(1)).unavailable;
        let mut item = weapon(&[], VLADOF);
        item.item_type = Some(9);
        assert_eq!(
            reason(&item).as_deref(),
            Some("the item's type is not in the asset tables")
        );
        item.item_type = None;
        assert!(reason(&item).is_some());
        let mut item = weapon(&[], VLADOF);
        item.set = None;
        assert_eq!(reason(&item).as_deref(), Some("the item has no set id"));
        // A DLC item resolves only when asked to.
        item.set = Some(2);
        assert_eq!(
            reason(&item).as_deref(),
            Some("DLC items are not resolved until verified in game")
        );
        item.parts[0] = Some(32 << 11);
        let verified = db.stats_with(&item, ItemLevel::new(1), DlcResolution::Verified);
        assert_eq!(texts(&verified), ["11", "86.7", "7.2", "3.5", "20"]);
        // A usable item has a definition but no card; a placeholder has nothing.
        let mut relic = shield(&[]);
        relic.item_type = Some(1);
        assert_eq!(
            reason(&relic).as_deref(),
            Some("only weapon, shield, grenade mod, class mod and relic cards are computed")
        );
        let placeholder = ItemRefs {
            is_weapon: false,
            set: Some(255),
            item_type: Some(0),
            balance: Some(0),
            manufacturer: Some(0),
            parts: [None; PART_SLOTS],
        };
        assert_eq!(
            reason(&placeholder).as_deref(),
            Some("a placeholder item has no card")
        );
        // A weapon type read as an item (or the reverse) is not a weapon card.
        let mut wrong = weapon(&[], VLADOF);
        wrong.is_weapon = false;
        wrong.item_type = Some(1);
        assert!(reason(&wrong).is_some());
    }

    #[test]
    fn shield_rows_are_the_pool_values_the_shield_adds() {
        let db = synthetic();
        // Capacity 10 × level; delay 3.5.
        let stats = db.stats(&shield(&[]), ItemLevel::new(20));
        let rows = card(&stats);
        assert_eq!(rows[0], ("Capacity".into(), "200".into()));
        assert_eq!(rows[1], ("Recharge Delay".into(), "3.50".into()));
        // An unmodified modifier-text row is not shown; a row of another pool stays unknown.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].1, "?OtherPool belongs to another resource pool");
        // The battery: capacity grade 5 (+15%), delay grade 10 (−40%: ÷ 1.4).
        let stats = db.stats(&shield(&[0]), ItemLevel::new(20));
        let rows = texts(&stats);
        assert_eq!(rows[0], "230");
        assert_eq!(rows[1], "2.50");
        // Without a level the capacity is unknown and the delay is not.
        let rows = texts(&db.stats(&shield(&[0]), ItemLevel::default()));
        assert_eq!(rows[0], "?the item's level is not known");
        assert_eq!(rows[1], "2.50");
    }

    #[test]
    fn fixed_text_lines_come_highest_priority_first() {
        let db = synthetic();
        let stats = db.stats(
            &weapon(&[W, W, W, W, W, W, W, W, W, W, 7], VLADOF),
            ItemLevel::new(1),
        );
        let lines: Vec<&str> = stats.texts.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(lines, ["Red text", "Second"]);
        assert_eq!(
            stats.texts[0].color.map(Rgba::hex).as_deref(),
            Some("#DC4646")
        );
    }

    /// Each line's text (`?` when none), in card order.
    fn line_texts(stats: &ItemStats) -> Vec<String> {
        stats
            .lines
            .iter()
            .map(|line| line.text.clone().unwrap_or_else(|| "?".into()))
            .collect()
    }

    #[test]
    fn bonus_lines_are_relative_changes_worded_by_the_presentation() {
        let db = synthetic();
        // A fire pistol with the Hyperion barrel and the bayonet, like the posted Phobia with a
        // bayonet: crit +0.15 on a base of 2 is +7.5% → "+8%"; melee +0.5 on a base of 1 is
        // +50%; shot cost +1 remapped 0…1000 → 1…1001 is 2. Ordered by priority: the fixed
        // text (3), then the three bonus lines (0.5 each, parts before the type), then the
        // damage type's line (0.15).
        let stats = db.stats(&weapon(&[W, W, 9, W, W, 2, 10], VLADOF), ItemLevel::new(4));
        assert_eq!(
            line_texts(&stats),
            [
                "Stabby.",
                "+8% Critical Hit Damage",
                "+50% Melee Damage",
                "Consumes 2 ammo per shot.",
                "Highly effective vs Flesh.",
            ]
        );
        assert!(stats.lines.iter().all(|line| line.note.is_none()));
        assert_eq!(stats.lines[0].kind, CardLineKind::Text);
        assert_eq!(stats.lines[1].kind, CardLineKind::Bonus);
        assert_eq!(stats.lines[1].attribute.as_deref(), Some(CRIT));
        assert_eq!(
            stats.lines[4].color.map(Rgba::hex).as_deref(),
            Some("#FF8000")
        );
        assert_eq!(
            stats.lines.iter().map(|l| l.priority).collect::<Vec<_>>(),
            [3.0, 0.5, 0.5, 0.5, 0.15]
        );
        // The weapon's rows do not see the external effects: 8 × 1.13^4 × 1.08 ÷ 1.2 = 11.7.
        assert_eq!(texts(&stats)[0], "12");
        // Without the fire element there is no damage-type line, and a non-elemental gun with
        // nothing listed has no lines at all.
        let stats = db.stats(&weapon(&[W, W, 9], VLADOF), ItemLevel::new(4));
        assert_eq!(
            line_texts(&stats),
            ["+8% Critical Hit Damage", "Consumes 2 ammo per shot."]
        );
        let stats = db.stats(&weapon(&[W, 1], VLADOF), ItemLevel::new(4));
        // The type still lists the shot cost, and still adds 1 to it.
        assert_eq!(line_texts(&stats), ["Consumes 2 ammo per shot."]);
        // A bayonet alone: its text, its melee line, the type's shot cost.
        let stats = db.stats(&weapon(&[W, W, W, W, W, W, 10], VLADOF), ItemLevel::new(4));
        assert_eq!(
            line_texts(&stats),
            ["Stabby.", "+50% Melee Damage", "Consumes 2 ammo per shot."]
        );
    }

    #[test]
    fn modifier_rows_and_signs() {
        let db = synthetic();
        let card = Card {
            data: &db.stat_data,
            list: PresentationList::Weapon,
            own_contexts: WEAPON_CONTEXTS,
            pool: None,
            manufacturer: None,
            properties: HashMap::new(),
            stacks: HashMap::new(),
            grades: HashMap::new(),
            fun_slots: Vec::new(),
            poison: None,
            depth: Cell::new(0),
        };
        let number = |presentation: &Presentation, value: f32| {
            card.modifier_number(
                presentation,
                &Modifier {
                    value,
                    exact: false,
                },
            )
            .map(|(_, display, _)| display)
        };
        let percent = plain("x", Rounding::IntRound);
        assert_eq!(number(&percent, 0.075).unwrap(), "+8%");
        assert_eq!(number(&percent, -0.1935).unwrap(), "-19%");
        assert_eq!(
            number(
                &Presentation {
                    rounding: Rounding::Float,
                    precision: 2,
                    ..percent.clone()
                },
                -0.1935
            )
            .unwrap(),
            "-19.35%"
        );
        assert_eq!(
            number(
                &Presentation {
                    plus_sign: false,
                    ..percent.clone()
                },
                0.26
            )
            .unwrap(),
            "26%"
        );
        assert_eq!(
            number(
                &Presentation {
                    sign: SignStyle::OppositeSign,
                    ..percent.clone()
                },
                0.26
            )
            .unwrap(),
            "-26%"
        );
        assert_eq!(
            number(
                &Presentation {
                    sign: SignStyle::Positive,
                    ..percent.clone()
                },
                -0.26
            )
            .unwrap(),
            "+26%"
        );
        let plain_number = Presentation {
            percentage: false,
            ..plain("x", Rounding::IntFloor)
        };
        assert_eq!(number(&plain_number, 2.02).unwrap(), "+2");
        assert_eq!(number(&plain_number, -75.0).unwrap(), "-75");
        // Custom placement puts the number into the description.
        let custom = Presentation {
            percentage: false,
            plus_sign: false,
            custom_placement: true,
            label: Some("Spawns $NUMBER$ child grenades.".into()),
            ..plain("", Rounding::IntRound)
        };
        assert_eq!(Card::line_text(&custom, "8"), "Spawns 8 child grenades.");
        // Prefix, number, suffix and description are joined with single spaces.
        let wordy = Presentation {
            prefix: Some("Consumes".into()),
            suffix: Some("ammo per shot.".into()),
            label: None,
            ..plain("", Rounding::IntRound)
        };
        assert_eq!(Card::line_text(&wordy, "2"), "Consumes 2 ammo per shot.");
        assert_eq!(
            Card::line_text(&plain("Melee Damage", Rounding::IntRound), "+50%"),
            "+50% Melee Damage"
        );
    }

    #[test]
    fn rounding_and_boundaries() {
        let show = |rounding, precision, y: f32, exact| {
            let presentation = Presentation {
                precision,
                ..plain("x", rounding)
            };
            let (value, display, boundary) = rounded(&presentation, y, exact);
            (value, display, boundary)
        };
        assert_eq!(
            show(Rounding::Float, 1, 3.675, false),
            (3.7, "3.7".into(), false)
        );
        assert_eq!(show(Rounding::Float, 2, 2.239_9, false).1, "2.24");
        assert_eq!(show(Rounding::Float, 0, 2.6, false).1, "3");
        assert_eq!(
            show(Rounding::IntCeil, 1, 12.497, false),
            (13.0, "13".into(), false)
        );
        assert_eq!(
            show(Rounding::IntFloor, 1, 12.997, false),
            (12.0, "12".into(), false)
        );
        assert_eq!(
            show(Rounding::IntRound, 1, 12.4, false),
            (12.0, "12".into(), false)
        );
        // A computed value within float error of a boundary is flagged; an exact one is not.
        assert!(show(Rounding::IntFloor, 1, 21.0, false).2);
        assert!(show(Rounding::IntFloor, 1, 20.999_998, false).2);
        assert!(!show(Rounding::IntFloor, 1, 21.0, true).2);
        assert!(show(Rounding::IntCeil, 1, 13.000_001, false).2);
        assert!(show(Rounding::Float, 1, 3.65, false).2);
        assert!(show(Rounding::IntRound, 1, 12.5, false).2);
        assert!(show(Rounding::IntRound, 1, 12.5, true).2); // a true tie stays a tie
        assert!(!show(Rounding::Float, 1, 3.66, false).2);
    }

    #[test]
    fn presentation_edges() {
        let db = synthetic();
        let card = Card {
            data: &db.stat_data,
            list: PresentationList::Weapon,
            own_contexts: WEAPON_CONTEXTS,
            pool: None,
            manufacturer: None,
            properties: HashMap::new(),
            stacks: HashMap::new(),
            grades: HashMap::new(),
            fun_slots: Vec::new(),
            poison: None,
            depth: Cell::new(0),
        };
        let Some(PresentationRule::Plain(accuracy)) =
            db.stat_data.presentation(PresentationList::Weapon, SPREAD)
        else {
            panic!("no accuracy presentation");
        };
        // Spread beyond the input range is clamped, and says so.
        let (value, display, approximate) = card.present(accuracy, Num::exact(20.0)).unwrap();
        assert_eq!((value, display.as_str()), (0.0, "0.0"));
        assert!(approximate.unwrap().contains("outside"));
        // A fire interval that is not positive cannot be shown as a rate.
        let Some(PresentationRule::Plain(rate)) = db
            .stat_data
            .presentation(PresentationList::Weapon, INTERVAL)
        else {
            panic!("no fire rate presentation");
        };
        assert!(card.present(rate, Num::exact(0.0)).is_err());
        // An attribute outside the database, and one read through another object.
        assert_eq!(
            card.attribute("A.Nowhere"),
            Err("attribute Nowhere is not in the database".into())
        );
        assert!(card
            .attribute("A.ItemLevel")
            .unwrap_err()
            .contains("not part of the item"));
        assert_eq!(card.attribute("A.Scaler"), Ok(Num::exact(1.13)));
        // The override list is consulted before the default one.
        assert!(matches!(
            db.stat_data.presentation(PresentationList::Weapon, DOT),
            Some(PresentationRule::ByDamageType(_))
        ));
        assert!(matches!(
            db.stat_data.presentation(PresentationList::Shield, DOT),
            Some(PresentationRule::Plain(_))
        ));
        assert_eq!(
            db.stat_data
                .presentation(PresentationList::Shield, "A.Nowhere"),
            None
        );
    }

    #[test]
    fn initialization_modes_and_limits() {
        let mut db = synthetic();
        let formula = |multiplier: f32| Formula {
            multiplier: constant(multiplier),
            level: constant(2.0),
            power: constant(3.0),
            offset: constant(1.0),
        };
        let init = |mode, rounding, minimum: Option<f32>, maximum: Option<f32>| Initialization {
            mode,
            rounding,
            formula: Some(formula(0.5)),
            conditional: None,
            minimum: minimum.map(constant),
            maximum: maximum.map(constant),
            unsupported: None,
        };
        let s = &mut db.stat_data;
        // 0.5 × (2^3 + 1) = 4.5
        s.initializations.insert(
            "I.Sets".into(),
            init(BaseValueMode::Sets, Rounding::Float, None, None),
        );
        s.initializations.insert(
            "I.Adds".into(),
            init(BaseValueMode::Adds, Rounding::Float, None, None),
        );
        s.initializations.insert(
            "I.Scales".into(),
            init(BaseValueMode::Scales, Rounding::IntRound, None, None),
        );
        s.initializations.insert(
            "I.Offset".into(),
            init(BaseValueMode::OffsetBy, Rounding::Float, None, None),
        );
        s.initializations.insert(
            "I.Min".into(),
            init(BaseValueMode::Sets, Rounding::IntFloor, Some(6.5), None),
        );
        s.initializations.insert(
            "I.Max".into(),
            init(BaseValueMode::Sets, Rounding::IntCeil, None, Some(3.2)),
        );
        s.initializations.insert(
            "I.Conditional".into(),
            Initialization {
                formula: None,
                conditional: None,
                unsupported: Some("random variance".into()),
                ..init(BaseValueMode::Sets, Rounding::Float, None, None)
            },
        );
        s.initializations.insert(
            "I.Loop".into(),
            Initialization {
                conditional: None,
                formula: Some(Formula {
                    multiplier: ValueData {
                        initialization: Some("I.Loop".into()),
                        ..constant(1.0)
                    },
                    ..formula(1.0)
                }),
                ..init(BaseValueMode::Sets, Rounding::Float, None, None)
            },
        );
        let card = Card {
            data: &db.stat_data,
            list: PresentationList::Weapon,
            own_contexts: WEAPON_CONTEXTS,
            pool: None,
            manufacturer: None,
            properties: HashMap::new(),
            stacks: HashMap::new(),
            grades: HashMap::new(),
            fun_slots: Vec::new(),
            poison: None,
            depth: Cell::new(0),
        };
        let value = |path: &str, base: f32, scale: f32| {
            card.value(&ValueData {
                constant: base,
                attribute: None,
                initialization: Some(path.into()),
                scale,
            })
            .map(|n| n.v)
        };
        assert_eq!(value("I.Sets", 100.0, 2.0), Ok(9.0));
        assert_eq!(value("I.Adds", 100.0, 1.0), Ok(104.5));
        assert_eq!(value("I.Scales", 3.0, 1.0), Ok(14.0)); // 13.5 rounds to 14
        assert_eq!(value("I.Min", 0.0, 1.0), Ok(6.0)); // max(4.5, 6.5) floored
        assert_eq!(value("I.Max", 0.0, 1.0), Ok(4.0)); // min(4.5, 3.2) ceiled
        assert!(value("I.Offset", 0.0, 1.0).unwrap_err().contains("offsets"));
        assert!(value("I.Conditional", 0.0, 1.0)
            .unwrap_err()
            .contains("random variance"));
        assert!(value("I.Nowhere", 0.0, 1.0)
            .unwrap_err()
            .contains("not in the database"));
        assert!(value("I.Loop", 0.0, 1.0)
            .unwrap_err()
            .contains("too deeply"));
        // The depth counter is restored after a failure.
        assert_eq!(card.depth.get(), 0);
    }
}

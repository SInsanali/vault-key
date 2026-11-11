//! Extracts what item cards are computed from: the attribute effects of every type and part in
//! the asset tables, the attribute and initialisation definitions they refer to, and the
//! presentation rules for the attributes that cards show.
//!
//! Nothing here fails the build. Whatever cannot be read as expected is recorded (a
//! definition's `problems`, an attribute's `Unsupported`), and the calculation later reports
//! the numbers that depend on it as unknown.

use std::collections::BTreeSet;

use super::builder::BuildContext;
use super::object_index::Loc;
use crate::database::{
    AttributeEffect, AttributeSource, BaseValueMode, CardAttribute, CardText, Comparison,
    Condition, Conditional, ConditionalCase, CustomPresentation, Formula, Initialization,
    ModifierKind, Presentation, PresentationEntries, PresentationList, PresentationRule, Remap,
    Rgba, Rounding, SignStyle, SlotEffect, SlotUpgrade, StatDefinition, StatsData, UiStat,
    ValueData, WeaponBase,
};
use crate::package::{PropertyList, PropertyValue};

/// How far an attribute's archetype chain is followed for a property it does not set itself.
const MAX_ARCHETYPES: usize = 4;

/// The class defaults of `WeaponTypeDefinition` (`WillowGame.Default__WeaponTypeDefinition`)
/// for the properties read here: what a definition has when it does not set the property.
/// Checked against the installed game by the test `weapon_type_class_defaults`.
pub(crate) mod weapon_defaults {
    pub(crate) const CLIP_SIZE: i32 = 0;
    pub(crate) const FIRE_RATE: f32 = 0.4;
    pub(crate) const RELOAD_TIME: f32 = 2.1;
    pub(crate) const SPREAD: f32 = 0.01;
    pub(crate) const PROJECTILES_PER_SHOT: i32 = 1;
    pub(crate) const SHOT_COST: i32 = 1;
    pub(crate) const PER_SHOT_ACCURACY_IMPULSE: f32 = 0.0;
    pub(crate) const AUTOMATIC_BURST_COUNT: i32 = 0;
    pub(crate) const INSTANT_HIT_DAMAGE: f32 = 20.0;
    pub(crate) const STATUS_EFFECT_DAMAGE: f32 = 1.0;
    pub(crate) const BASE_STATUS_EFFECT_CHANCE_MODIFIER: f32 = 1.0;
}

/// The class defaults of `AttributePresentationDefinition` for the properties read here.
/// Checked by the test `presentation_class_defaults`.
pub(crate) mod presentation_defaults {
    use crate::database::Rounding;
    pub(crate) const ROUNDING: Rounding = Rounding::IntRound;
    pub(crate) const FLOAT_PRECISION: i32 = 1;
    pub(crate) const DISPLAY_AS_PERCENTAGE: bool = true;
    pub(crate) const BIGGER_IS_BETTER: bool = true;
    pub(crate) const ENABLE_TEXT_COLOR: bool = true;
    pub(crate) const INCLUDE_PREFIX: bool = true;
    pub(crate) const INCLUDE_SUFFIX: bool = true;
    /// `TextColor` as stored (B, G, R, A): white, the card's plain text colour.
    pub(crate) const TEXT_COLOR: [u8; 4] = [255, 255, 255, 255];
}

/// Extracts the stat data for the table objects `paths`, following their references.
pub(crate) fn extract_stats<'p>(
    ctx: &BuildContext<'_>,
    paths: impl Iterator<Item = &'p str>,
    globals: &PropertyList,
    globals_package: usize,
) -> StatsData {
    let mut x = Extractor {
        ctx,
        data: StatsData::default(),
        attributes: BTreeSet::new(),
        initializations: BTreeSet::new(),
        shown: BTreeSet::new(),
        damage_type_paths: BTreeSet::new(),
    };
    let mut paths: Vec<&str> = paths.collect();
    paths.sort_unstable();
    for path in paths {
        let Some(loc) = ctx.index.locate(path) else {
            continue;
        };
        if let Some(definition) = x.definition(loc) {
            x.data.definitions.insert(path.to_string(), definition);
        }
    }
    for list in PresentationList::ALL {
        if let Some(entries) = x.presentation_list(globals, globals_package, list) {
            x.data.presentations.push((list, entries));
        }
    }
    x.follow_references();
    x.damage_types();
    x.data
}

struct Extractor<'c, 'a> {
    ctx: &'c BuildContext<'a>,
    data: StatsData,
    /// Attributes used as values somewhere, whose definitions are still to be read.
    attributes: BTreeSet<String>,
    initializations: BTreeSet<String>,
    /// Attributes some `UIStatList` shows or some card line is about; presentation lists are
    /// filtered to these.
    shown: BTreeSet<String>,
    /// Damage types the definitions name, whose card lines are read at the end.
    damage_type_paths: BTreeSet<String>,
}

/// The value of an enum property, or `default` when the property is absent.
fn enum_or<'v>(props: &'v PropertyList, name: &str, default: &'v str) -> Result<&'v str, String> {
    match props.get(name) {
        None => Ok(default),
        Some(PropertyValue::EnumName(value)) => Ok(value),
        Some(other) => Err(format!("{name} is {other:?}")),
    }
}

fn bool_or(props: &PropertyList, name: &str, default: bool) -> Result<bool, String> {
    match props.get(name) {
        None => Ok(default),
        Some(PropertyValue::Bool(value)) => Ok(*value),
        Some(other) => Err(format!("{name} is {other:?}")),
    }
}

fn int_or(props: &PropertyList, name: &str, default: i32) -> Result<i32, String> {
    match props.get(name) {
        None => Ok(default),
        Some(PropertyValue::Int(value)) => Ok(*value),
        Some(other) => Err(format!("{name} is {other:?}")),
    }
}

fn float_or(props: &PropertyList, name: &str, default: f32) -> Result<f32, String> {
    match props.get(name) {
        None => Ok(default),
        Some(PropertyValue::Float(value)) => Ok(*value),
        Some(other) => Err(format!("{name} is {other:?}")),
    }
}

fn name_or<'v>(props: &'v PropertyList, name: &str, default: &'v str) -> Result<&'v str, String> {
    match props.get(name) {
        None => Ok(default),
        Some(PropertyValue::Name(value)) => Ok(value),
        Some(other) => Err(format!("{name} is {other:?}")),
    }
}

/// A struct property's fields; `None` when the property is absent.
fn struct_or_none<'v>(
    props: &'v PropertyList,
    name: &str,
) -> Result<Option<&'v PropertyList>, String> {
    match props.get(name) {
        None => Ok(None),
        Some(PropertyValue::Struct(fields)) => Ok(Some(fields)),
        Some(other) => Err(format!("{name} is {other:?}")),
    }
}

fn modifier_kind(props: &PropertyList) -> Result<ModifierKind, String> {
    match enum_or(props, "ModifierType", "MT_Scale")? {
        "MT_Scale" => Ok(ModifierKind::Scale),
        "MT_PreAdd" => Ok(ModifierKind::PreAdd),
        "MT_PostAdd" => Ok(ModifierKind::PostAdd),
        other => Err(format!("ModifierType {other}")),
    }
}

fn rounding(props: &PropertyList, default: Rounding) -> Result<Rounding, String> {
    match props.get("RoundingMode") {
        None => Ok(default),
        Some(PropertyValue::EnumName(name)) => match name.as_str() {
            "ATTRROUNDING_Float" => Ok(Rounding::Float),
            "ATTRROUNDING_IntRound" => Ok(Rounding::IntRound),
            "ATTRROUNDING_IntFloor" => Ok(Rounding::IntFloor),
            "ATTRROUNDING_IntCeil" => Ok(Rounding::IntCeil),
            other => Err(format!("RoundingMode {other}")),
        },
        Some(other) => Err(format!("RoundingMode is {other:?}")),
    }
}

fn sign_style(props: &PropertyList) -> Result<SignStyle, String> {
    match enum_or(props, "SignStyle", "SIGNSTYLE_AsIs")? {
        "SIGNSTYLE_AsIs" => Ok(SignStyle::AsIs),
        "SIGNSTYLE_Positive" => Ok(SignStyle::Positive),
        "SIGNSTYLE_Negative" => Ok(SignStyle::Negative),
        "SIGNSTYLE_AsIsAnyStyle" => Ok(SignStyle::AsIsAnyStyle),
        "SIGNSTYLE_OppositeSign" => Ok(SignStyle::OppositeSign),
        other => Err(format!("SignStyle {other}")),
    }
}

fn comparison(name: &str) -> Result<Comparison, String> {
    match name {
        "OPERATOR_EqualTo" => Ok(Comparison::EqualTo),
        "OPERATOR_NotEqualTo" => Ok(Comparison::NotEqualTo),
        "OPERATOR_LessThan" => Ok(Comparison::LessThan),
        "OPERATOR_LessThanOrEqual" => Ok(Comparison::LessThanOrEqual),
        "OPERATOR_GreaterThan" => Ok(Comparison::GreaterThan),
        "OPERATOR_GreaterThanOrEqual" => Ok(Comparison::GreaterThanOrEqual),
        other => Err(format!("ComparisonOperator {other}")),
    }
}

/// Removes the card's `[tag]` and `[-tag]` markup.
fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let tag = &rest[open + 1..open + close];
        let is_tag = !tag.is_empty()
            && tag
                .trim_start_matches('-')
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        out.push_str(&rest[..open]);
        if !is_tag {
            out.push_str(&rest[open..=open + close]);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    // HTML-style tags (`<font color="#FFFFFF">`) go the same way.
    let mut plain = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(open) = rest.find('<') {
        let tag_like = |body: &str| body.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/');
        match rest[open..].find('>') {
            Some(close) if tag_like(&rest[open + 1..open + close]) => {
                plain.push_str(&rest[..open]);
                rest = &rest[open + close + 1..];
            }
            _ => {
                plain.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    plain.push_str(rest);
    // Collapse the runs of spaces that designers used for line breaks.
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Extractor<'_, '_> {
    /// The path an object property refers to; `None` for an absent or null reference.
    fn reference(
        &self,
        props: &PropertyList,
        name: &str,
        package: usize,
    ) -> Result<Option<String>, String> {
        match props.get(name) {
            None | Some(PropertyValue::Object(0)) => Ok(None),
            Some(PropertyValue::Object(reference)) => self.ctx.packages[package]
                .path_of(*reference)
                .map(Some)
                .map_err(|e| format!("{name}: {e}")),
            Some(other) => Err(format!("{name} is {other:?}")),
        }
    }

    /// An attribute reference that will be evaluated: its definition is queued for reading.
    fn attribute_value(
        &mut self,
        props: &PropertyList,
        name: &str,
        package: usize,
    ) -> Result<Option<String>, String> {
        let path = self.reference(props, name, package)?;
        if let Some(path) = &path {
            self.attributes.insert(path.clone());
        }
        Ok(path)
    }

    /// Reads an `AttributeInitializationData` struct. Fields the struct does not set come from
    /// `defaults` (the class default's struct for a top-level property, the struct's own
    /// defaults inside an array).
    fn value_data(
        &mut self,
        fields: Option<&PropertyList>,
        package: usize,
        defaults: ValueData,
    ) -> Result<ValueData, String> {
        let Some(fields) = fields else {
            return Ok(defaults);
        };
        let attribute = match fields.get("BaseValueAttribute") {
            None => defaults.attribute,
            Some(_) => self.attribute_value(fields, "BaseValueAttribute", package)?,
        };
        let initialization = match fields.get("InitializationDefinition") {
            None => defaults.initialization,
            Some(_) => {
                let path = self.reference(fields, "InitializationDefinition", package)?;
                if let Some(path) = &path {
                    self.initializations.insert(path.clone());
                }
                path
            }
        };
        Ok(ValueData {
            constant: float_or(fields, "BaseValueConstant", defaults.constant)?,
            attribute,
            initialization,
            scale: float_or(fields, "BaseValueScaleConstant", defaults.scale)?,
        })
    }

    /// A struct property holding an `AttributeInitializationData`.
    fn value_property(
        &mut self,
        props: &PropertyList,
        name: &str,
        package: usize,
        default_constant: f32,
    ) -> Result<ValueData, String> {
        let fields = struct_or_none(props, name)?;
        self.value_data(fields, package, ValueData::constant(default_constant))
            .map_err(|e| format!("{name}: {e}"))
    }

    /// The elements of a struct array property; empty when the property is absent.
    fn struct_array(
        &self,
        props: &PropertyList,
        name: &str,
        package: usize,
    ) -> Result<Vec<PropertyList>, String> {
        match props.get(name) {
            None => Ok(Vec::new()),
            Some(value) => self.ctx.packages[package]
                .struct_array(value)
                .map_err(|e| format!("{name}: {e}")),
        }
    }

    fn effects(
        &mut self,
        props: &PropertyList,
        name: &str,
        package: usize,
    ) -> Result<Vec<AttributeEffect>, String> {
        let mut out = Vec::new();
        for (i, item) in self.struct_array(props, name, package)?.iter().enumerate() {
            let what = |e: String| format!("{name}[{i}]: {e}");
            // An entry that names no attribute modifies nothing.
            let Some(attribute) = self
                .reference(item, "AttributeToModify", package)
                .map_err(what)?
            else {
                continue;
            };
            let fields = struct_or_none(item, "BaseModifierValue").map_err(what)?;
            out.push(AttributeEffect {
                attribute,
                kind: modifier_kind(item).map_err(what)?,
                value: self
                    .value_data(fields, package, ValueData::default())
                    .map_err(what)?,
            });
        }
        Ok(out)
    }

    fn slot_effects(
        &mut self,
        props: &PropertyList,
        package: usize,
    ) -> Result<Vec<SlotEffect>, String> {
        let name = "AttributeSlotEffects";
        let mut out = Vec::new();
        for (i, item) in self.struct_array(props, name, package)?.iter().enumerate() {
            let what = |e: String| format!("{name}[{i}]: {e}");
            let Some(attribute) = self
                .reference(item, "AttributeToModify", package)
                .map_err(what)?
            else {
                continue;
            };
            let base = struct_or_none(item, "BaseModifierValue").map_err(what)?;
            let per_grade = struct_or_none(item, "PerGradeUpgrade").map_err(what)?;
            // The struct's defaults enforce a minimum grade of 0 and no maximum.
            let minimum = bool_or(item, "bEnforceMinimumGrade", true).map_err(what)?;
            let maximum = bool_or(item, "bEnforceMaximumGrade", false).map_err(what)?;
            out.push(SlotEffect {
                slot: name_or(item, "SlotName", "None").map_err(what)?.to_string(),
                external: bool_or(item, "bExternalSlot", false).map_err(what)?,
                attribute,
                kind: modifier_kind(item).map_err(what)?,
                base: self
                    .value_data(base, package, ValueData::default())
                    .map_err(what)?,
                per_grade: self
                    .value_data(per_grade, package, ValueData::default())
                    .map_err(what)?,
                minimum_grade: if minimum {
                    Some(int_or(item, "MinimumGrade", 0).map_err(what)?)
                } else {
                    None
                },
                maximum_grade: if maximum {
                    Some(int_or(item, "MaximumGrade", 0).map_err(what)?)
                } else {
                    None
                },
                fun_stats: bool_or(item, "bIncludeInFunStats", false).map_err(what)?,
            });
            if let Some(slot) = out.last().filter(|slot| slot.fun_stats) {
                self.shown.insert(slot.attribute.clone());
                self.attributes.insert(slot.attribute.clone());
            }
        }
        Ok(out)
    }

    fn card_attributes(
        &mut self,
        props: &PropertyList,
        package: usize,
    ) -> Result<Vec<CardAttribute>, String> {
        let name = "WeaponCardAttributes";
        let mut out = Vec::new();
        for (i, item) in self.struct_array(props, name, package)?.iter().enumerate() {
            let what = |e: String| format!("{name}[{i}]: {e}");
            let Some(attribute) = self.reference(item, "Attribute", package).map_err(what)? else {
                continue;
            };
            // The line needs the attribute's definition (for its base) and its presentation.
            self.shown.insert(attribute.clone());
            self.attributes.insert(attribute.clone());
            out.push(CardAttribute {
                attribute,
                priority_increase: float_or(item, "PriorityIncrease", 0.0).map_err(what)?,
            });
        }
        Ok(out)
    }

    /// Every entry of an object-array property of presentations (`CustomPresentations`,
    /// `WeaponCardPresentations`), read in full.
    fn custom_presentations(
        &mut self,
        props: &PropertyList,
        name: &str,
        package: usize,
    ) -> Result<Vec<CustomPresentation>, String> {
        let Some(value) = props.get(name) else {
            return Ok(Vec::new());
        };
        let references = self.ctx.packages[package]
            .object_array(value)
            .map_err(|e| format!("{name}: {e}"))?;
        let mut out = Vec::new();
        for reference in references {
            if reference == 0 {
                continue;
            }
            let loc = self
                .ctx
                .resolve(reference, package)
                .ok_or_else(|| format!("{name}: an entry is not in the packages"))?;
            let entry = self
                .ctx
                .properties(loc)
                .map_err(|e| format!("{name}: {e}"))?;
            let attribute = self.attribute_value(&entry, "Attribute", loc.package)?;
            if let Some(attribute) = &attribute {
                self.shown.insert(attribute.clone());
            }
            out.push(CustomPresentation {
                attribute,
                presentation: self.presentation(&entry, loc.package)?,
            });
        }
        Ok(out)
    }

    /// The `WeaponCardPresentations` of every damage type the definitions name.
    fn damage_types(&mut self) {
        let paths: Vec<String> = std::mem::take(&mut self.damage_type_paths)
            .into_iter()
            .collect();
        for path in paths {
            let Some(loc) = self.ctx.index.locate(&path) else {
                continue;
            };
            let Ok(props) = self.ctx.properties(loc) else {
                continue;
            };
            if let Ok(lines) =
                self.custom_presentations(&props, "WeaponCardPresentations", loc.package)
            {
                self.data.damage_types.insert(path, lines);
            }
        }
        // The lines may name attributes and definitions that are new.
        self.follow_references();
    }

    fn slot_upgrades(
        &self,
        props: &PropertyList,
        package: usize,
    ) -> Result<Vec<SlotUpgrade>, String> {
        let name = "AttributeSlotUpgrades";
        self.struct_array(props, name, package)?
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let what = |e: String| format!("{name}[{i}]: {e}");
                Ok(SlotUpgrade {
                    slot: name_or(item, "SlotName", "None").map_err(what)?.to_string(),
                    grade_increase: int_or(item, "GradeIncrease", 0).map_err(what)?,
                    activates: bool_or(item, "bActivateSlot", false).map_err(what)?,
                })
            })
            .collect()
    }

    fn ui_stats(&mut self, props: &PropertyList, package: usize) -> Result<Vec<UiStat>, String> {
        let name = "UIStatList";
        let mut out = Vec::new();
        for (i, item) in self.struct_array(props, name, package)?.iter().enumerate() {
            let what = |e: String| format!("{name}[{i}]: {e}");
            let attribute = self
                .attribute_value(item, "Attribute", package)
                .map_err(what)?;
            let supplemental = self
                .attribute_value(item, "SupplementalAttributeToAppend", package)
                .map_err(what)?;
            let condition =
                match struct_or_none(item, "SupplementalAttributeExpression").map_err(what)? {
                    None => None,
                    Some(expression) => self.condition(expression, package).map_err(what)?,
                };
            for shown in attribute.iter().chain(&supplemental) {
                self.shown.insert(shown.clone());
            }
            let style = enum_or(item, "AttributeStyle", "ATTRSTYLE_JustNumber").map_err(what)?;
            out.push(UiStat {
                attribute,
                supplemental,
                condition,
                just_number: style == "ATTRSTYLE_JustNumber"
                    && !bool_or(item, "bDisplayAsModifierOnly", false).map_err(what)?,
            });
        }
        Ok(out)
    }

    /// An `AttributeExpressionData`; `None` when it names no first operand.
    fn condition(
        &mut self,
        expression: &PropertyList,
        package: usize,
    ) -> Result<Option<Condition>, String> {
        let Some(attribute) = self.attribute_value(expression, "AttributeOperand1", package)?
        else {
            return Ok(None);
        };
        let usage = enum_or(expression, "Operand2Usage", "OPERAND_PreferAttribute")?;
        if usage != "OPERAND_PreferAttribute" {
            return Err(format!("Operand2Usage {usage}"));
        }
        Ok(Some(Condition {
            attribute,
            comparison: comparison(enum_or(
                expression,
                "ComparisonOperator",
                "OPERATOR_EqualTo",
            )?)?,
            other_attribute: self.attribute_value(expression, "AttributeOperand2", package)?,
            constant: float_or(expression, "ConstantOperand2", 0.0)?,
        }))
    }

    /// The fixed text lines among a definition's `CustomPresentations`, and how many entries
    /// are something else.
    fn texts(&self, props: &PropertyList, package: usize) -> Result<(Vec<CardText>, u32), String> {
        let Some(value) = props.get("CustomPresentations") else {
            return Ok((Vec::new(), 0));
        };
        let references = self.ctx.packages[package]
            .object_array(value)
            .map_err(|e| format!("CustomPresentations: {e}"))?;
        let mut texts = Vec::new();
        let mut omitted = 0;
        for reference in references {
            if reference == 0 {
                continue;
            }
            match self.fixed_text(reference, package) {
                Some(text) => texts.push(text),
                None => omitted += 1,
            }
        }
        Ok((texts, omitted))
    }

    /// A presentation that shows only text: no attribute, no number, and text that needs no
    /// localisation lookup.
    fn fixed_text(&self, reference: i32, package: usize) -> Option<CardText> {
        let loc = self.ctx.resolve(reference, package)?;
        let props = self.ctx.properties(loc).ok()?;
        if !matches!(
            props.get("Attribute"),
            None | Some(PropertyValue::Object(0))
        ) || props.bool("bDontDisplayNumber") != Some(true)
        {
            return None;
        }
        let raw = props
            .string("Description")
            .filter(|s| !s.is_empty())
            .or_else(|| props.string("NoConstraintText").filter(|s| !s.is_empty()))?;
        if raw.starts_with('$') {
            return None;
        }
        let color = match props.get("TextColor") {
            Some(PropertyValue::StructRaw { name, bytes }) if name == "Color" => {
                let [b, g, r, a] = <[u8; 4]>::try_from(bytes.as_slice()).ok()?;
                Some(Rgba::new(r, g, b, a))
            }
            _ => None,
        };
        Some(CardText {
            text: strip_markup(raw),
            color,
            priority: props.float("BasePriority").unwrap_or(0.0),
        })
    }

    /// The stat definition of a type or part. `None` for classes that carry none (balances,
    /// manufacturers).
    fn definition(&mut self, loc: Loc) -> Option<StatDefinition> {
        let class = self.ctx.class_name(loc);
        let is_weapon_type = class == "WeaponTypeDefinition";
        if class.ends_with("BalanceDefinition") || class == "ManufacturerDefinition" {
            return None;
        }
        let mut definition = StatDefinition::default();
        let props = match self.ctx.properties(loc) {
            Ok(props) => props,
            Err(e) => {
                definition.problems.push(e.to_string());
                return Some(definition);
            }
        };
        let package = loc.package;
        if self.has_archetype(loc) {
            definition
                .problems
                .push("the definition inherits from an archetype".into());
        }
        let mut problems = Vec::new();
        let mut keep = |result: Result<(), String>| {
            if let Err(e) = result {
                problems.push(e);
            }
        };
        let internal = if props.get("WeaponAttributeEffects").is_some() {
            "WeaponAttributeEffects"
        } else {
            "ItemAttributeEffects"
        };
        keep(
            self.effects(&props, internal, package)
                .map(|v| definition.effects = v),
        );
        keep(
            self.effects(&props, "ExternalAttributeEffects", package)
                .map(|v| definition.external_effects = v),
        );
        keep(
            self.slot_effects(&props, package)
                .map(|v| definition.slot_effects = v),
        );
        keep(
            self.slot_upgrades(&props, package)
                .map(|v| definition.slot_upgrades = v),
        );
        keep(
            self.ui_stats(&props, package)
                .map(|v| definition.ui_stats = v),
        );
        keep(
            self.reference(&props, "CustomDamageTypeDefinition", package)
                .map(|v| definition.damage_type = v),
        );
        keep(self.texts(&props, package).map(|(texts, omitted)| {
            definition.texts = texts;
            definition.texts_omitted = omitted;
        }));
        keep(
            self.card_attributes(&props, package)
                .map(|v| definition.card_attributes = v),
        );
        keep(
            self.custom_presentations(&props, "CustomPresentations", package)
                .map(|v| definition.presentations = v),
        );
        if let Some(damage_type) = &definition.damage_type {
            self.damage_type_paths.insert(damage_type.clone());
        }
        if is_weapon_type {
            keep(
                self.weapon_base(&props, package)
                    .map(|v| definition.weapon = Some(v)),
            );
            let default_type = definition
                .weapon
                .as_ref()
                .and_then(|w| w.damage_type.clone());
            if let Some(damage_type) = default_type {
                self.damage_type_paths.insert(damage_type);
            }
        }
        definition.problems.append(&mut problems);
        Some(definition)
    }

    fn weapon_base(&mut self, props: &PropertyList, package: usize) -> Result<WeaponBase, String> {
        use weapon_defaults as d;
        Ok(WeaponBase {
            clip_size: int_or(props, "ClipSize", d::CLIP_SIZE)?,
            fire_interval: float_or(props, "FireRate", d::FIRE_RATE)?,
            reload_time: float_or(props, "ReloadTime", d::RELOAD_TIME)?,
            spread: float_or(props, "Spread", d::SPREAD)?,
            projectiles_per_shot: int_or(props, "ProjectilesPerShot", d::PROJECTILES_PER_SHOT)?,
            shot_cost: int_or(props, "ShotCost", d::SHOT_COST)?,
            per_shot_accuracy_impulse: float_or(
                props,
                "PerShotAccuracyImpulse",
                d::PER_SHOT_ACCURACY_IMPULSE,
            )?,
            automatic_burst_count: int_or(props, "AutomaticBurstCount", d::AUTOMATIC_BURST_COUNT)?,
            damage: self.value_property(
                props,
                "InstantHitDamage",
                package,
                d::INSTANT_HIT_DAMAGE,
            )?,
            status_effect_damage: self.value_property(
                props,
                "StatusEffectDamage",
                package,
                d::STATUS_EFFECT_DAMAGE,
            )?,
            base_status_effect_chance: self.value_property(
                props,
                "BaseStatusEffectChanceModifier",
                package,
                d::BASE_STATUS_EFFECT_CHANCE_MODIFIER,
            )?,
            damage_type: self.reference(props, "DefaultDamageTypeDefinition", package)?,
        })
    }

    /// Whether the export's archetype is a real object and not its class's default object.
    fn has_archetype(&self, loc: Loc) -> bool {
        self.archetype(loc).is_some()
    }

    /// The export's archetype, unless it is null or a class default object.
    fn archetype(&self, loc: Loc) -> Option<Loc> {
        let pkg = &self.ctx.packages[loc.package];
        let reference = pkg.exports.get(loc.export)?.archetype;
        if reference == 0 {
            return None;
        }
        let path = pkg.path_of(reference).ok()?;
        if path.rsplit('.').next()?.starts_with("Default__") {
            return None;
        }
        self.ctx.resolve(reference, loc.package)
    }

    /// The entries of one of the globals' presentation lists for the attributes that cards
    /// show. `None` when the globals do not name the list or it cannot be read.
    fn presentation_list(
        &mut self,
        globals: &PropertyList,
        globals_package: usize,
        list: PresentationList,
    ) -> Option<PresentationEntries> {
        let reference = globals.object(list.globals_property())?;
        let loc = self.ctx.resolve(reference, globals_package)?;
        let props = self.ctx.properties(loc).ok()?;
        let references = match props.get("Attributes") {
            None => Vec::new(),
            Some(value) => self.ctx.packages[loc.package].object_array(value).ok()?,
        };
        let mut entries = Vec::new();
        for reference in references {
            let Some(entry) = self.ctx.resolve(reference, loc.package) else {
                continue;
            };
            let Ok(props) = self.ctx.properties(entry) else {
                continue;
            };
            let Ok(Some(attribute)) = self.reference(&props, "Attribute", entry.package) else {
                continue;
            };
            if !self.shown.contains(&attribute) {
                continue;
            }
            let rule = if self.ctx.class_name(entry)
                == "WeaponStatusEffectAttributePresentationDefinition"
            {
                self.damage_type_rule(&props, entry.package)
            } else {
                self.presentation(&props, entry.package)
                    .map(|presentation| PresentationRule::Plain(Box::new(presentation)))
            };
            // A rule that cannot be read is left out, so its rows are reported as having no
            // presentation instead of being shown wrongly.
            if let Ok(rule) = rule {
                entries.push((attribute, rule));
            }
        }
        Some(entries)
    }

    fn damage_type_rule(
        &mut self,
        props: &PropertyList,
        package: usize,
    ) -> Result<PresentationRule, String> {
        let mut out = Vec::new();
        for item in self.struct_array(props, "MappingData", package)? {
            let (Some(damage_type), Some(reference)) = (
                self.reference(&item, "DamageType", package)?,
                item.object("Presentation").filter(|&r| r != 0),
            ) else {
                continue;
            };
            let loc = self
                .ctx
                .resolve(reference, package)
                .ok_or("MappingData presentation not found")?;
            let mapped = self.ctx.properties(loc).map_err(|e| e.to_string())?;
            out.push((damage_type, self.presentation(&mapped, loc.package)?));
        }
        Ok(PresentationRule::ByDamageType(out))
    }

    fn presentation(
        &mut self,
        props: &PropertyList,
        package: usize,
    ) -> Result<Presentation, String> {
        let text = |name: &str| {
            props
                .string(name)
                .map(strip_markup)
                .filter(|s| !s.is_empty())
        };
        let remap = if bool_or(props, "bValueRemappingEnabled", false)? {
            let fields = struct_or_none(props, "RemappingData")?;
            let mut bound = |name: &str| -> Result<ValueData, String> {
                let inner = match fields {
                    None => None,
                    Some(fields) => struct_or_none(fields, name)?,
                };
                self.value_data(inner, package, ValueData::default())
            };
            Some(Remap {
                input_min: bound("InputValueMn")?,
                input_max: bound("InputValueMx")?,
                output_min: bound("OutputValueMn")?,
                output_max: bound("OutputValueMx")?,
            })
        } else {
            None
        };
        use presentation_defaults as d;
        let stored = match props.get("TextColor") {
            None => d::TEXT_COLOR,
            Some(PropertyValue::StructRaw { name, bytes }) if name == "Color" => {
                <[u8; 4]>::try_from(bytes.as_slice()).map_err(|_| "TextColor size".to_string())?
            }
            Some(other) => return Err(format!("TextColor is {other:?}")),
        };
        let [b, g, r, a] = stored;
        let colored = bool_or(props, "bEnableTextColor", d::ENABLE_TEXT_COLOR)?;
        let color = (colored && stored != d::TEXT_COLOR).then_some(Rgba::new(r, g, b, a));
        Ok(Presentation {
            label: text("Description"),
            no_constraint_text: text("NoConstraintText"),
            base_priority: float_or(props, "BasePriority", 0.0)?,
            percentage: bool_or(props, "bDisplayAsPercentage", d::DISPLAY_AS_PERCENTAGE)?,
            percent_as_float: bool_or(props, "bDisplayPercentAsFloat", false)?,
            plus_sign: !bool_or(props, "bDontDisplayPlusSign", false)?,
            display_number: !bool_or(props, "bDontDisplayNumber", false)?,
            custom_placement: bool_or(props, "bUseCustomNumberPlacement", false)?,
            bigger_is_better: bool_or(props, "bBiggerIsBetter", d::BIGGER_IS_BETTER)?,
            sign: sign_style(props)?,
            color,
            include_prefix: bool_or(props, "bIncludePrefixInTranslation", d::INCLUDE_PREFIX)?,
            include_suffix: bool_or(props, "bIncludeSuffixInTranslation", d::INCLUDE_SUFFIX)?,
            inverse: bool_or(props, "bDisplayAsInverse", false)?,
            remap,
            rounding: rounding(props, d::ROUNDING)?,
            precision: int_or(props, "FloatPrecision", d::FLOAT_PRECISION)?,
            prefix: text("Prefix"),
            suffix: text("Suffix"),
        })
    }

    /// Reads the definitions of every queued attribute and initialisation definition, and of
    /// those they refer to in turn.
    fn follow_references(&mut self) {
        loop {
            let attributes: Vec<String> = std::mem::take(&mut self.attributes)
                .into_iter()
                .filter(|path| !self.data.attributes.contains_key(path))
                .collect();
            let initializations: Vec<String> = std::mem::take(&mut self.initializations)
                .into_iter()
                .filter(|path| !self.data.initializations.contains_key(path))
                .collect();
            if attributes.is_empty() && initializations.is_empty() {
                break;
            }
            for path in attributes {
                // An attribute no package defines stays absent: its value is unknown.
                if let Some(loc) = self.ctx.index.locate(&path) {
                    let source = self
                        .attribute_source(loc)
                        .unwrap_or_else(AttributeSource::Unsupported);
                    self.data.attributes.insert(path, source);
                }
            }
            for path in initializations {
                if let Some(loc) = self.ctx.index.locate(&path) {
                    let initialization =
                        self.initialization(loc).unwrap_or_else(|e| Initialization {
                            mode: BaseValueMode::Sets,
                            rounding: Rounding::Float,
                            formula: None,
                            conditional: None,
                            minimum: None,
                            maximum: None,
                            unsupported: Some(e),
                        });
                    self.data.initializations.insert(path, initialization);
                }
            }
        }
    }

    /// The object references of an array property of `loc`, looked up through the archetype
    /// chain when the object does not set the property itself.
    fn inherited_objects(&self, loc: Loc, name: &str) -> Result<Vec<Loc>, String> {
        let mut current = loc;
        for _ in 0..=MAX_ARCHETYPES {
            let props = self.ctx.properties(current).map_err(|e| e.to_string())?;
            if let Some(value) = props.get(name) {
                return self.ctx.packages[current.package]
                    .object_array(value)
                    .map_err(|e| format!("{name}: {e}"))?
                    .into_iter()
                    .map(|r| {
                        self.ctx
                            .resolve(r, current.package)
                            .ok_or_else(|| format!("{name}: an entry is not in the packages"))
                    })
                    .collect();
            }
            match self.archetype(current) {
                Some(next) => current = next,
                None => return Ok(Vec::new()),
            }
        }
        Err(format!("{name}: archetype chain too long"))
    }

    fn attribute_source(&mut self, loc: Loc) -> Result<AttributeSource, String> {
        let class = self.ctx.class_name(loc);
        if !class.ends_with("AttributeDefinition") {
            return Err(format!("class {class}"));
        }
        let resolvers = self.inherited_objects(loc, "ValueResolverChain")?;
        let [resolver] = resolvers[..] else {
            return Err(format!("{} value resolvers", resolvers.len()));
        };
        let props = self.ctx.properties(resolver).map_err(|e| e.to_string())?;
        let package = resolver.package;
        match self.ctx.class_name(resolver) {
            // `ConstantValue` has no class default, so an absent one is 0.
            "ConstantAttributeValueResolver" => Ok(AttributeSource::Constant(float_or(
                &props,
                "ConstantValue",
                0.0,
            )?)),
            "ObjectPropertyAttributeValueResolver" => {
                let contexts = self.inherited_objects(loc, "ContextResolverChain")?;
                let (context, resource) = match contexts.first() {
                    None => (String::new(), None),
                    Some(&first) => {
                        let context_props =
                            self.ctx.properties(first).map_err(|e| e.to_string())?;
                        (
                            self.ctx.class_name(first).to_string(),
                            self.reference(&context_props, "Resource", first.package)?,
                        )
                    }
                };
                Ok(AttributeSource::Property {
                    context,
                    resource,
                    property: name_or(&props, "PropertyName", "None")?.to_string(),
                })
            }
            "ManufacturerAttributeValueResolver" => {
                let mut cases = Vec::new();
                for item in self.struct_array(&props, "Manufacturers", package)? {
                    let fields = struct_or_none(&item, "ValueIfMatched")?;
                    cases.push((
                        name_or(&item, "AssociatedManufacturerName", "None")?.to_string(),
                        self.value_data(fields, package, ValueData::default())?,
                    ));
                }
                Ok(AttributeSource::Manufacturer {
                    cases,
                    otherwise: self.value_property(&props, "ValueIfNotMatched", package, 0.0)?,
                })
            }
            "SimpleMathValueResolver" => {
                // Both enums must be stated: their defaults are not known here.
                let option = enum_or(&props, "Arg1Option", "")?;
                let operand = enum_or(&props, "Operand", "")?;
                if option != "MATHRESOLVEARG1_FromAttribute" || operand != "MATHRESOLVEROPERAND_Mul"
                {
                    return Err(format!("SimpleMathValueResolver {option:?} {operand:?}"));
                }
                Ok(AttributeSource::Product(
                    self.value_property(&props, "Arg1Attribute", package, 0.0)?,
                    self.value_property(&props, "Argument", package, 0.0)?,
                ))
            }
            "AttributeSlotEffectAttributeValueResolver" => Ok(AttributeSource::SlotGrade(
                name_or(&props, "SlotName", "None")?.to_string(),
            )),
            other => Err(format!("value resolver {other}")),
        }
    }

    fn initialization(&mut self, loc: Loc) -> Result<Initialization, String> {
        if self.ctx.class_name(loc) != "AttributeInitializationDefinition" {
            return Err(format!("class {}", self.ctx.class_name(loc)));
        }
        if self.has_archetype(loc) {
            return Err("the definition inherits from an archetype".into());
        }
        let props = self.ctx.properties(loc).map_err(|e| e.to_string())?;
        let package = loc.package;
        let mode = match enum_or(
            &props,
            "BaseValueMode",
            "BASEVALUE_InitializationDefSetsBaseValue",
        )? {
            "BASEVALUE_InitializationDefSetsBaseValue" => BaseValueMode::Sets,
            "BASEVALUE_InitializationDefAddsToBaseValue" => BaseValueMode::Adds,
            "BASEVALUE_InitializationDefScalesBaseValue" => BaseValueMode::Scales,
            "BASEVALUE_InitializationDefOffsetByBaseValue" => BaseValueMode::OffsetBy,
            other => return Err(format!("BaseValueMode {other}")),
        };
        let enabled = |fields: Option<&PropertyList>, flag: &str| -> Result<bool, String> {
            fields.map_or(Ok(false), |f| bool_or(f, flag, false))
        };
        let formula_fields = struct_or_none(&props, "ValueFormula")?;
        let formula = if enabled(formula_fields, "bEnabled")? {
            let mut part = |name: &str| -> Result<ValueData, String> {
                let inner = match formula_fields {
                    None => None,
                    Some(fields) => struct_or_none(fields, name)?,
                };
                self.value_data(inner, package, ValueData::default())
            };
            Some(Formula {
                multiplier: part("Multiplier")?,
                level: part("Level")?,
                power: part("Power")?,
                offset: part("Offset")?,
            })
        } else {
            None
        };
        let conditional_fields = struct_or_none(&props, "ConditionalInitialization")?;
        let conditional = if formula.is_none() && enabled(conditional_fields, "bEnabled")? {
            let fields =
                conditional_fields.ok_or("ConditionalInitialization enabled without fields")?;
            let mut cases = Vec::new();
            let list = self.struct_array(fields, "ConditionalExpressionList", package)?;
            for (i, case) in list.iter().enumerate() {
                let what = |e: String| format!("ConditionalExpressionList[{i}]: {e}");
                let mut conditions = Vec::new();
                for expression in self
                    .struct_array(case, "Expressions", package)
                    .map_err(what)?
                {
                    if let Some(condition) = self.condition(&expression, package).map_err(what)? {
                        conditions.push(condition);
                    }
                }
                let value = struct_or_none(case, "BaseValueIfTrue").map_err(what)?;
                cases.push(ConditionalCase {
                    conditions,
                    value: self
                        .value_data(value, package, ValueData::default())
                        .map_err(what)?,
                });
            }
            let default = struct_or_none(fields, "DefaultBaseValue")?;
            Some(Conditional {
                cases,
                default: self.value_data(default, package, ValueData::default())?,
            })
        } else {
            None
        };
        let variance = enabled(struct_or_none(&props, "RandomVariance")?, "bEnabled")?;
        let range = struct_or_none(&props, "RangeRestriction")?;
        let mut bound = |flag: &str, name: &str| -> Result<Option<ValueData>, String> {
            if !enabled(range, flag)? {
                return Ok(None);
            }
            let inner = match range {
                None => None,
                Some(fields) => struct_or_none(fields, name)?,
            };
            self.value_data(inner, package, ValueData::default())
                .map(Some)
        };
        let minimum = bound("bEnableMinValueRestriction", "MinValue")?;
        let maximum = bound("bEnableMaxValueRestriction", "MaxValue")?;
        // The game's own note: conditional initialisation is ignored when the formula is
        // enabled.
        let unsupported = if variance {
            Some("random variance".to_string())
        } else if formula.is_none() && conditional.is_none() {
            Some("no formula".to_string())
        } else {
            None
        };
        Ok(Initialization {
            mode,
            rounding: rounding(&props, Rounding::Float)?,
            formula,
            conditional,
            minimum,
            maximum,
            unsupported,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::UPackage;
    use crate::test_support::{p, World, P, V};

    /// An `AttributeInitializationData` struct value.
    fn data(fields: Vec<P>) -> V {
        V::Struct(fields)
    }

    /// An attribute at `path` with one value resolver of class `resolver` and, when given, one
    /// context resolver.
    fn attribute(
        w: &mut World,
        path: &str,
        resolver: &str,
        resolver_props: &[P],
        context: Option<&str>,
    ) -> i32 {
        let attribute = w.object(path, "AttributeDefinition", &[]);
        let value = w.object(&format!("{path}.{resolver}_0"), resolver, resolver_props);
        let mut props = vec![p("ValueResolverChain", V::Objs(vec![value]))];
        if let Some(context) = context {
            let context = w.object(&format!("{path}.{context}_0"), context, &[]);
            props.push(p("ContextResolverChain", V::Objs(vec![context])));
        }
        w.pkg.set_export_props(attribute, &props);
        attribute
    }

    /// A small game: a weapon type, a grip, a title, and what they refer to.
    fn world() -> World {
        let mut w = World::new();
        let weapon = Some("WeaponAttributeContextResolver");
        let property = |name: &'static str| vec![p("PropertyName", V::Name(name))];
        let damage = attribute(
            &mut w,
            "D_A.W.Damage",
            "ObjectPropertyAttributeValueResolver",
            &property("InstantHitDamage"),
            weapon,
        );
        let level = attribute(
            &mut w,
            "D_A.W.Level",
            "ObjectPropertyAttributeValueResolver",
            &property("ExpLevel"),
            weapon,
        );
        let pellets = attribute(
            &mut w,
            "D_A.W.Pellets",
            "ObjectPropertyAttributeValueResolver",
            &property("ProjectilesPerShot"),
            weapon,
        );
        let scaler = w.rarity_attribute("GD_B.Scaler", 1.13);
        let is_jakobs = attribute(
            &mut w,
            "D_A.M.IsJakobs",
            "ManufacturerAttributeValueResolver",
            &[p(
                "Manufacturers",
                V::Structs(vec![vec![
                    p("AssociatedManufacturerName", V::Name("Jakobs")),
                    p(
                        "ValueIfMatched",
                        data(vec![p("BaseValueConstant", V::Float(1.0))]),
                    ),
                ]]),
            )],
            weapon,
        );
        let chance = attribute(
            &mut w,
            "D_A.W.Chance",
            "SimpleMathValueResolver",
            &[
                p("Arg1Option", V::Enum("MATHRESOLVEARG1_FromAttribute")),
                p("Operand", V::Enum("MATHRESOLVEROPERAND_Mul")),
                p(
                    "Arg1Attribute",
                    data(vec![p("BaseValueAttribute", V::Obj(level))]),
                ),
                p(
                    "Argument",
                    data(vec![p("BaseValueConstant", V::Float(0.5))]),
                ),
            ],
            weapon,
        );
        let odd = attribute(&mut w, "D_A.W.Odd", "WeirdResolver", &[], None);
        // An attribute that takes its resolver chain from an archetype.
        let child = w.object("D_A.W.Child", "AttributeDefinition", &[]);
        w.pkg.set_export_archetype(child, damage);
        let pool_resource = w.object("D_Resources.Shield", "ResourceDefinition", &[]);
        let pool = w.object("D_A.P.Capacity", "ResourcePoolAttributeDefinition", &[]);
        let pool_value = w.object(
            "D_A.P.Capacity.ObjectPropertyAttributeValueResolver_0",
            "ObjectPropertyAttributeValueResolver",
            &property("MaxValue"),
        );
        let pool_context = w.object(
            "D_A.P.Capacity.ResourcePoolAttributeContextResolver_0",
            "ResourcePoolAttributeContextResolver",
            &[p("Resource", V::Obj(pool_resource))],
        );
        w.pkg.set_export_props(
            pool,
            &[
                p("ValueResolverChain", V::Objs(vec![pool_value])),
                p("ContextResolverChain", V::Objs(vec![pool_context])),
            ],
        );

        let init = w.object(
            "GD_B.Init_Damage",
            "AttributeInitializationDefinition",
            &[
                p(
                    "ValueFormula",
                    data(vec![
                        p("bEnabled", V::Bool(true)),
                        p(
                            "Multiplier",
                            data(vec![p("BaseValueConstant", V::Float(8.0))]),
                        ),
                        p("Level", data(vec![p("BaseValueAttribute", V::Obj(scaler))])),
                        p("Power", data(vec![p("BaseValueAttribute", V::Obj(level))])),
                    ]),
                ),
                // A minimum that is stated but not enabled, as the game's own definition has.
                p(
                    "RangeRestriction",
                    data(vec![p(
                        "MinValue",
                        data(vec![p("BaseValueConstant", V::Float(5.0))]),
                    )]),
                ),
            ],
        );
        let conditional = w.object(
            "GD_B.Init_Conditional",
            "AttributeInitializationDefinition",
            &[
                p(
                    "BaseValueMode",
                    V::Enum("BASEVALUE_InitializationDefScalesBaseValue"),
                ),
                p("RoundingMode", V::Enum("ATTRROUNDING_IntRound")),
                p(
                    "ConditionalInitialization",
                    data(vec![p("bEnabled", V::Bool(true))]),
                ),
                p(
                    "RangeRestriction",
                    data(vec![
                        p("bEnableMaxValueRestriction", V::Bool(true)),
                        p(
                            "MaxValue",
                            data(vec![p("BaseValueConstant", V::Float(9.0))]),
                        ),
                    ]),
                ),
            ],
        );

        let fire = w.object("GD_Fire.DmgType", "WillowDamageTypeDefinition", &[]);
        // The title exists before its presentations, which live inside it.
        let title = w.object("GD_W.P.Title", "WeaponNamePartDefinition", &[]);
        let red = w.object(
            "GD_W.P.Title.AttributePresentationDefinition_0",
            "AttributePresentationDefinition",
            &[
                p("NoConstraintText", V::Str("Red  [skill]text[-skill]")),
                p("bDontDisplayNumber", V::Bool(true)),
                p("BasePriority", V::Float(20.0)),
                p("TextColor", V::Color([0x46, 0x46, 0xDC, 0xFF])),
            ],
        );
        let numbered = w.object(
            "GD_W.P.Title.AttributePresentationDefinition_1",
            "AttributePresentationDefinition",
            &[p("Description", V::Str("Contains $NUMBER$ bullets."))],
        );
        let localised = w.object(
            "GD_W.P.Title.AttributePresentationDefinition_2",
            "AttributePresentationDefinition",
            &[
                p("Description", V::Str("$WillowGame.Cards.Skin")),
                p("bDontDisplayNumber", V::Bool(true)),
            ],
        );
        w.pkg.set_export_props(
            title,
            &[p(
                "CustomPresentations",
                V::Objs(vec![red, numbered, 0, localised]),
            )],
        );

        let effect = |attribute: i32, kind: Option<&'static str>, value: Vec<P>| {
            let mut fields = vec![p("AttributeToModify", V::Obj(attribute))];
            if let Some(kind) = kind {
                fields.push(p("ModifierType", V::Enum(kind)));
            }
            fields.push(p("BaseModifierValue", data(value)));
            fields
        };
        w.object(
            "GD_W.A.Rifle",
            "WeaponTypeDefinition",
            &[
                p("ClipSize", V::Int(20)),
                p("FireRate", V::Float(0.115)),
                p("Spread", V::Float(2.0)),
                p(
                    "InstantHitDamage",
                    data(vec![
                        p("InitializationDefinition", V::Obj(init)),
                        p("BaseValueScaleConstant", V::Float(1.08)),
                    ]),
                ),
                p("DefaultDamageTypeDefinition", V::Obj(fire)),
                p(
                    "ExternalAttributeEffects",
                    V::Structs(vec![effect(
                        pool,
                        Some("MT_PreAdd"),
                        vec![p("InitializationDefinition", V::Obj(conditional))],
                    )]),
                ),
                p(
                    "AttributeSlotEffects",
                    V::Structs(vec![
                        vec![
                            p("SlotName", V::Name("WeaponDamage")),
                            p("AttributeToModify", V::Obj(damage)),
                            p(
                                "PerGradeUpgrade",
                                data(vec![p("BaseValueConstant", V::Float(0.03))]),
                            ),
                            p("bEnforceMinimumGrade", V::Bool(false)),
                        ],
                        vec![
                            p("SlotName", V::Name("Pool")),
                            p("bExternalSlot", V::Bool(true)),
                            p("AttributeToModify", V::Obj(pool)),
                            p("ModifierType", V::Enum("MT_PostAdd")),
                            p("bEnforceMaximumGrade", V::Bool(true)),
                            p("MaximumGrade", V::Int(7)),
                        ],
                        // A slot with no attribute (a skill slot) is left out.
                        vec![p("SlotName", V::Name("Skill"))],
                    ]),
                ),
                p(
                    "UIStatList",
                    V::Structs(vec![
                        vec![
                            p("Attribute", V::Obj(damage)),
                            p(
                                "SupplementalAttributeExpression",
                                data(vec![
                                    p("AttributeOperand1", V::Obj(pellets)),
                                    p("ComparisonOperator", V::Enum("OPERATOR_GreaterThan")),
                                    p("ConstantOperand2", V::Float(1.0)),
                                ]),
                            ),
                            p("SupplementalAttributeToAppend", V::Obj(pellets)),
                        ],
                        vec![
                            p("Attribute", V::Obj(chance)),
                            p("AttributeStyle", V::Enum("ATTRSTYLE_ModifierText")),
                        ],
                        vec![p("Attribute", V::Obj(odd))],
                        vec![p("Attribute", V::Obj(child))],
                    ]),
                ),
            ],
        );
        w.object(
            "GD_W.P.Grip",
            "WeaponPartDefinition",
            &[
                p(
                    "WeaponAttributeEffects",
                    V::Structs(vec![
                        effect(
                            damage,
                            Some("MT_PreAdd"),
                            vec![
                                p("BaseValueAttribute", V::Obj(is_jakobs)),
                                p("BaseValueScaleConstant", V::Float(0.05)),
                            ],
                        ),
                        effect(damage, None, vec![p("BaseValueConstant", V::Float(-0.2))]),
                        // No attribute: nothing to modify.
                        vec![p("ModifierType", V::Enum("MT_Scale"))],
                    ]),
                ),
                p(
                    "AttributeSlotUpgrades",
                    V::Structs(vec![
                        vec![
                            p("SlotName", V::Name("WeaponDamage")),
                            p("GradeIncrease", V::Int(4)),
                            p("bActivateSlot", V::Bool(true)),
                        ],
                        vec![p("SlotName", V::Name("Pool"))],
                    ]),
                ),
                p("CustomDamageTypeDefinition", V::Obj(fire)),
            ],
        );
        // A part whose effects have the wrong shape.
        w.object(
            "GD_W.P.Broken",
            "WeaponPartDefinition",
            &[p("WeaponAttributeEffects", V::Int(3))],
        );
        w.object("GD_W.Bal.Rifle", "WeaponBalanceDefinition", &[]);

        // Presentations and the globals' lists.
        let damage_presentation = w.object(
            "GD_AP.W.Damage",
            "AttributePresentationDefinition",
            &[
                p("Attribute", V::Obj(damage)),
                p("Description", V::Str("Damage")),
                p("RoundingMode", V::Enum("ATTRROUNDING_IntCeil")),
            ],
        );
        let pellets_presentation = w.object(
            "GD_AP.W.Pellets",
            "AttributePresentationDefinition",
            &[
                p("Attribute", V::Obj(pellets)),
                p("Prefix", V::Str("[projectilecount]x")),
                p("Suffix", V::Str("[-projectilecount]")),
                p("bDisplayAsInverse", V::Bool(true)),
                p("bValueRemappingEnabled", V::Bool(true)),
                p("FloatPrecision", V::Int(2)),
                p(
                    "RemappingData",
                    data(vec![
                        p(
                            "InputValueMx",
                            data(vec![p("BaseValueConstant", V::Float(15.0))]),
                        ),
                        p(
                            "OutputValueMn",
                            data(vec![p("BaseValueAttribute", V::Obj(scaler))]),
                        ),
                    ]),
                ),
            ],
        );
        // An attribute no card shows: its presentation is not kept.
        let unused = w.object(
            "GD_AP.W.Level",
            "AttributePresentationDefinition",
            &[p("Attribute", V::Obj(level))],
        );
        let burn = w.object(
            "GD_AP.E.Burn",
            "AttributePresentationDefinition",
            &[p("Description", V::Str("Burn Damage / sec."))],
        );
        let by_type = w.object(
            "GD_AP.W.Status",
            "WeaponStatusEffectAttributePresentationDefinition",
            &[
                p("Attribute", V::Obj(chance)),
                p(
                    "MappingData",
                    V::Structs(vec![vec![
                        p("Presentation", V::Obj(burn)),
                        p("DamageType", V::Obj(fire)),
                    ]]),
                ),
            ],
        );
        let default_list = w.object(
            "GD_AP.Lists.Default",
            "AttributePresentationListDefinition",
            &[p(
                "Attributes",
                V::Objs(vec![damage_presentation, 0, unused, pellets_presentation]),
            )],
        );
        let weapon_list = w.object(
            "GD_AP.Lists.Weapon",
            "AttributePresentationListDefinition",
            &[p("Attributes", V::Objs(vec![by_type]))],
        );
        let shield_list = w.object(
            "GD_AP.Lists.Shield",
            "AttributePresentationListDefinition",
            &[],
        );
        w.object(
            "GD_Globals.General.Globals",
            "GlobalsDefinition",
            &[
                p("DefaultAttributePresentation", V::Obj(default_list)),
                p("WeaponAttributePresentationOverride", V::Obj(weapon_list)),
                p("ShieldAttributePresentationOverride", V::Obj(shield_list)),
            ],
        );
        w
    }

    fn extract() -> StatsData {
        let packages = [UPackage::parse("t", world().build()).unwrap()];
        let ctx = BuildContext::new(&packages);
        let globals_loc = ctx.index.locate("GD_Globals.General.Globals").unwrap();
        let globals = ctx.properties(globals_loc).unwrap();
        let paths = [
            "GD_W.A.Rifle",
            "GD_W.P.Grip",
            "GD_W.P.Title",
            "GD_W.P.Broken",
            "GD_W.Bal.Rifle",
            "GD_W.P.NotThere",
        ];
        extract_stats(&ctx, paths.into_iter(), &globals, globals_loc.package)
    }

    #[test]
    fn extracts_definitions() {
        let stats = extract();
        // The balance carries no stats and the missing path has no object.
        let mut paths: Vec<&str> = stats.definitions.keys().map(String::as_str).collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            [
                "GD_W.A.Rifle",
                "GD_W.P.Broken",
                "GD_W.P.Grip",
                "GD_W.P.Title"
            ]
        );

        let rifle = &stats.definitions["GD_W.A.Rifle"];
        assert!(rifle.problems.is_empty(), "{:?}", rifle.problems);
        let base = rifle.weapon.as_ref().unwrap();
        assert_eq!(
            (base.clip_size, base.fire_interval, base.spread),
            (20, 0.115, 2.0)
        );
        // Properties the definition does not set take the class defaults.
        assert_eq!(
            (base.reload_time, base.projectiles_per_shot, base.shot_cost),
            (2.1, 1, 1)
        );
        assert_eq!(
            base.damage,
            ValueData {
                constant: 20.0,
                attribute: None,
                initialization: Some("GD_B.Init_Damage".into()),
                scale: 1.08,
            }
        );
        assert_eq!(base.status_effect_damage, ValueData::constant(1.0));
        assert_eq!(base.base_status_effect_chance, ValueData::constant(1.0));
        assert_eq!(base.damage_type.as_deref(), Some("GD_Fire.DmgType"));
        assert_eq!(rifle.effects, []);
        assert_eq!(
            rifle.external_effects,
            [AttributeEffect {
                attribute: "D_A.P.Capacity".into(),
                kind: ModifierKind::PreAdd,
                value: ValueData {
                    initialization: Some("GD_B.Init_Conditional".into()),
                    ..ValueData::default()
                },
            }]
        );
        assert_eq!(
            rifle.slot_effects,
            [
                SlotEffect {
                    slot: "WeaponDamage".into(),
                    external: false,
                    attribute: "D_A.W.Damage".into(),
                    kind: ModifierKind::Scale,
                    base: ValueData::default(),
                    per_grade: ValueData::constant(0.03),
                    minimum_grade: None,
                    maximum_grade: None,
                    fun_stats: false,
                },
                // The struct's defaults: a minimum grade of 0 is enforced unless switched off.
                SlotEffect {
                    slot: "Pool".into(),
                    external: true,
                    attribute: "D_A.P.Capacity".into(),
                    kind: ModifierKind::PostAdd,
                    base: ValueData::default(),
                    per_grade: ValueData::default(),
                    minimum_grade: Some(0),
                    maximum_grade: Some(7),
                    fun_stats: false,
                },
            ]
        );
        assert_eq!(
            rifle.ui_stats,
            [
                UiStat {
                    attribute: Some("D_A.W.Damage".into()),
                    supplemental: Some("D_A.W.Pellets".into()),
                    condition: Some(Condition {
                        attribute: "D_A.W.Pellets".into(),
                        comparison: Comparison::GreaterThan,
                        other_attribute: None,
                        constant: 1.0,
                    }),
                    just_number: true,
                },
                UiStat {
                    attribute: Some("D_A.W.Chance".into()),
                    supplemental: None,
                    condition: None,
                    just_number: false,
                },
                UiStat {
                    attribute: Some("D_A.W.Odd".into()),
                    supplemental: None,
                    condition: None,
                    just_number: true,
                },
                UiStat {
                    attribute: Some("D_A.W.Child".into()),
                    supplemental: None,
                    condition: None,
                    just_number: true,
                },
            ]
        );

        let grip = &stats.definitions["GD_W.P.Grip"];
        assert!(grip.problems.is_empty() && grip.weapon.is_none());
        assert_eq!(
            grip.effects,
            [
                AttributeEffect {
                    attribute: "D_A.W.Damage".into(),
                    kind: ModifierKind::PreAdd,
                    value: ValueData {
                        attribute: Some("D_A.M.IsJakobs".into()),
                        scale: 0.05,
                        ..ValueData::default()
                    },
                },
                AttributeEffect {
                    attribute: "D_A.W.Damage".into(),
                    kind: ModifierKind::Scale,
                    value: ValueData::constant(-0.2),
                },
            ]
        );
        assert_eq!(
            grip.slot_upgrades,
            [
                SlotUpgrade {
                    slot: "WeaponDamage".into(),
                    grade_increase: 4,
                    activates: true,
                },
                SlotUpgrade {
                    slot: "Pool".into(),
                    grade_increase: 0,
                    activates: false,
                },
            ]
        );
        assert_eq!(grip.damage_type.as_deref(), Some("GD_Fire.DmgType"));

        // Fixed text is kept without markup; numbered and localised lines are counted.
        let title = &stats.definitions["GD_W.P.Title"];
        assert_eq!(
            title.texts,
            [CardText {
                text: "Red text".into(),
                color: Some(Rgba::new(0xDC, 0x46, 0x46, 0xFF)),
                priority: 20.0,
            }]
        );
        assert_eq!(title.texts_omitted, 2);

        let broken = &stats.definitions["GD_W.P.Broken"];
        assert_eq!(broken.problems.len(), 1);
        assert!(broken.problems[0].starts_with("WeaponAttributeEffects"));
    }

    #[test]
    fn extracts_attributes_and_formulas() {
        let stats = extract();
        let weapon = |property: &str| AttributeSource::Property {
            context: "WeaponAttributeContextResolver".into(),
            resource: None,
            property: property.into(),
        };
        let a = &stats.attributes;
        assert_eq!(a["D_A.W.Damage"], weapon("InstantHitDamage"));
        assert_eq!(a["D_A.W.Level"], weapon("ExpLevel"));
        // The child has no chains of its own and takes its archetype's.
        assert_eq!(a["D_A.W.Child"], weapon("InstantHitDamage"));
        assert_eq!(a["GD_B.Scaler"], AttributeSource::Constant(1.13));
        assert_eq!(
            a["D_A.M.IsJakobs"],
            AttributeSource::Manufacturer {
                cases: vec![("Jakobs".into(), ValueData::constant(1.0))],
                otherwise: ValueData::default(),
            }
        );
        assert_eq!(
            a["D_A.W.Chance"],
            AttributeSource::Product(
                ValueData {
                    attribute: Some("D_A.W.Level".into()),
                    ..ValueData::default()
                },
                ValueData::constant(0.5),
            )
        );
        assert_eq!(
            a["D_A.W.Odd"],
            AttributeSource::Unsupported("value resolver WeirdResolver".into())
        );
        // Attributes that are only modified, never read as values, are not extracted, except
        // where a card row shows them.
        assert!(!a.contains_key("D_A.P.Capacity"));

        let i = &stats.initializations;
        let damage = &i["GD_B.Init_Damage"];
        assert_eq!(
            (damage.mode, damage.rounding),
            (BaseValueMode::Sets, Rounding::Float)
        );
        let formula = damage.formula.as_ref().unwrap();
        assert_eq!(formula.multiplier, ValueData::constant(8.0));
        assert_eq!(formula.level.attribute.as_deref(), Some("GD_B.Scaler"));
        assert_eq!(formula.power.attribute.as_deref(), Some("D_A.W.Level"));
        assert_eq!(formula.offset, ValueData::default());
        // A minimum that is not enabled is not a restriction.
        assert_eq!(
            (&damage.minimum, &damage.maximum, &damage.unsupported),
            (&None, &None, &None)
        );
        let conditional = &i["GD_B.Init_Conditional"];
        assert_eq!(
            (conditional.mode, conditional.rounding),
            (BaseValueMode::Scales, Rounding::IntRound)
        );
        assert_eq!(conditional.maximum, Some(ValueData::constant(9.0)));
        assert_eq!(conditional.unsupported, None);
        assert_eq!(conditional.formula, None);
        let cases = &conditional.conditional.as_ref().unwrap();
        assert_eq!(cases.cases.len(), 0);
        assert_eq!(cases.default, ValueData::default());
    }

    #[test]
    fn extracts_presentation_lists() {
        let stats = extract();
        let lists: Vec<PresentationList> = stats.presentations.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            lists,
            [
                PresentationList::Default,
                PresentationList::Weapon,
                PresentationList::Shield
            ]
        );
        let entries = |list: PresentationList| {
            &stats
                .presentations
                .iter()
                .find(|(l, _)| *l == list)
                .unwrap()
                .1
        };
        // Only attributes a card shows are kept: the level's presentation is not.
        let default: Vec<&str> = entries(PresentationList::Default)
            .iter()
            .map(|(a, _)| a.as_str())
            .collect();
        assert_eq!(default, ["D_A.W.Damage", "D_A.W.Pellets"]);
        assert!(entries(PresentationList::Shield).is_empty());

        let Some(PresentationRule::Plain(damage)) =
            stats.presentation(PresentationList::Weapon, "D_A.W.Damage")
        else {
            panic!("no damage presentation");
        };
        assert_eq!(damage.label.as_deref(), Some("Damage"));
        assert_eq!(
            (damage.rounding, damage.precision, damage.inverse),
            (Rounding::IntCeil, 1, false)
        );
        assert_eq!(damage.remap, None);

        let Some(PresentationRule::Plain(pellets)) =
            stats.presentation(PresentationList::Default, "D_A.W.Pellets")
        else {
            panic!("no pellets presentation");
        };
        // The class defaults: rounding to the nearest whole number.
        assert_eq!(
            (pellets.rounding, pellets.precision, pellets.inverse),
            (Rounding::IntRound, 2, true)
        );
        assert_eq!(pellets.prefix.as_deref(), Some("x"));
        assert_eq!(pellets.suffix, None); // only markup
        let remap = pellets.remap.as_ref().unwrap();
        assert_eq!(remap.input_min, ValueData::default());
        assert_eq!(remap.input_max, ValueData::constant(15.0));
        assert_eq!(remap.output_min.attribute.as_deref(), Some("GD_B.Scaler"));
        assert_eq!(remap.output_max, ValueData::default());

        let Some(PresentationRule::ByDamageType(by_type)) =
            stats.presentation(PresentationList::Weapon, "D_A.W.Chance")
        else {
            panic!("no status presentation");
        };
        assert_eq!(by_type.len(), 1);
        assert_eq!(by_type[0].0, "GD_Fire.DmgType");
        assert_eq!(by_type[0].1.label.as_deref(), Some("Burn Damage / sec."));
        assert_eq!(
            stats.presentation(PresentationList::Shield, "D_A.W.Chance"),
            None
        );
    }

    #[test]
    fn markup_is_removed() {
        assert_eq!(
            strip_markup("Fires as fast as you can [skill]pull the trigger[-skill]."),
            "Fires as fast as you can pull the trigger."
        );
        assert_eq!(
            strip_markup("It's closer than you think!        (no it isn't)"),
            "It's closer than you think! (no it isn't)"
        );
        // Brackets that are not tags stay.
        assert_eq!(strip_markup("a [b c] d [unclosed"), "a [b c] d [unclosed");
        assert_eq!(strip_markup("[projectilecount]x"), "x");
        assert_eq!(strip_markup(""), "");
    }
}

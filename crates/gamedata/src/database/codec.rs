//! The cache file's format: a compact little-endian encoding of an [`ItemDatabase`].
//!
//! `GKDB`, the schema version, the fingerprint, the sets, the objects (sorted by path), the
//! colour rows, the skipped packages, the stat data (each map sorted by path), then `END!`.
//! Strings are a u32 byte length plus UTF-8.
//! Decoding is bounds-checked like package parsing: a damaged file gives an error, never a
//! panic or a huge allocation.

use super::stat_data::{
    AttributeEffect, AttributeSource, BaseValueMode, CardAttribute, CardText, Comparison,
    Condition, Conditional, ConditionalCase, CustomPresentation, Formula, Initialization,
    ModifierKind, Presentation, PresentationList, PresentationRule, Remap, Rounding, SignStyle,
    SlotEffect, SlotUpgrade, StatDefinition, StatsData, UiStat, ValueData, WeaponBase,
};
use super::{
    AssetCategory, AssetSet, AssetTable, ItemDatabase, ObjectInfo, RarityColorRow, RarityValue,
    Rgba, WeaponKind,
};
use crate::error::{GameDataError, Result};
use crate::package::reader::ByteReader;

const MAGIC: &[u8; 4] = b"GKDB";
const END: &[u8; 4] = b"END!";

const WEAPON_KINDS: [WeaponKind; 6] = [
    WeaponKind::Pistol,
    WeaponKind::Shotgun,
    WeaponKind::Smg,
    WeaponKind::AssaultRifle,
    WeaponKind::SniperRifle,
    WeaponKind::Launcher,
];

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }

    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn count(&mut self, n: usize) {
        // Counts come from tables built out of packages far smaller than 4 GiB.
        self.u32(u32::try_from(n).unwrap_or(u32::MAX));
    }

    fn string(&mut self, s: &str) {
        self.count(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }

    fn optional_string(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                self.u8(1);
                self.string(s);
            }
            None => self.u8(0),
        }
    }

    fn rarity(&mut self, r: RarityValue) {
        match r {
            RarityValue::None => self.u8(0),
            RarityValue::Value(v) => {
                self.u8(1);
                self.0.extend_from_slice(&v.to_bits().to_le_bytes());
            }
            RarityValue::Unresolvable => self.u8(2),
        }
    }

    fn object(&mut self, info: &ObjectInfo) {
        match info {
            ObjectInfo::WeaponType {
                kind,
                type_name,
                base_rarity,
            } => {
                self.u8(0);
                let index = kind.and_then(|k| WEAPON_KINDS.iter().position(|&w| w == k));
                self.u8(index.map_or(0, |i| i as u8 + 1));
                self.optional_string(type_name.as_deref());
                self.rarity(*base_rarity);
            }
            ObjectInfo::ItemType {
                class_name,
                item_name,
                name_is_full,
                base_rarity,
            } => {
                self.u8(1);
                self.string(class_name);
                self.optional_string(item_name.as_deref());
                self.u8(u8::from(*name_is_full));
                self.rarity(*base_rarity);
            }
            ObjectInfo::Part {
                class_name,
                part_name,
                rarity,
            } => {
                self.u8(2);
                self.string(class_name);
                self.optional_string(part_name.as_deref());
                self.rarity(*rarity);
            }
            ObjectInfo::Manufacturer { display_name } => {
                self.u8(3);
                self.optional_string(display_name.as_deref());
            }
            ObjectInfo::Balance { class_name } => {
                self.u8(4);
                self.string(class_name);
            }
        }
    }
}

fn bad(what: &str) -> GameDataError {
    GameDataError::UnexpectedData(format!("cache: {what}"))
}

struct Reader<'a>(ByteReader<'a>);

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8> {
        self.0.u8()
    }

    fn u32(&mut self) -> Result<u32> {
        self.0.u32()
    }

    /// A count of elements that each take at least `min_size` bytes, so a damaged count cannot
    /// ask for more memory than the file is long.
    fn count(&mut self, min_size: usize) -> Result<usize> {
        let n = self.0.u32_size()?;
        if n > self.0.remaining() / min_size {
            return Err(bad("count exceeds the data"));
        }
        Ok(n)
    }

    fn flag(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(bad("flag")),
        }
    }

    fn string(&mut self) -> Result<String> {
        let n = self.count(1)?;
        String::from_utf8(self.0.take(n)?.to_vec()).map_err(|_| bad("text is not UTF-8"))
    }

    fn optional_string(&mut self) -> Result<Option<String>> {
        Ok(if self.flag()? {
            Some(self.string()?)
        } else {
            None
        })
    }

    fn strings(&mut self) -> Result<Vec<String>> {
        let n = self.count(4)?;
        (0..n).map(|_| self.string()).collect()
    }

    fn rarity(&mut self) -> Result<RarityValue> {
        match self.u8()? {
            0 => Ok(RarityValue::None),
            1 => Ok(RarityValue::Value(f64::from_bits(self.0.u64()?))),
            2 => Ok(RarityValue::Unresolvable),
            _ => Err(bad("rarity tag")),
        }
    }

    fn object(&mut self) -> Result<ObjectInfo> {
        Ok(match self.u8()? {
            0 => {
                let kind = match self.u8()? {
                    0 => None,
                    k => Some(
                        *WEAPON_KINDS
                            .get(k as usize - 1)
                            .ok_or_else(|| bad("weapon kind"))?,
                    ),
                };
                ObjectInfo::WeaponType {
                    kind,
                    type_name: self.optional_string()?,
                    base_rarity: self.rarity()?,
                }
            }
            1 => ObjectInfo::ItemType {
                class_name: self.string()?,
                item_name: self.optional_string()?,
                name_is_full: self.flag()?,
                base_rarity: self.rarity()?,
            },
            2 => ObjectInfo::Part {
                class_name: self.string()?,
                part_name: self.optional_string()?,
                rarity: self.rarity()?,
            },
            3 => ObjectInfo::Manufacturer {
                display_name: self.optional_string()?,
            },
            4 => ObjectInfo::Balance {
                class_name: self.string()?,
            },
            _ => return Err(bad("object tag")),
        })
    }

    fn table(&mut self) -> Result<AssetTable> {
        let n = self.count(1)?;
        (0..n).map(|_| self.optional_string()).collect()
    }

    fn set(&mut self) -> Result<AssetSet> {
        let mut set = AssetSet::new(self.optional_string()?);
        for category in AssetCategory::ALL {
            let n = self.count(4)?;
            set.set_tables(
                category,
                (0..n).map(|_| self.table()).collect::<Result<_>>()?,
            );
            set.set_unresolved_sublibraries(category, self.strings()?);
        }
        Ok(set)
    }
}

const MODIFIER_KINDS: [ModifierKind; 3] = [
    ModifierKind::Scale,
    ModifierKind::PreAdd,
    ModifierKind::PostAdd,
];
const COMPARISONS: [Comparison; 6] = [
    Comparison::EqualTo,
    Comparison::NotEqualTo,
    Comparison::LessThan,
    Comparison::LessThanOrEqual,
    Comparison::GreaterThan,
    Comparison::GreaterThanOrEqual,
];
const BASE_VALUE_MODES: [BaseValueMode; 4] = [
    BaseValueMode::Sets,
    BaseValueMode::Adds,
    BaseValueMode::Scales,
    BaseValueMode::OffsetBy,
];
const SIGN_STYLES: [SignStyle; 5] = [
    SignStyle::AsIs,
    SignStyle::Positive,
    SignStyle::Negative,
    SignStyle::AsIsAnyStyle,
    SignStyle::OppositeSign,
];
const ROUNDINGS: [Rounding; 4] = [
    Rounding::Float,
    Rounding::IntRound,
    Rounding::IntFloor,
    Rounding::IntCeil,
];

/// The position of `value` in `all`, as the byte that encodes it.
fn index_of<T: PartialEq>(all: &[T], value: &T) -> u8 {
    all.iter().position(|v| v == value).unwrap_or(0) as u8
}

/// The stat data: definitions, attributes and initialisation definitions sorted by path, then
/// the presentation lists.
impl Writer {
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn flag(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) {
        self.count(items.len());
        for item in items {
            each(self, item);
        }
    }

    fn optional<T>(&mut self, item: Option<&T>, each: impl FnOnce(&mut Self, &T)) {
        match item {
            Some(item) => {
                self.u8(1);
                each(self, item);
            }
            None => self.u8(0),
        }
    }

    fn value_data(&mut self, v: &ValueData) {
        self.f32(v.constant);
        self.optional_string(v.attribute.as_deref());
        self.optional_string(v.initialization.as_deref());
        self.f32(v.scale);
    }

    fn effect(&mut self, e: &AttributeEffect) {
        self.string(&e.attribute);
        self.u8(index_of(&MODIFIER_KINDS, &e.kind));
        self.value_data(&e.value);
    }

    fn slot_effect(&mut self, e: &SlotEffect) {
        self.string(&e.slot);
        self.flag(e.external);
        self.string(&e.attribute);
        self.u8(index_of(&MODIFIER_KINDS, &e.kind));
        self.value_data(&e.base);
        self.value_data(&e.per_grade);
        self.optional(e.minimum_grade.as_ref(), |w, g| w.i32(*g));
        self.optional(e.maximum_grade.as_ref(), |w, g| w.i32(*g));
        self.flag(e.fun_stats);
    }

    fn condition(&mut self, c: &Condition) {
        self.string(&c.attribute);
        self.u8(index_of(&COMPARISONS, &c.comparison));
        self.optional_string(c.other_attribute.as_deref());
        self.f32(c.constant);
    }

    fn custom_presentation(&mut self, c: &CustomPresentation) {
        self.optional_string(c.attribute.as_deref());
        self.presentation(&c.presentation);
    }

    fn ui_stat(&mut self, s: &UiStat) {
        self.optional_string(s.attribute.as_deref());
        self.optional_string(s.supplemental.as_deref());
        self.optional(s.condition.as_ref(), Self::condition);
        self.flag(s.just_number);
    }

    fn card_text(&mut self, t: &CardText) {
        self.string(&t.text);
        self.optional(t.color.as_ref(), |w, c| {
            w.0.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        });
        self.f32(t.priority);
    }

    fn weapon_base(&mut self, b: &WeaponBase) {
        self.i32(b.clip_size);
        self.f32(b.fire_interval);
        self.f32(b.reload_time);
        self.f32(b.spread);
        self.i32(b.projectiles_per_shot);
        self.i32(b.shot_cost);
        self.f32(b.per_shot_accuracy_impulse);
        self.i32(b.automatic_burst_count);
        self.value_data(&b.damage);
        self.value_data(&b.status_effect_damage);
        self.value_data(&b.base_status_effect_chance);
        self.optional_string(b.damage_type.as_deref());
    }

    fn definition(&mut self, d: &StatDefinition) {
        self.optional(d.weapon.as_ref(), Self::weapon_base);
        self.list(&d.effects, Self::effect);
        self.list(&d.external_effects, Self::effect);
        self.list(&d.slot_effects, Self::slot_effect);
        self.list(&d.slot_upgrades, |w, u: &SlotUpgrade| {
            w.string(&u.slot);
            w.i32(u.grade_increase);
            w.flag(u.activates);
        });
        self.list(&d.ui_stats, Self::ui_stat);
        self.optional_string(d.damage_type.as_deref());
        self.list(&d.texts, Self::card_text);
        self.u32(d.texts_omitted);
        self.list(&d.problems, |w, p: &String| w.string(p));
        self.list(&d.presentations, Self::custom_presentation);
        self.list(&d.card_attributes, |w, c: &CardAttribute| {
            w.string(&c.attribute);
            w.f32(c.priority_increase);
        });
    }

    fn attribute_source(&mut self, a: &AttributeSource) {
        match a {
            AttributeSource::Constant(value) => {
                self.u8(0);
                self.f32(*value);
            }
            AttributeSource::Property {
                context,
                resource,
                property,
            } => {
                self.u8(1);
                self.string(context);
                self.optional_string(resource.as_deref());
                self.string(property);
            }
            AttributeSource::Manufacturer { cases, otherwise } => {
                self.u8(2);
                self.list(cases, |w, (name, value)| {
                    w.string(name);
                    w.value_data(value);
                });
                self.value_data(otherwise);
            }
            AttributeSource::Product(left, right) => {
                self.u8(3);
                self.value_data(left);
                self.value_data(right);
            }
            AttributeSource::Unsupported(what) => {
                self.u8(4);
                self.string(what);
            }
            AttributeSource::SlotGrade(slot) => {
                self.u8(5);
                self.string(slot);
            }
        }
    }

    fn initialization(&mut self, i: &Initialization) {
        self.u8(index_of(&BASE_VALUE_MODES, &i.mode));
        self.u8(index_of(&ROUNDINGS, &i.rounding));
        self.optional(i.formula.as_ref(), |w, f| {
            w.value_data(&f.multiplier);
            w.value_data(&f.level);
            w.value_data(&f.power);
            w.value_data(&f.offset);
        });
        self.optional(i.conditional.as_ref(), |w, c| {
            w.list(&c.cases, |w, case: &ConditionalCase| {
                w.list(&case.conditions, Self::condition);
                w.value_data(&case.value);
            });
            w.value_data(&c.default);
        });
        self.optional(i.minimum.as_ref(), Self::value_data);
        self.optional(i.maximum.as_ref(), Self::value_data);
        self.optional_string(i.unsupported.as_deref());
    }

    fn presentation(&mut self, p: &Presentation) {
        self.optional_string(p.label.as_deref());
        self.optional_string(p.no_constraint_text.as_deref());
        self.f32(p.base_priority);
        let flags = [
            p.percentage,
            p.percent_as_float,
            p.plus_sign,
            p.display_number,
            p.custom_placement,
            p.bigger_is_better,
            p.include_prefix,
            p.include_suffix,
        ];
        self.u8(flags
            .iter()
            .enumerate()
            .map(|(i, &f)| u8::from(f) << i)
            .sum());
        self.u8(index_of(&SIGN_STYLES, &p.sign));
        self.optional(p.color.as_ref(), |w, c| {
            w.0.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        });
        self.flag(p.inverse);
        self.optional(p.remap.as_ref(), |w, r| {
            w.value_data(&r.input_min);
            w.value_data(&r.input_max);
            w.value_data(&r.output_min);
            w.value_data(&r.output_max);
        });
        self.u8(index_of(&ROUNDINGS, &p.rounding));
        self.i32(p.precision);
        self.optional_string(p.prefix.as_deref());
        self.optional_string(p.suffix.as_deref());
    }

    fn stats(&mut self, stats: &StatsData) {
        fn sorted<T>(map: &std::collections::HashMap<String, T>) -> Vec<(&String, &T)> {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(path, _)| *path);
            entries
        }
        self.list(&sorted(&stats.definitions), |w, (path, definition)| {
            w.string(path);
            w.definition(definition);
        });
        self.list(&sorted(&stats.attributes), |w, (path, source)| {
            w.string(path);
            w.attribute_source(source);
        });
        self.list(
            &sorted(&stats.initializations),
            |w, (path, initialization)| {
                w.string(path);
                w.initialization(initialization);
            },
        );
        self.list(&stats.presentations, |w, (list, entries)| {
            w.u8(index_of(&PresentationList::ALL, list));
            w.list(entries, |w, (attribute, rule)| {
                w.string(attribute);
                match rule {
                    PresentationRule::Plain(presentation) => {
                        w.u8(0);
                        w.presentation(presentation);
                    }
                    PresentationRule::ByDamageType(by_type) => {
                        w.u8(1);
                        w.list(by_type, |w, (damage_type, presentation)| {
                            w.string(damage_type);
                            w.presentation(presentation);
                        });
                    }
                }
            });
        });
        self.list(&sorted(&stats.damage_types), |w, (path, lines)| {
            w.string(path);
            w.list(lines, Self::custom_presentation);
        });
    }
}

impl Reader<'_> {
    fn f32(&mut self) -> Result<f32> {
        self.0.f32()
    }

    fn i32(&mut self) -> Result<i32> {
        self.0.i32()
    }

    /// A list of elements that each take at least `min_size` bytes.
    fn list<T>(
        &mut self,
        min_size: usize,
        mut each: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let n = self.count(min_size)?;
        (0..n).map(|_| each(self)).collect()
    }

    fn optional<T>(&mut self, each: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        Ok(if self.flag()? {
            Some(each(self)?)
        } else {
            None
        })
    }

    fn pick<T: Copy>(&mut self, all: &[T], what: &str) -> Result<T> {
        let index = self.u8()? as usize;
        all.get(index).copied().ok_or_else(|| bad(what))
    }

    fn value_data(&mut self) -> Result<ValueData> {
        Ok(ValueData {
            constant: self.f32()?,
            attribute: self.optional_string()?,
            initialization: self.optional_string()?,
            scale: self.f32()?,
        })
    }

    fn effect(&mut self) -> Result<AttributeEffect> {
        Ok(AttributeEffect {
            attribute: self.string()?,
            kind: self.pick(&MODIFIER_KINDS, "modifier kind")?,
            value: self.value_data()?,
        })
    }

    fn slot_effect(&mut self) -> Result<SlotEffect> {
        Ok(SlotEffect {
            slot: self.string()?,
            external: self.flag()?,
            attribute: self.string()?,
            kind: self.pick(&MODIFIER_KINDS, "modifier kind")?,
            base: self.value_data()?,
            per_grade: self.value_data()?,
            minimum_grade: self.optional(Self::i32)?,
            maximum_grade: self.optional(Self::i32)?,
            fun_stats: self.flag()?,
        })
    }

    fn condition(&mut self) -> Result<Condition> {
        Ok(Condition {
            attribute: self.string()?,
            comparison: self.pick(&COMPARISONS, "comparison")?,
            other_attribute: self.optional_string()?,
            constant: self.f32()?,
        })
    }

    fn custom_presentation(&mut self) -> Result<CustomPresentation> {
        Ok(CustomPresentation {
            attribute: self.optional_string()?,
            presentation: self.presentation()?,
        })
    }

    fn ui_stat(&mut self) -> Result<UiStat> {
        Ok(UiStat {
            attribute: self.optional_string()?,
            supplemental: self.optional_string()?,
            condition: self.optional(Self::condition)?,
            just_number: self.flag()?,
        })
    }

    fn card_text(&mut self) -> Result<CardText> {
        Ok(CardText {
            text: self.string()?,
            color: self.optional(|r| {
                let c = r.0.take(4)?;
                Ok(Rgba::new(c[0], c[1], c[2], c[3]))
            })?,
            priority: self.f32()?,
        })
    }

    fn weapon_base(&mut self) -> Result<WeaponBase> {
        Ok(WeaponBase {
            clip_size: self.i32()?,
            fire_interval: self.f32()?,
            reload_time: self.f32()?,
            spread: self.f32()?,
            projectiles_per_shot: self.i32()?,
            shot_cost: self.i32()?,
            per_shot_accuracy_impulse: self.f32()?,
            automatic_burst_count: self.i32()?,
            damage: self.value_data()?,
            status_effect_damage: self.value_data()?,
            base_status_effect_chance: self.value_data()?,
            damage_type: self.optional_string()?,
        })
    }

    fn definition(&mut self) -> Result<StatDefinition> {
        Ok(StatDefinition {
            weapon: self.optional(Self::weapon_base)?,
            effects: self.list(15, Self::effect)?,
            external_effects: self.list(15, Self::effect)?,
            slot_effects: self.list(32, Self::slot_effect)?,
            slot_upgrades: self.list(9, |r| {
                Ok(SlotUpgrade {
                    slot: r.string()?,
                    grade_increase: r.i32()?,
                    activates: r.flag()?,
                })
            })?,
            ui_stats: self.list(4, Self::ui_stat)?,
            damage_type: self.optional_string()?,
            texts: self.list(9, Self::card_text)?,
            texts_omitted: self.u32()?,
            problems: self.strings()?,
            presentations: self.list(20, Self::custom_presentation)?,
            card_attributes: self.list(8, |r| {
                Ok(CardAttribute {
                    attribute: r.string()?,
                    priority_increase: r.f32()?,
                })
            })?,
        })
    }

    fn attribute_source(&mut self) -> Result<AttributeSource> {
        Ok(match self.u8()? {
            0 => AttributeSource::Constant(self.f32()?),
            1 => AttributeSource::Property {
                context: self.string()?,
                resource: self.optional_string()?,
                property: self.string()?,
            },
            2 => AttributeSource::Manufacturer {
                cases: self.list(14, |r| Ok((r.string()?, r.value_data()?)))?,
                otherwise: self.value_data()?,
            },
            3 => AttributeSource::Product(self.value_data()?, self.value_data()?),
            4 => AttributeSource::Unsupported(self.string()?),
            5 => AttributeSource::SlotGrade(self.string()?),
            _ => return Err(bad("attribute source tag")),
        })
    }

    fn initialization(&mut self) -> Result<Initialization> {
        Ok(Initialization {
            mode: self.pick(&BASE_VALUE_MODES, "base value mode")?,
            rounding: self.pick(&ROUNDINGS, "rounding")?,
            formula: self.optional(|r| {
                Ok(Formula {
                    multiplier: r.value_data()?,
                    level: r.value_data()?,
                    power: r.value_data()?,
                    offset: r.value_data()?,
                })
            })?,
            conditional: self.optional(|r| {
                Ok(Conditional {
                    cases: r.list(14, |r| {
                        Ok(ConditionalCase {
                            conditions: r.list(14, Self::condition)?,
                            value: r.value_data()?,
                        })
                    })?,
                    default: r.value_data()?,
                })
            })?,
            minimum: self.optional(Self::value_data)?,
            maximum: self.optional(Self::value_data)?,
            unsupported: self.optional_string()?,
        })
    }

    fn presentation(&mut self) -> Result<Presentation> {
        let label = self.optional_string()?;
        let no_constraint_text = self.optional_string()?;
        let base_priority = self.f32()?;
        let flags = self.u8()?;
        let flag = |bit: u8| flags & (1 << bit) != 0;
        let sign = self.pick(&SIGN_STYLES, "sign style")?;
        let color = self.optional(|r| {
            let c = r.0.take(4)?;
            Ok(Rgba::new(c[0], c[1], c[2], c[3]))
        })?;
        Ok(Presentation {
            label,
            no_constraint_text,
            base_priority,
            percentage: flag(0),
            percent_as_float: flag(1),
            plus_sign: flag(2),
            display_number: flag(3),
            custom_placement: flag(4),
            bigger_is_better: flag(5),
            include_prefix: flag(6),
            include_suffix: flag(7),
            sign,
            color,
            inverse: self.flag()?,
            remap: self.optional(|r| {
                Ok(Remap {
                    input_min: r.value_data()?,
                    input_max: r.value_data()?,
                    output_min: r.value_data()?,
                    output_max: r.value_data()?,
                })
            })?,
            rounding: self.pick(&ROUNDINGS, "rounding")?,
            precision: self.i32()?,
            prefix: self.optional_string()?,
            suffix: self.optional_string()?,
        })
    }

    fn stats(&mut self) -> Result<StatsData> {
        let mut stats = StatsData::default();
        for (path, definition) in self.list(30, |r| Ok((r.string()?, r.definition()?)))? {
            stats.definitions.insert(path, definition);
        }
        for (path, source) in self.list(9, |r| Ok((r.string()?, r.attribute_source()?)))? {
            stats.attributes.insert(path, source);
        }
        for (path, initialization) in self.list(10, |r| Ok((r.string()?, r.initialization()?)))? {
            stats.initializations.insert(path, initialization);
        }
        stats.presentations = self.list(5, |r| {
            let list = r.pick(&PresentationList::ALL, "presentation list")?;
            let entries = r.list(9, |r| {
                let attribute = r.string()?;
                let rule = match r.u8()? {
                    0 => PresentationRule::Plain(Box::new(r.presentation()?)),
                    1 => PresentationRule::ByDamageType(
                        r.list(14, |r| Ok((r.string()?, r.presentation()?)))?,
                    ),
                    _ => return Err(bad("presentation rule tag")),
                };
                Ok((attribute, rule))
            })?;
            Ok((list, entries))
        })?;
        for (path, lines) in self.list(8, |r| {
            Ok((r.string()?, r.list(20, Self::custom_presentation)?))
        })? {
            stats.damage_types.insert(path, lines);
        }
        Ok(stats)
    }
}

impl ItemDatabase {
    /// Encodes the database for the cache file.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(MAGIC);
        w.u32(self.schema);
        w.string(&self.fingerprint);

        w.count(self.sets.len());
        for (id, set) in &self.sets {
            w.u32(*id);
            w.optional_string(set.dlc_name.as_deref());
            for category in AssetCategory::ALL {
                let tables = set.tables(category);
                w.count(tables.len());
                for table in tables {
                    w.count(table.len());
                    for path in table {
                        w.optional_string(path.as_deref());
                    }
                }
                let unresolved = set.unresolved_sublibraries(category);
                w.count(unresolved.len());
                for name in unresolved {
                    w.string(name);
                }
            }
        }

        let mut objects: Vec<(&String, &ObjectInfo)> = self.objects.iter().collect();
        objects.sort_by_key(|(path, _)| *path);
        w.count(objects.len());
        for (path, info) in objects {
            w.string(path);
            w.object(info);
        }

        w.count(self.rarity_colors.len());
        for row in &self.rarity_colors {
            w.0.extend_from_slice(&row.min_level.to_le_bytes());
            w.0.extend_from_slice(&row.max_level.to_le_bytes());
            w.0.extend_from_slice(&[row.color.r, row.color.g, row.color.b, row.color.a]);
            w.string(&row.rating);
        }

        w.count(self.skipped_packages.len());
        for name in &self.skipped_packages {
            w.string(name);
        }
        w.stats(&self.stat_data);
        w.0.extend_from_slice(END);
        w.0
    }

    /// Decodes a cache file. Fails on anything that is not a complete encoding; it does not
    /// check the schema version or fingerprint against what the caller expects.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader(ByteReader::new(bytes));
        if r.0.take(4)? != MAGIC {
            return Err(bad("not a Vault Key item database"));
        }
        let schema = r.u32()?;
        let mut db = ItemDatabase::new(r.string()?);
        db.schema = schema;

        for _ in 0..r.count(4)? {
            let id = r.u32()?;
            db.sets.insert(id, r.set()?);
        }
        for _ in 0..r.count(5)? {
            let path = r.string()?;
            db.objects.insert(path, r.object()?);
        }
        for _ in 0..r.count(16)? {
            db.rarity_colors.push(RarityColorRow {
                min_level: r.0.i32()?,
                max_level: r.0.i32()?,
                color: {
                    let c = r.0.take(4)?;
                    Rgba::new(c[0], c[1], c[2], c[3])
                },
                rating: r.string()?,
            });
        }
        db.skipped_packages = r.strings()?;
        db.stat_data = r.stats()?;
        if r.0.take(4)? != END || !r.0.at_end() {
            return Err(bad("no end marker"));
        }
        Ok(db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ItemDatabase {
        let mut base = AssetSet::new(None);
        base.set_tables(
            AssetCategory::Balances,
            vec![vec![Some("GD_X.A.Bal".into()), None], vec![]],
        );
        base.set_tables(
            AssetCategory::Manufacturers,
            vec![vec![Some("GD_M.M.Vladof".into())]],
        );
        base.set_unresolved_sublibraries(
            AssetCategory::Balances,
            vec!["GD_Y.PackageAssetSublibrary_1".into()],
        );
        let mut dlc = AssetSet::new(Some("Tulip".into()));
        dlc.set_tables(
            AssetCategory::ItemParts,
            vec![vec![Some("GD_T.P.Title".into())]],
        );

        let mut db = ItemDatabase::new("abc");
        db.sets.insert(0, base);
        db.sets.insert(2, dlc);
        let objects = [
            (
                "GD_X.A.Bal",
                ObjectInfo::Balance {
                    class_name: "WeaponBalanceDefinition".into(),
                },
            ),
            (
                "GD_M.M.Vladof",
                ObjectInfo::Manufacturer {
                    display_name: Some("Vladof".into()),
                },
            ),
            (
                "GD_M.M.Nobody",
                ObjectInfo::Manufacturer { display_name: None },
            ),
            (
                "GD_T.P.Title",
                ObjectInfo::Part {
                    class_name: "ItemNamePartDefinition".into(),
                    part_name: Some("Caf\u{e9} \u{2014} Necromancer".into()),
                    rarity: RarityValue::Value(497.0),
                },
            ),
            (
                "GD_W.T.Sniper",
                ObjectInfo::WeaponType {
                    kind: Some(WeaponKind::SniperRifle),
                    type_name: Some("Sniper Rifle".into()),
                    base_rarity: RarityValue::None,
                },
            ),
            (
                "GD_W.T.Odd",
                ObjectInfo::WeaponType {
                    kind: None,
                    type_name: None,
                    base_rarity: RarityValue::Unresolvable,
                },
            ),
            (
                "GD_I.T.Echo",
                ObjectInfo::ItemType {
                    class_name: "UsableItemDefinition".into(),
                    item_name: Some("Helmet Data Rec0rding".into()),
                    name_is_full: true,
                    base_rarity: RarityValue::Value(502.0),
                },
            ),
        ];
        db.objects
            .extend(objects.map(|(path, info)| (path.to_string(), info)));
        db.rarity_colors = vec![RarityColorRow {
            min_level: 7,
            max_level: 10,
            color: Rgba::new(255, 180, 0, 255),
            rating: "RARITY_Legendary".into(),
        }];
        db.skipped_packages = vec!["Broken_Startup_SF.upk: truncated data: header".into()];
        db.stat_data = sample_stats();
        db
    }

    /// Stat data that uses every variant and every optional field both ways.
    fn sample_stats() -> StatsData {
        let value =
            |constant: f32, attribute: Option<&str>, initialization: Option<&str>| ValueData {
                constant,
                attribute: attribute.map(str::to_string),
                initialization: initialization.map(str::to_string),
                scale: 1.08,
            };
        let presentation = |label: Option<&str>, remap: bool| Presentation {
            label: label.map(str::to_string),
            no_constraint_text: remap.then(|| "Weapon".to_string()),
            base_priority: 0.5,
            percentage: !remap,
            percent_as_float: remap,
            plus_sign: remap,
            display_number: !remap,
            custom_placement: remap,
            bigger_is_better: !remap,
            sign: if remap {
                SignStyle::OppositeSign
            } else {
                SignStyle::AsIs
            },
            color: remap.then_some(Rgba::new(220, 70, 70, 255)),
            include_prefix: !remap,
            include_suffix: remap,
            inverse: remap,
            remap: remap.then(|| Remap {
                input_min: value(0.0, None, None),
                input_max: value(15.0, Some("A.Max"), None),
                output_min: value(100.0, None, None),
                output_max: value(0.0, None, Some("I.Out")),
            }),
            rounding: if remap {
                Rounding::Float
            } else {
                Rounding::IntCeil
            },
            precision: 2,
            prefix: remap.then(|| "x".to_string()),
            suffix: (!remap).then(|| "%".to_string()),
        };
        let mut stats = StatsData::default();
        stats.definitions.insert(
            "T.Rifle".into(),
            StatDefinition {
                weapon: Some(WeaponBase {
                    clip_size: 20,
                    fire_interval: 0.115,
                    reload_time: 3.5,
                    spread: 2.0,
                    projectiles_per_shot: 1,
                    shot_cost: 2,
                    per_shot_accuracy_impulse: 2.3,
                    automatic_burst_count: 3,
                    damage: value(20.0, None, Some("I.Damage")),
                    status_effect_damage: value(1.0, Some("A.Dot"), None),
                    base_status_effect_chance: value(0.3, None, None),
                    damage_type: Some("D.Normal".into()),
                }),
                effects: vec![AttributeEffect {
                    attribute: "A.Damage".into(),
                    kind: ModifierKind::PreAdd,
                    value: value(-0.5, Some("A.IsJakobs"), None),
                }],
                external_effects: vec![AttributeEffect {
                    attribute: "A.Pool".into(),
                    kind: ModifierKind::PostAdd,
                    value: value(4.0, None, None),
                }],
                slot_effects: vec![
                    SlotEffect {
                        slot: "WeaponDamage".into(),
                        external: false,
                        attribute: "A.Damage".into(),
                        kind: ModifierKind::Scale,
                        base: value(0.0, None, None),
                        per_grade: value(0.03, None, None),
                        minimum_grade: Some(-2),
                        maximum_grade: None,
                        fun_stats: true,
                    },
                    SlotEffect {
                        slot: "AccuracyMin".into(),
                        external: true,
                        attribute: "A.Pool".into(),
                        kind: ModifierKind::PreAdd,
                        base: value(1.0, None, None),
                        per_grade: value(0.0, None, None),
                        minimum_grade: None,
                        maximum_grade: Some(7),
                        fun_stats: true,
                    },
                ],
                slot_upgrades: vec![SlotUpgrade {
                    slot: "WeaponDamage".into(),
                    grade_increase: -3,
                    activates: true,
                }],
                ui_stats: vec![
                    UiStat {
                        attribute: Some("A.Damage".into()),
                        supplemental: Some("A.Pellets".into()),
                        condition: Some(Condition {
                            attribute: "A.Pellets".into(),
                            comparison: Comparison::GreaterThan,
                            other_attribute: Some("A.One".into()),
                            constant: 1.0,
                        }),
                        just_number: true,
                    },
                    UiStat {
                        attribute: None,
                        supplemental: None,
                        condition: None,
                        just_number: false,
                    },
                ],
                damage_type: None,
                texts: vec![
                    CardText {
                        text: "Caf\u{e9} \u{2014} red".into(),
                        color: Some(Rgba::new(220, 70, 70, 255)),
                        priority: 20.0,
                    },
                    CardText {
                        text: "plain".into(),
                        color: None,
                        priority: 0.1,
                    },
                ],
                texts_omitted: 3,
                problems: vec!["UIStatList[2]: Attribute is Int(3)".into()],
                presentations: vec![
                    CustomPresentation {
                        attribute: Some("A.Pellets".into()),
                        presentation: presentation(Some("x$NUMBER$"), true),
                    },
                    CustomPresentation {
                        attribute: None,
                        presentation: presentation(Some("Red text"), false),
                    },
                ],
                card_attributes: vec![CardAttribute {
                    attribute: "A.Damage".into(),
                    priority_increase: 0.5,
                }],
            },
        );
        stats.definitions.insert(
            "P.Fire".into(),
            StatDefinition {
                damage_type: Some("D.Fire".into()),
                ..StatDefinition::default()
            },
        );
        let sources = [
            ("A.Scaler", AttributeSource::Constant(1.13)),
            (
                "A.Damage",
                AttributeSource::Property {
                    context: "WeaponAttributeContextResolver".into(),
                    resource: None,
                    property: "InstantHitDamage".into(),
                },
            ),
            (
                "A.Pool",
                AttributeSource::Property {
                    context: "ResourcePoolAttributeContextResolver".into(),
                    resource: Some("D_Resources.Shield".into()),
                    property: "MaxValue".into(),
                },
            ),
            (
                "A.IsJakobs",
                AttributeSource::Manufacturer {
                    cases: vec![("Jakobs".into(), value(1.0, None, None))],
                    otherwise: value(0.0, None, None),
                },
            ),
            (
                "A.Chance",
                AttributeSource::Product(value(0.0, Some("A.X"), None), value(2.0, None, None)),
            ),
            (
                "A.Odd",
                AttributeSource::Unsupported("0 value resolvers".into()),
            ),
            ("A.Grade", AttributeSource::SlotGrade("Skill1".into())),
        ];
        stats
            .attributes
            .extend(sources.map(|(path, source)| (path.to_string(), source)));
        stats.initializations.insert(
            "I.Damage".into(),
            Initialization {
                mode: BaseValueMode::Scales,
                rounding: Rounding::IntRound,
                formula: Some(Formula {
                    multiplier: value(8.0, None, None),
                    level: value(0.0, Some("A.Scaler"), None),
                    power: value(0.0, Some("A.Level"), None),
                    offset: value(0.0, None, None),
                }),
                conditional: None,
                minimum: Some(value(5.0, None, None)),
                maximum: None,
                unsupported: None,
            },
        );
        stats.initializations.insert(
            "I.Out".into(),
            Initialization {
                mode: BaseValueMode::OffsetBy,
                rounding: Rounding::IntFloor,
                formula: None,
                conditional: Some(Conditional {
                    cases: vec![
                        ConditionalCase {
                            conditions: vec![Condition {
                                attribute: "A.Level".into(),
                                comparison: Comparison::GreaterThanOrEqual,
                                other_attribute: None,
                                constant: 50.0,
                            }],
                            value: value(4.0, None, None),
                        },
                        ConditionalCase {
                            conditions: vec![],
                            value: value(1.0, Some("A.X"), None),
                        },
                    ],
                    default: value(1.0, None, None),
                }),
                minimum: None,
                maximum: Some(value(9.0, None, None)),
                unsupported: Some("random variance".into()),
            },
        );
        stats.presentations = vec![
            (
                PresentationList::Default,
                vec![
                    (
                        "A.Damage".into(),
                        PresentationRule::Plain(Box::new(presentation(Some("Damage"), false))),
                    ),
                    (
                        "A.Spread".into(),
                        PresentationRule::Plain(Box::new(presentation(None, true))),
                    ),
                ],
            ),
            (PresentationList::Shield, vec![]),
            (
                PresentationList::Weapon,
                vec![(
                    "A.Dot".into(),
                    PresentationRule::ByDamageType(vec![
                        ("D.Fire".into(), presentation(Some("Burn"), false)),
                        ("D.Shock".into(), presentation(Some("Shock"), true)),
                    ]),
                )],
            ),
        ];
        stats.damage_types.insert(
            "D.Fire".into(),
            vec![CustomPresentation {
                attribute: None,
                presentation: presentation(Some("Highly effective vs Flesh."), false),
            }],
        );
        stats
    }

    #[test]
    fn round_trips() {
        let db = sample();
        let bytes = db.to_bytes();
        let back = ItemDatabase::from_bytes(&bytes).unwrap();
        assert_eq!(back, db);
        // Encoding does not depend on hash-map order.
        assert_eq!(back.to_bytes(), bytes);
        assert_eq!(
            ItemDatabase::from_bytes(&ItemDatabase::new("").to_bytes())
                .unwrap()
                .sets
                .len(),
            0
        );
    }

    #[test]
    fn damaged_files_are_errors() {
        let bytes = sample().to_bytes();
        assert!(ItemDatabase::from_bytes(&[]).is_err());
        assert!(ItemDatabase::from_bytes(b"{").is_err());
        // Every truncation fails, and never by panicking.
        for len in 0..bytes.len() {
            assert!(
                ItemDatabase::from_bytes(&bytes[..len]).is_err(),
                "prefix of {len} bytes"
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(ItemDatabase::from_bytes(&trailing).is_err());
        // Flipping any single byte either fails or decodes to something; it never panics.
        for i in 0..bytes.len() {
            let mut flipped = bytes.clone();
            flipped[i] ^= 0xFF;
            let _ = ItemDatabase::from_bytes(&flipped);
        }
        // An enum byte outside its range is an error wherever it appears.
        let position = |needle: &[u8]| {
            bytes
                .windows(needle.len())
                .position(|w| w == needle)
                .unwrap()
        };
        let after_path = position(b"A.Scaler") + "A.Scaler".len();
        let mut bad_tag = bytes.clone();
        bad_tag[after_path] = 9; // the attribute source tag
        assert!(ItemDatabase::from_bytes(&bad_tag).is_err());
        // A count far larger than the file is refused before any allocation.
        let mut huge = bytes.clone();
        let sets_count_at = 4 + 4 + 4 + 3;
        huge[sets_count_at..sets_count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(ItemDatabase::from_bytes(&huge).is_err());
    }
}

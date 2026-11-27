//! Which item produced an effect (`CombatSourceCatalog.cs`), from `assets/game-data/combat.json`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatItemDefinition {
    pub index: i32,
    pub id: String,
    pub name: String,
    pub slot: String,
    pub tier: Option<i32>,
    pub enchantment: i32,
    #[serde(default)]
    pub passive_spells: Option<Vec<String>>,
    #[serde(default)]
    pub spells: Option<Vec<String>>,
}

impl CombatItemDefinition {
    pub fn new(index: i32, id: &str, name: &str, slot: &str, tier: Option<i32>, enchantment: i32) -> Self {
        Self { index, id: id.into(), name: name.into(), slot: slot.into(), tier, enchantment, passive_spells: None, spells: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatSpellDefinition {
    pub index: i32,
    pub id: String,
    pub effects: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatSourceData {
    pub version: String,
    pub items: Vec<CombatItemDefinition>,
    pub spells: Vec<CombatSpellDefinition>,
}

/// The item a contribution is credited to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemAttribution {
    pub index: Option<i32>,
    pub slot: &'static str,
    pub kind: &'static str,
    pub exact: bool,
}

impl ItemAttribution {
    pub fn new(index: Option<i32>, slot: &str, kind: &str) -> Self {
        Self { index, slot: crate::cs::intern(slot), kind: crate::cs::intern(kind), exact: true }
    }
    pub fn unknown() -> Self {
        Self::new(None, "unknown", "unknown")
    }
}

type Name = u32;

struct Item {
    definition: CombatItemDefinition,
    slot: &'static str,
    family: String,
    spells: Option<Vec<Name>>,
    passives: Option<Vec<Name>>,
}

/// Spell and item names are interned, so effect lookups hash integers instead of strings.
pub struct CombatSourceCatalog {
    items: HashMap<i32, Item>,
    order: Vec<i32>,
    spells: HashMap<i32, Name>,
    item_ids: HashSet<String>,
    descendants: HashMap<Name, HashSet<Name>>,
    family_by_effect: Mutex<HashMap<i32, Option<ItemAttribution>>>,
    producers: Mutex<HashMap<Name, Arc<HashMap<i32, bool>>>>,
    representatives: HashMap<String, i32>,
    ids_by_name: Mutex<Option<HashMap<String, String>>>,
}

const TIER_WORDS: [&str; 8] = ["Beginner's ", "Novice's ", "Journeyman's ", "Adept's ", "Expert's ", "Master's ", "Grandmaster's ", "Elder's "];

fn family_of(id: &str) -> String {
    let base = id.split('@').next().unwrap_or("");
    let bytes = base.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'T' && bytes[1].is_ascii_digit() && bytes[2] == b'_' { base[3..].to_owned() } else { base.to_owned() }
}

pub fn slot_name(slot: usize) -> &'static str {
    match slot {
        0 => "mainhand",
        1 => "offhand",
        2 => "head",
        3 => "chest",
        4 => "shoes",
        6 => "cape",
        8 => "potion",
        9 => "food",
        _ => "unknown",
    }
}

const SKILL_SLOTS: [i32; 14] = [0, 0, 0, 3, 2, 4, -1, -1, -1, -1, -1, -1, 8, 9];

fn positive(value: Option<i32>) -> Option<i32> {
    value.filter(|&v| v > 0)
}

impl CombatSourceCatalog {
    pub fn new(data: CombatSourceData) -> Self {
        let mut names: HashMap<String, Name> = HashMap::new();
        let mut intern = |name: &str| -> Name {
            let next = names.len() as Name;
            *names.entry(name.to_owned()).or_insert(next)
        };
        let mut by_name: HashMap<Name, Vec<Name>> = HashMap::new();
        let mut first_order = Vec::new();
        let mut spells = HashMap::new();
        for spell in &data.spells {
            let id = intern(&spell.id);
            spells.insert(spell.index, id);
            if let std::collections::hash_map::Entry::Vacant(entry) = by_name.entry(id) {
                entry.insert(spell.effects.iter().map(|effect| intern(effect)).collect());
                first_order.push(id);
            }
        }
        let mut descendants = HashMap::new();
        for &spell in &first_order {
            let mut seen = HashSet::new();
            let mut pending = vec![spell];
            while let Some(id) = pending.pop() {
                if seen.len() >= 512 {
                    break;
                }
                if !seen.insert(id) {
                    continue;
                }
                let Some(effects) = by_name.get(&id) else { continue };
                pending.extend(effects.iter().copied());
            }
            descendants.insert(spell, seen);
        }
        let mut items = HashMap::new();
        let mut order = Vec::new();
        let mut representatives: HashMap<String, i32> = HashMap::new();
        for definition in data.items.iter().cloned() {
            let family = family_of(&definition.id);
            let representative = representatives.entry(family.clone()).or_insert(definition.index);
            *representative = (*representative).min(definition.index);
            let spells = definition.spells.as_ref().map(|list| list.iter().map(|name| intern(name)).collect());
            let passives = definition.passive_spells.as_ref().map(|list| list.iter().map(|name| intern(name)).collect());
            if !items.contains_key(&definition.index) {
                order.push(definition.index);
            }
            let slot = crate::cs::intern(&definition.slot);
            items.insert(definition.index, Item { definition, slot, family, spells, passives });
        }
        Self {
            item_ids: data.items.iter().map(|item| item.id.clone()).collect(),
            items,
            order,
            spells,
            descendants,
            family_by_effect: Mutex::new(HashMap::new()),
            producers: Mutex::new(HashMap::new()),
            representatives,
            ids_by_name: Mutex::new(None),
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(Self::new(serde_json::from_str(&text).ok()?))
    }

    pub fn find_item(&self, index: i32) -> Option<&CombatItemDefinition> {
        self.items.get(&index).map(|item| &item.definition)
    }

    fn produces_name(&self, skill: Name, effect: Name) -> bool {
        self.descendants.get(&skill).is_some_and(|set| set.contains(&effect))
    }

    pub fn produces(&self, active: i32, effect: i32) -> bool {
        active == effect
            || match (self.spells.get(&active), self.spells.get(&effect)) {
                (Some(&root), Some(&effect)) => self.produces_name(root, effect),
                _ => false,
            }
    }

    pub fn contains_item(&self, id: &str) -> bool {
        self.item_ids.contains(id)
    }

    pub fn family_name(&self, index: Option<i32>) -> String {
        match index.and_then(|i| self.items.get(&i)) {
            Some(item) => {
                let name = TIER_WORDS.iter().fold(item.definition.name.as_str(), |name, word| name.strip_prefix(word).unwrap_or(name));
                format!("{name} · tier not observed")
            }
            None => self.item_name(index),
        }
    }

    fn producing(&self, skills: &[Name], effect: Name) -> Vec<Name> {
        skills.iter().copied().filter(|&skill| self.produces_name(skill, effect)).collect()
    }

    pub fn unique_family(&self, effect_index: i32) -> Option<ItemAttribution> {
        if let Some(cached) = self.family_by_effect.lock().unwrap().get(&effect_index) {
            return cached.clone();
        }
        let mut result = None;
        if let Some(&effect) = self.spells.get(&effect_index) {
            let mut families: Vec<(String, ItemAttribution)> = Vec::new();
            let mut producers = Vec::new();
            for index in &self.order {
                let item = &self.items[index];
                let Some(spells) = &item.spells else { continue };
                if item.definition.slot == "bag" || item.definition.slot == "mount" {
                    continue;
                }
                let producing = self.producing(spells, effect);
                if producing.is_empty() {
                    continue;
                }
                producers.push(item.definition.index);
                if !families.iter().any(|(family, _)| *family == item.family) {
                    let passive = producing.iter().all(|skill| item.passives.as_ref().is_some_and(|p| p.contains(skill)));
                    families.push((
                        item.family.clone(),
                        ItemAttribution {
                            index: Some(self.family_representative(item.definition.index)),
                            slot: item.slot,
                            kind: if passive { "passive" } else { "ability" },
                            exact: false,
                        },
                    ));
                }
                if families.len() > 1 {
                    break;
                }
            }
            if families.len() == 1 {
                let family = families.pop().unwrap().1;
                result = Some(if let [only] = producers[..] { ItemAttribution { index: Some(only), exact: true, ..family } } else { family });
            }
        }
        let mut cache = self.family_by_effect.lock().unwrap();
        if cache.len() < 16_384 {
            cache.insert(effect_index, result.clone());
        }
        result
    }

    pub fn family_representative(&self, item: i32) -> i32 {
        self.items.get(&item).map(|definition| self.representatives[&definition.family]).unwrap_or(item)
    }

    pub fn same_family(&self, item: i32, other: i32) -> bool {
        matches!((self.items.get(&item), self.items.get(&other)), (Some(a), Some(b)) if a.family == b.family)
    }

    fn worn_skills(item: &Item) -> &[Name] {
        item.spells.as_deref().or(item.passives.as_deref()).unwrap_or(&[])
    }

    /// The items whose worn skills (`Spells ?? PassiveSpells`) produce an effect, each with whether only its
    /// passives do. Built once per effect, so resolving a hit is a lookup per worn item instead of a scan of its skills.
    fn producers(&self, effect: Name) -> Arc<HashMap<i32, bool>> {
        if let Some(known) = self.producers.lock().unwrap().get(&effect) {
            return known.clone();
        }
        let mut producers = HashMap::new();
        for (index, item) in &self.items {
            let mut producing = Self::worn_skills(item).iter().filter(|&&skill| self.produces_name(skill, effect)).peekable();
            if producing.peek().is_some() {
                producers.insert(*index, producing.all(|skill| item.passives.as_ref().is_some_and(|p| p.contains(skill))));
            }
        }
        let producers = Arc::new(producers);
        let mut cache = self.producers.lock().unwrap();
        if cache.len() < 16_384 {
            cache.insert(effect, producers.clone());
        }
        producers
    }

    pub fn could_produce(&self, effect_index: i32, equipment: &[Option<i32>]) -> bool {
        let Some(&effect) = self.spells.get(&effect_index) else { return true };
        let producers = self.producers(effect);
        for (slot, &worn) in equipment.iter().enumerate().take(10) {
            let Some(worn) = positive(worn) else { continue };
            if slot == 5 || slot == 7 {
                continue;
            }
            if !self.items.contains_key(&worn) || producers.contains_key(&worn) {
                return true;
            }
        }
        false
    }

    pub fn item_id_by_name(&self, name: &str) -> Option<String> {
        let mut cache = self.ids_by_name.lock().unwrap();
        let map = cache.get_or_insert_with(|| {
            let mut map = HashMap::new();
            for index in &self.order {
                let item = &self.items[index].definition;
                map.entry(format!("{} · T{}.{}", item.name, tier_text(item.tier), item.enchantment)).or_insert_with(|| item.id.clone());
            }
            map
        });
        map.get(name).cloned()
    }

    pub fn item_name(&self, index: Option<i32>) -> String {
        match index {
            None => "Unknown source".into(),
            Some(0) => "Unarmed".into(),
            Some(index) => match self.items.get(&index) {
                Some(item) => format!("{} · T{}.{}", item.definition.name, tier_text(item.definition.tier), item.definition.enchantment),
                None => format!("Item #{index}"),
            },
        }
    }

    pub fn resolve(&self, spell_index: Option<i32>, origin: Option<i32>, damage: bool, equipment: &[Option<i32>], active_spells: &[Option<i32>]) -> ItemAttribution {
        if damage && spell_index == Some(-1) && matches!(origin, Some(0 | 1)) {
            return ItemAttribution::new(equipment.first().copied().flatten(), "mainhand", "autoAttack");
        }
        let Some(&effect) = spell_index.and_then(|index| self.spells.get(&index)) else { return ItemAttribution::unknown() };
        // Slots are 0..=9 (and the cape at 6): sets of them are bit masks, so resolving allocates nothing.
        let mut matches = 0u16;
        for (index, &slot) in SKILL_SLOTS.iter().enumerate().take(active_spells.len()) {
            if slot < 0 || slot as usize >= equipment.len() || positive(equipment[slot as usize]).is_none() {
                continue;
            }
            let Some(active) = active_spells[index] else { continue };
            if self.spells.get(&active).is_some_and(|&root| self.produces_name(root, effect)) {
                matches |= 1 << slot;
            }
        }
        if let Some(Some(cape)) = equipment.get(6).filter(|_| equipment.len() > 6)
            && let Some(cape) = self.items.get(cape).filter(|item| item.definition.slot == "cape")
            && cape.passives.as_deref().unwrap_or(&[]).iter().any(|&passive| self.produces_name(passive, effect))
        {
            matches |= 1 << 6;
        }
        let mut allowed = 0u16;
        let mut only_passive = true;
        let mut unprovable = false;
        let producers = self.producers(effect);
        for (slot, &worn) in equipment.iter().enumerate().take(10) {
            let Some(worn) = positive(worn) else { continue };
            if slot == 5 || slot == 7 || matches.count_ones() > 1 && matches & (1 << slot) == 0 {
                continue;
            }
            if !self.items.contains_key(&worn) {
                unprovable = true;
                continue;
            }
            let Some(&passive) = producers.get(&worn) else { continue };
            allowed |= 1 << slot;
            only_passive &= passive;
        }
        if matches.count_ones() != 1 && allowed.count_ones() == 1 && !unprovable {
            return attribution(equipment, allowed.trailing_zeros() as usize, only_passive);
        }
        if matches.count_ones() != 1 {
            return ItemAttribution::unknown();
        }
        let slot = matches.trailing_zeros() as usize;
        attribution(equipment, slot, slot == 6)
    }
}

fn tier_text(tier: Option<i32>) -> String {
    tier.map(|t| t.to_string()).unwrap_or_default()
}

fn attribution(equipment: &[Option<i32>], slot: usize, passive: bool) -> ItemAttribution {
    ItemAttribution::new(equipment[slot], slot_name(slot), if passive { "passive" } else { "ability" })
}

//! The map and weapon catalog imported from the game (`MapCatalog.cs`, data types of `GameAssetImporter.cs`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapDefinition {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub tier: Option<i32>,
    pub file: String,
    pub enabled: bool,
    pub exits: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponDefinition {
    pub id: String,
    pub name: String,
    pub family: String,
    pub tier: Option<i32>,
    pub slot: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedWeaponDefinition {
    pub index: i32,
    pub id: String,
    pub name: String,
    pub family: String,
    pub tier: Option<i32>,
    pub enchantment: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapCatalogData {
    pub schema_version: i32,
    pub source: String,
    pub game_version: String,
    /// The importer's `DateTimeOffset`, kept as written.
    pub imported_at: String,
    pub source_file: String,
    pub sha256: String,
    pub map_count: i32,
    pub maps: Vec<MapDefinition>,
    #[serde(default)]
    pub weapon_count: i32,
    #[serde(default)]
    pub weapons: Option<Vec<WeaponDefinition>>,
    #[serde(default)]
    pub indexed_weapons: Option<Vec<IndexedWeaponDefinition>>,
}

const FAMILY_NAMES: [(&str, &str); 17] = [
    ("arcanestaff", "Arcane Staff"),
    ("axe", "Axe"),
    ("bow", "Bow"),
    ("crossbow", "Crossbow"),
    ("cursestaff", "Cursed Staff"),
    ("dagger", "Dagger"),
    ("firestaff", "Fire Staff"),
    ("froststaff", "Frost Staff"),
    ("hammer", "Hammer"),
    ("holystaff", "Holy Staff"),
    ("knuckles", "War Gloves"),
    ("mace", "Mace"),
    ("naturestaff", "Nature Staff"),
    ("quarterstaff", "Quarterstaff"),
    ("shapeshifterstaff", "Shapeshifter Staff"),
    ("spear", "Spear"),
    ("sword", "Sword"),
];

/// Lookups are indexed once at load instead of scanning the lists as the C# does.
#[derive(Default)]
pub struct MapCatalog {
    data: Option<MapCatalogData>,
    maps: HashMap<String, usize>,
    weapons: HashMap<i32, usize>,
}

impl MapCatalog {
    pub fn new(data: Option<MapCatalogData>) -> Self {
        let mut catalog = Self::default();
        catalog.set(data);
        catalog
    }

    /// Reads `maps.json`; a missing or unreadable file is an empty catalog, as in C#.
    pub fn load(path: impl AsRef<Path>) -> Self {
        Self::new(std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()))
    }

    fn set(&mut self, data: Option<MapCatalogData>) {
        self.maps.clear();
        self.weapons.clear();
        if let Some(data) = &data {
            for (index, map) in data.maps.iter().enumerate() {
                self.maps.entry(map.id.clone()).or_insert(index);
            }
            for (index, weapon) in data.indexed_weapons.iter().flatten().enumerate() {
                self.weapons.entry(weapon.index).or_insert(index);
            }
        }
        self.data = data;
    }

    pub fn data(&self) -> Option<&MapCatalogData> {
        self.data.as_ref()
    }

    pub fn find(&self, id: &str) -> Option<&MapDefinition> {
        self.maps.get(id).map(|&index| &self.data.as_ref().unwrap().maps[index])
    }

    fn weapon(&self, index: i32) -> Option<&IndexedWeaponDefinition> {
        self.weapons.get(&index).map(|&i| &self.data.as_ref().unwrap().indexed_weapons.as_ref().unwrap()[i])
    }

    pub fn weapon_name(&self, index: Option<i32>) -> String {
        match index {
            None => "Weapon not observed".into(),
            Some(0) => "Unarmed".into(),
            Some(index) => match self.weapon(index) {
                Some(w) => format!("{} · T{}.{}", w.name, w.tier.map(|t| t.to_string()).unwrap_or_default(), w.enchantment),
                None => format!("Item #{index}"),
            },
        }
    }

    pub fn weapon_family(&self, index: Option<i32>) -> Option<String> {
        let family = index.filter(|&i| i > 0).and_then(|i| self.weapon(i)).map(|w| w.family.as_str())?;
        if family == "unknown" {
            return None;
        }
        Some(FAMILY_NAMES.iter().find(|(key, _)| *key == family).map_or(family, |(_, name)| name).to_owned())
    }

    pub fn resolve_name(&self, id: &str) -> String {
        self.find(id).map_or_else(|| id.to_owned(), |map| map.name.clone())
    }

    /// Writes the catalog atomically (temporary file, then rename) and uses it from now on.
    pub fn save(&mut self, path: impl AsRef<Path>, data: MapCatalogData) -> std::io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec(&data)?)?;
        std::fs::rename(&temporary, path)?;
        self.set(Some(data));
        Ok(())
    }
}

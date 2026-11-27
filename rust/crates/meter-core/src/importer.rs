//! Reads the game's encrypted data files into the map and combat catalogs (`GameAssetImporter.cs`).

use crate::catalog::{CombatItemDefinition, CombatSourceData, CombatSpellDefinition};
use crate::cs::DateTimeOffset;
use crate::maps::{IndexedWeaponDefinition, MapCatalogData, MapDefinition, WeaponDefinition};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use roxmltree::{Document, Node, ParsingOptions};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read;

const KEY: [u8; 8] = [0x30, 0xEF, 0x72, 0x47, 0x42, 0xF2, 0x04, 0x32];
const IV: [u8; 8] = [0x0E, 0xA6, 0xDC, 0x89, 0xDB, 0xED, 0xDC, 0x4F];

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidData(pub String);

impl fmt::Display for InvalidData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidData {}

type Result<T> = std::result::Result<T, InvalidData>;

fn invalid(message: &str) -> InvalidData {
    InvalidData(message.into())
}

/// Decrypts (DES-CBC), decompresses (gzip) and decodes a game file to its XML text.
pub fn open(encrypted: &[u8]) -> Result<String> {
    if encrypted.is_empty() || encrypted.len() > 20_000_000 {
        return Err(invalid("Game files must be between 1 byte and 20 MB."));
    }
    let compressed = cbc::Decryptor::<des::Des>::new(&KEY.into(), &IV.into())
        .decrypt_padded_vec_mut::<Pkcs7>(encrypted)
        .map_err(|_| invalid("The game file cannot be decrypted."))?;
    let mut text = String::new();
    flate2::read::MultiGzDecoder::new(&compressed[..]).take(120_000_001).read_to_string(&mut text).map_err(|_| invalid("The game file is not valid compressed XML."))?;
    if text.len() > 120_000_000 {
        return Err(invalid("The game file is too large."));
    }
    Ok(text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text))
}

fn parse(text: &str) -> Result<Document<'_>> {
    Document::parse_with_options(text, ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..ParsingOptions::default() })
        .map_err(|error| InvalidData(error.to_string()))
}

fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attribute(name)
}

fn children<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    children(node).find(|c| c.tag_name().name() == name)
}

fn descendants<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.descendants().skip(1).filter(Node::is_element)
}

/// `int.TryParse`.
fn integer(text: Option<&str>) -> Option<i32> {
    text?.trim().parse().ok()
}

fn distinct(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values.into_iter().filter(|v| seen.insert(v.clone())).collect()
}

fn blank(text: &str) -> bool {
    crate::cs::is_blank(text)
}

pub fn import_maps(world: &[u8], items: Option<&[u8]>, localization: Option<&[u8]>, version: &str, source_file: &str) -> Result<MapCatalogData> {
    let text = open(world)?;
    let document = parse(&text)?;
    let clusters = child(document.root_element(), "clusters").ok_or_else(|| invalid("World clusters were not found."))?;
    let maps: Vec<MapDefinition> = children(clusters)
        .filter(|c| c.tag_name().name() == "cluster")
        .map(|cluster| {
            let get = |key: &str| attribute(cluster, key).unwrap_or("").to_owned();
            let file = get("file");
            let tier = file.match_indices("_T").find_map(|(at, _)| {
                let rest = &file.as_bytes()[at + 2..];
                (rest.len() >= 2 && (b'1'..=b'8').contains(&rest[0]) && rest[1] == b'_').then(|| i32::from(rest[0] - b'0'))
            });
            let exits = child(cluster, "exits").map_or_else(Vec::new, |exits| {
                distinct(
                    children(exits)
                        .filter(|e| e.tag_name().name() == "exit")
                        .filter_map(|e| attribute(e, "targetid"))
                        .filter(|t| !blank(t))
                        .map(|t| t.rsplit('@').next().unwrap_or("").to_owned()),
                )
            });
            let name = get("displayname");
            MapDefinition {
                id: get("id"),
                name: if name.is_empty() { get("id") } else { name },
                kind: get("type"),
                tier,
                file,
                enabled: get("enabled") != "false",
                exits,
            }
        })
        .collect();
    let ids: HashSet<&str> = maps.iter().map(|m| m.id.as_str()).collect();
    if maps.is_empty() || maps.iter().any(|m| blank(&m.id)) || ids.len() != maps.len() {
        return Err(invalid("World map identifiers are missing or duplicated."));
    }
    let weapons = match items {
        Some(items) => read_weapons(items, localization)?,
        None => Vec::new(),
    };
    let indexed = match items {
        Some(items) => read_indexed_weapons(items, &weapons)?,
        None => Vec::new(),
    };
    Ok(MapCatalogData {
        schema_version: 1,
        source: "local-game-installation".into(),
        game_version: version.into(),
        imported_at: DateTimeOffset::now().to_string(),
        source_file: source_file.into(),
        sha256: Sha256::digest(world).iter().map(|b| format!("{b:02x}")).collect(),
        map_count: maps.len() as i32,
        maps,
        weapon_count: weapons.len() as i32,
        weapons: Some(weapons),
        indexed_weapons: Some(indexed),
    })
}

const TRIGGER_ATTRIBUTES: [&str; 11] = [
    "spell", "effect", "endeffect", "collisioneffect", "landscapecollisioneffect", "spellonhit", "spellonend", "spellonexpire", "spellontrigger",
    "applyspellwhentargetsinarea", "applyspellwhennotargetsinarea",
];

pub fn import_combat(items: &[u8], spells: &[u8], localization: Option<&[u8]>, version: &str, transformations: Option<&[u8]>) -> Result<CombatSourceData> {
    let item_text = open(items)?;
    let spell_text = open(spells)?;
    let item_document = parse(&item_text)?;
    let item_elements: Vec<Node> = children(item_document.root_element()).collect();
    let ids: HashSet<String> =
        item_elements.iter().filter(|i| i.has_attribute("slottype")).map(|i| attribute(*i, "uniquename").unwrap_or("").to_owned()).collect();
    let names = match localization {
        Some(localization) => read_names(localization, &ids)?,
        None => HashMap::new(),
    };
    let spell_document = parse(&spell_text)?;
    let spell_root = spell_document.root_element();
    let spell_names: HashSet<&str> = children(spell_root).filter_map(|s| attribute(s, "uniquename")).collect();
    let passives: HashSet<&str> = children(spell_root).filter(|s| s.tag_name().name() == "passivespell").filter_map(|s| attribute(s, "uniquename")).collect();
    let forms_text = transformations.map(open).transpose()?;
    let forms_document = forms_text.as_deref().map(parse).transpose()?;
    let mut forms: HashMap<&str, Vec<String>> = HashMap::new();
    if let Some(document) = &forms_document {
        for form in children(document.root_element()).filter(|f| f.tag_name().name() == "transformation") {
            if let Some(id) = attribute(form, "uniquename") {
                let spells = distinct(
                    descendants(form)
                        .filter(|e| matches!(e.tag_name().name(), "spell" | "passivespell"))
                        .filter_map(|e| attribute(e, "uniquename"))
                        .map(str::to_owned),
                );
                forms.insert(id, spells);
            }
        }
    }
    let mut by_id: HashMap<&str, Node> = HashMap::new();
    for item in &item_elements {
        if let Some(id) = attribute(*item, "uniquename") {
            by_id.entry(id).or_insert(*item);
        }
    }
    fn pool(item: Node, by_id: &HashMap<&str, Node>, depth: u32) -> Vec<String> {
        let Some(list) = child(item, "craftingspelllist") else { return Vec::new() };
        let shared = match attribute(list, "reference").and_then(|r| by_id.get(r)) {
            Some(source) if depth < 8 => pool(*source, by_id, depth + 1),
            _ => Vec::new(),
        };
        let own = children(list).filter(|s| s.tag_name().name() == "craftspell").filter_map(|s| attribute(s, "uniquename")).map(str::to_owned);
        distinct(shared.into_iter().chain(own))
    }
    let mut definitions = Vec::new();
    let mut index = 1;
    for item in &item_elements {
        let Some(id) = attribute(*item, "uniquename").filter(|id| !id.is_empty()) else { continue };
        let slot = attribute(*item, "slottype");
        let crafted = if slot.is_none() { Vec::new() } else { pool(*item, &by_id, 0) };
        let mut add = |level: i32, consumed: Option<&str>| {
            let pool = match consumed {
                Some(consumed) if spell_names.contains(consumed) => distinct(crafted.iter().cloned().chain(std::iter::once(consumed.to_owned()))),
                _ => crafted.clone(),
            };
            if let Some(slot) = slot {
                let passive = if slot == "cape" {
                    distinct(descendants(*item).filter(|e| e.tag_name().name() == "craftspell").filter_map(|e| attribute(e, "uniquename")).map(str::to_owned))
                } else {
                    pool.iter().filter(|s| passives.contains(s.as_str())).cloned().collect()
                };
                definitions.push(CombatItemDefinition {
                    index,
                    id: if level == 0 { id.to_owned() } else { format!("{id}@{level}") },
                    name: names.get(id).cloned().unwrap_or_else(|| id.to_owned()),
                    slot: slot.to_owned(),
                    tier: integer(attribute(*item, "tier")),
                    enchantment: level,
                    passive_spells: Some(passive),
                    spells: (!pool.is_empty()).then_some(pool),
                });
            }
            index += 1;
        };
        add(integer(attribute(*item, "enchantmentlevel")).unwrap_or(0), attribute(*item, "consumespell"));
        if let Some(enchantments) = child(*item, "enchantments") {
            for enchantment in children(enchantments) {
                let level = attribute(enchantment, "enchantmentlevel").and_then(|l| l.trim().parse().ok()).ok_or_else(|| invalid("Invalid enchantment level."))?;
                add(level, attribute(enchantment, "consumespell"));
            }
        }
    }
    let mut spell_definitions = Vec::new();
    let mut index = 0;
    for spell in children(spell_root) {
        let tag = spell.tag_name().name();
        if tag == "colortag" {
            continue;
        }
        if !matches!(tag, "activespell" | "passivespell" | "togglespell") {
            return Err(invalid("Unsupported spell definition."));
        }
        let id = attribute(spell, "uniquename").ok_or_else(|| invalid("Missing spell identifier."))?;
        let triggered = descendants(spell)
            .filter(|e| !e.tag_name().name().starts_with("If") && !e.tag_name().name().starts_with("remove"))
            .flat_map(|e| {
                let spell_names = &spell_names;
                let parent = e.tag_name().name();
                e.attributes()
                    .filter(move |a| TRIGGER_ATTRIBUTES.contains(&a.name()) && spell_names.contains(a.value()) || parent == "triggerspell" && a.name() == "name")
                    .map(|a| a.value().to_owned())
                    .collect::<Vec<_>>()
            });
        let transformed = descendants(spell)
            .filter(|e| e.tag_name().name() == "transformation")
            .filter_map(|e| attribute(e, "type"))
            .flat_map(|form| forms.get(form).cloned().unwrap_or_default());
        let effects = distinct(triggered.chain(transformed));
        spell_definitions.push(CombatSpellDefinition { index, id: id.into(), effects: effects.clone() });
        index += 1;
        if tag == "activespell" && child(spell, "channelingspell").is_some() {
            spell_definitions.push(CombatSpellDefinition { index, id: id.into(), effects });
            index += 1;
        }
    }
    Ok(CombatSourceData { version: version.into(), items: definitions, spells: spell_definitions })
}

fn read_indexed_weapons(items: &[u8], weapons: &[WeaponDefinition]) -> Result<Vec<IndexedWeaponDefinition>> {
    let text = open(items)?;
    let document = parse(&text)?;
    let lookup: HashMap<&str, &WeaponDefinition> = weapons.iter().map(|w| (w.id.as_str(), w)).collect();
    let mut result = Vec::new();
    let mut index = 1;
    for item in children(document.root_element()) {
        let Some(id) = attribute(item, "uniquename").filter(|id| !id.is_empty()) else { continue };
        let mut add = |enchantment: i32| {
            if let Some(weapon) = lookup.get(id) {
                result.push(IndexedWeaponDefinition {
                    index,
                    id: if enchantment == 0 { id.to_owned() } else { format!("{id}@{enchantment}") },
                    name: weapon.name.clone(),
                    family: weapon.family.clone(),
                    tier: weapon.tier,
                    enchantment,
                });
            }
            index += 1;
        };
        add(integer(attribute(item, "enchantmentlevel")).unwrap_or(0));
        if let Some(enchantments) = child(item, "enchantments") {
            for enchantment in children(enchantments) {
                add(attribute(enchantment, "enchantmentlevel").and_then(|l| l.trim().parse().ok()).ok_or_else(|| invalid("Invalid enchantment level."))?);
            }
        }
    }
    Ok(result)
}

fn read_weapons(items: &[u8], localization: Option<&[u8]>) -> Result<Vec<WeaponDefinition>> {
    let text = open(items)?;
    let document = parse(&text)?;
    let root = document.root_element();
    if root.tag_name().name() != "items" {
        return Err(invalid("The selected items file is invalid."));
    }
    let definitions: Vec<Node> = children(root)
        .filter(|i| matches!(i.tag_name().name(), "weapon" | "transformationweapon") && attribute(*i, "shopcategory") == Some("weapons"))
        .collect();
    let ids: HashSet<String> = definitions.iter().map(|i| attribute(*i, "uniquename").unwrap_or("").to_owned()).collect();
    let names = match localization {
        Some(localization) => read_names(localization, &ids)?,
        None => HashMap::new(),
    };
    definitions
        .iter()
        .map(|item| {
            let id = attribute(*item, "uniquename").ok_or_else(|| invalid("Missing weapon identifier."))?;
            Ok(WeaponDefinition {
                id: id.into(),
                name: names.get(id).cloned().unwrap_or_else(|| id.to_owned()),
                family: attribute(*item, "shopsubcategory1").unwrap_or("unknown").into(),
                tier: integer(attribute(*item, "tier")),
                slot: attribute(*item, "slottype").unwrap_or("unknown").into(),
            })
        })
        .collect()
}

fn read_names(localization: &[u8], ids: &HashSet<String>) -> Result<HashMap<String, String>> {
    let text = open(localization)?;
    let document = parse(&text)?;
    let mut names = HashMap::new();
    for unit in document.descendants().filter(|n| n.is_element() && n.tag_name().name() == "tu") {
        let Some(key) = attribute(unit, "tuid").filter(|k| k.starts_with("@ITEMS_") && ids.contains(&k[7..])) else { continue };
        let value = children(unit)
            .find(|t| {
                t.tag_name().name() == "tuv"
                    && t.attribute(("http://www.w3.org/XML/1998/namespace", "lang")).is_some_and(|lang| lang.eq_ignore_ascii_case("EN-US"))
            })
            .and_then(|t| child(t, "seg"))
            .map(|seg| seg.descendants().filter(Node::is_text).filter_map(|n| n.text()).collect::<String>());
        if let Some(value) = value.filter(|v| !blank(v)) {
            names.insert(key[7..].to_owned(), value);
        }
    }
    Ok(names)
}

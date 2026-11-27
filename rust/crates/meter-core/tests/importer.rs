//! GameAssetImporter checks: the encrypted game XML becomes the catalogs (ObservationChecks "Imported area effects").

use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockEncryptMut, KeyIvInit};
use meter_core::CombatSourceCatalog;
use meter_core::importer::{import_combat, import_maps, open};
use std::io::Write;

fn encrypt(xml: &str) -> Vec<u8> {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(xml.as_bytes()).unwrap();
    let compressed = gzip.finish().unwrap();
    let key = [0x30, 0xEF, 0x72, 0x47, 0x42, 0xF2, 0x04, 0x32];
    let iv = [0x0E, 0xA6, 0xDC, 0x89, 0xDB, 0xED, 0xDC, 0x4F];
    cbc::Encryptor::<des::Des>::new(&key.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(&compressed)
}

#[test]
fn imported_area_effects_retain_the_weapon_source_of_ironbreaker_damage() {
    let data = import_combat(
        &encrypt("<items><equipmentitem uniquename='T6_MAIN_HAMMER' slottype='mainhand' tier='6'/></items>"),
        &encrypt("<spells><colortag/><activespell uniquename='IRONBREAKER'><spelleffectarea effect='IRONBREAKER_EFFECT'/></activespell><activespell uniquename='IRONBREAKER_EFFECT'/></spells>"),
        None,
        "test",
        None,
    )
    .unwrap();
    let sources = CombatSourceCatalog::new(data);
    let resolved = sources.resolve(Some(1), Some(2), true, &[Some(1)], &[Some(0)]);
    assert!(resolved.index == Some(1) && resolved.slot == "mainhand" && resolved.kind == "ability");
    assert!(sources.resolve(Some(1), Some(2), true, &[Some(1)], &[]).index.is_none());
}

#[test]
fn items_enchantments_crafting_pools_capes_and_channeled_spells_import_like_the_game_data() {
    let items = encrypt(concat!(
        "<items>",
        "<weapon uniquename='T4_MAIN_FIRESTAFF' slottype='mainhand' tier='4' shopcategory='weapons' shopsubcategory1='firestaff'>",
        "<craftingspelllist><craftspell uniquename='FIREBALL'/><craftspell uniquename='PASSIVE_BURN'/></craftingspelllist>",
        "<enchantments><enchantment enchantmentlevel='1' consumespell='FIRE_P1'/><enchantment enchantmentlevel='2'/></enchantments></weapon>",
        "<simpleitem uniquename='T4_RUNE'/>",
        "<weapon uniquename='T5_MAIN_FIRESTAFF' slottype='mainhand' tier='5' shopcategory='weapons' shopsubcategory1='firestaff'><craftingspelllist reference='T4_MAIN_FIRESTAFF'/></weapon>",
        "<equipmentitem uniquename='T4_CAPEITEM_FW_THETFORD' slottype='cape' tier='4'><craftingspelllist><craftspell uniquename='CAPE_THETFORD'/></craftingspelllist></equipmentitem>",
        "</items>"
    ));
    let spells = encrypt(concat!(
        "<spells>",
        "<activespell uniquename='FIREBALL'><channelingspell/><IfTargetIsMob><spelleffect effect='HIDDEN'/></IfTargetIsMob><triggerspell name='FIREBALL_EXPLOSION'/></activespell>",
        "<passivespell uniquename='PASSIVE_BURN'/>",
        "<activespell uniquename='FIRE_P1'/>",
        "<activespell uniquename='HIDDEN'/>",
        "<passivespell uniquename='CAPE_THETFORD'/>",
        "</spells>"
    ));
    let localization = encrypt(concat!(
        "<tmx><body>",
        "<tu tuid='@ITEMS_T4_MAIN_FIRESTAFF'><tuv xml:lang='EN-US'><seg>Adept's Fire Staff</seg></tuv><tuv xml:lang='DE-DE'><seg>Feuerstab</seg></tuv></tu>",
        "<tu tuid='@ITEMS_T4_RUNE'><tuv xml:lang='en-us'><seg>Rune</seg></tuv></tu>",
        "</body></tmx>"
    ));
    let data = import_combat(&items, &spells, Some(&localization), "1.0", None).unwrap();
    let ids: Vec<(i32, &str)> = data.items.iter().map(|i| (i.index, i.id.as_str())).collect();
    assert_eq!(ids, [(1, "T4_MAIN_FIRESTAFF"), (2, "T4_MAIN_FIRESTAFF@1"), (3, "T4_MAIN_FIRESTAFF@2"), (5, "T5_MAIN_FIRESTAFF"), (6, "T4_CAPEITEM_FW_THETFORD")]);
    assert_eq!(data.items[0].name, "Adept's Fire Staff");
    assert_eq!(data.items[3].name, "T5_MAIN_FIRESTAFF");
    assert_eq!(data.items[0].spells.as_deref(), Some(&["FIREBALL".to_owned(), "PASSIVE_BURN".to_owned()][..]));
    assert_eq!(data.items[0].passive_spells.as_deref(), Some(&["PASSIVE_BURN".to_owned()][..]));
    assert_eq!(data.items[1].spells.as_ref().unwrap().last().map(String::as_str), Some("FIRE_P1"));
    assert_eq!(data.items[3].spells, data.items[0].spells);
    assert_eq!(data.items[4].passive_spells.as_deref(), Some(&["CAPE_THETFORD".to_owned()][..]));
    let spells: Vec<(i32, &str, Vec<&str>)> = data.spells.iter().map(|s| (s.index, s.id.as_str(), s.effects.iter().map(String::as_str).collect())).collect();
    // Only the If* element's own attributes are skipped; an effect nested inside it still counts, as in C#.
    assert_eq!(spells[0], (0, "FIREBALL", vec!["HIDDEN", "FIREBALL_EXPLOSION"]));
    assert_eq!(spells[1], (1, "FIREBALL", vec!["HIDDEN", "FIREBALL_EXPLOSION"]));
    assert_eq!(spells[2].0, 2);
    assert_eq!(spells.len(), 6);
}

#[test]
fn world_clusters_import_as_maps_with_tiers_exits_and_indexed_weapons() {
    let world = encrypt(concat!(
        "<world><clusters>",
        "<cluster id='1301' displayname='Birchcopse' type='SAFEAREA' file='Steppe_T4_Birch_02.cluster.xml'><exits><exit targetid='abc@1302'/><exit targetid='x@1302'/><exit targetid=' '/></exits></cluster>",
        "<cluster id='1302' type='OPENPVP' file='Forest_T9_X.cluster.xml' enabled='false'/>",
        "</clusters></world>"
    ));
    let items = encrypt("<items><weapon uniquename='T4_2H_BOW' slottype='mainhand' tier='4' shopcategory='weapons' shopsubcategory1='bow'><enchantments><enchantment enchantmentlevel='1'/></enchantments></weapon></items>");
    let data = import_maps(&world, Some(&items), None, "1.0", "world.bin").unwrap();
    assert_eq!(data.map_count, 2);
    assert!(data.maps[0].name == "Birchcopse" && data.maps[0].tier == Some(4) && data.maps[0].exits == ["1302"] && data.maps[0].enabled);
    assert!(data.maps[1].name == "1302" && data.maps[1].tier.is_none() && !data.maps[1].enabled);
    assert_eq!(data.weapons.as_ref().unwrap()[0].family, "bow");
    let indexed = data.indexed_weapons.as_ref().unwrap();
    assert!(indexed.len() == 2 && indexed[1].id == "T4_2H_BOW@1" && indexed[1].index == 2);
    assert_eq!(data.sha256.len(), 64);
    assert!(import_maps(&encrypt("<world><clusters><cluster id='a'/><cluster id='a'/></clusters></world>"), None, None, "1", "w").is_err());
    assert!(open(&[]).is_err() && open(&[1, 2, 3, 4, 5, 6, 7, 8]).is_err());
    assert!(open(&encrypt("<!DOCTYPE x [<!ENTITY a 'b'>]><x>&a;</x>")).is_ok_and(|text| import_combat(text.as_bytes(), &[], None, "", None).is_err()));
}

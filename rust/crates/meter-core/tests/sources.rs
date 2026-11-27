//! tests/AlbionMeter.Tests/ObservationChecks.cs, part two: item sources, rosters, the local character, party
//! bindings and Protocol18.

mod common;

use common::*;
use meter_core::CombatSourceCatalog;
use meter_core::capture::protocol18::decode;
use meter_core::catalog::{CombatItemDefinition, CombatSourceData, CombatSpellDefinition};
use meter_core::observation::local::LocalCharacterObservation;
use meter_core::observation::party::{ObservedCharacter, PartyError, PartyObservation};
use std::sync::Arc;

fn list(values: &[&str]) -> Option<Vec<String>> {
    Some(values.iter().map(|&v| v.to_owned()).collect())
}

/// `new(index, id, name, slot, tier, enchantment, passiveSpells, spells)`
#[allow(clippy::too_many_arguments)]
fn item(index: i32, id: &str, name: &str, slot: &str, tier: i32, enchantment: i32, passive: Option<&[&str]>, spells: Option<&[&str]>) -> CombatItemDefinition {
    CombatItemDefinition { passive_spells: passive.and_then(list), spells: spells.and_then(list), ..CombatItemDefinition::new(index, id, name, slot, Some(tier), enchantment) }
}

fn spell(index: i32, id: &str, effects: &[&str]) -> CombatSpellDefinition {
    CombatSpellDefinition { index, id: id.into(), effects: list(effects).unwrap() }
}

fn catalog(items: Vec<CombatItemDefinition>, spells: Vec<CombatSpellDefinition>) -> Arc<CombatSourceCatalog> {
    Arc::new(CombatSourceCatalog::new(CombatSourceData { version: "test".into(), items, spells }))
}

fn some(values: &[i32]) -> Vec<Option<i32>> {
    gear(values)
}

const NONE: Option<&[&str]> = None;
const EMPTY: Option<&[&str]> = Some(&[]);

fn event_at(combat: &mut CapturedCombatObservation, fields: Vec<(u8, Value<'static>)>, at: i64, local: Option<&LocalCharacterSnapshot>, party: Option<&PartyRosterSnapshot>) {
    combat.on(ev(fields), "flow", &format!("event:{at}"), at, local, party);
}

#[test]
fn a_skill_several_item_families_share_is_never_assigned_to_one_of_them_without_the_worn_item() {
    let catalog = catalog(
        vec![item(1, "T4_MAIN_HOLYSTAFF", "Holy Staff", "mainhand", 4, 0, EMPTY, Some(&["HOLYFLASH"])), item(2, "T4_2H_DIVINESTAFF", "Divine Staff", "mainhand", 4, 0, EMPTY, Some(&["HOLYFLASH"]))],
        vec![spell(0, "HOLYFLASH", &[])],
    );
    assert!(catalog.unique_family(0).is_none());
    let resolved = catalog.resolve(Some(0), Some(3), false, &some(&[1]), &[]);
    assert!(resolved.index == Some(1) && resolved.slot == "mainhand");
    assert!(catalog.resolve(Some(0), Some(3), false, &[], &[]).index.is_none());
    assert_eq!(catalog.family_name(Some(1)), "Holy Staff · tier not observed");
}

#[test]
fn worn_items_game_skill_lists_place_effects_and_equipment_reported_later_corrects_earlier_effects_of_the_same_visit() {
    let catalog = catalog(
        vec![
            item(1, "T4_MAIN_NATURESTAFF_AVALON", "Ironroot Staff", "mainhand", 4, 0, EMPTY, Some(&["SOUL_LINK"])),
            item(2, "T5_MAIN_NATURESTAFF_AVALON", "Ironroot Staff", "mainhand", 5, 0, EMPTY, Some(&["SOUL_LINK"])),
            item(3, "T6_ARMOR_LEATHER_ROYAL", "Royal Jacket", "armor", 6, 0, EMPTY, Some(&["FLAMESHIELD"])),
            item(4, "T4_MAIN_FIRESTAFF", "Fire Staff", "mainhand", 4, 0, EMPTY, Some(&["FIREBALL"])),
        ],
        vec![spell(0, "SOUL_LINK", &["SOUL_LINK_MULTISPELL2"]), spell(1, "SOUL_LINK_MULTISPELL2", &[]), spell(2, "FLAMESHIELD", &[]), spell(3, "FIREBALL", &[])],
    );
    let (one, two, three) = (new_id(), new_id(), new_id());
    let roster = roster(&[(one, "One"), (two, "Two"), (three, "Three")], 1, true);
    let mut combat = CapturedCombatObservation::new(Some(catalog));
    combat.begin_session(Some(1), None);
    let mut at = 1_000i64;
    let mut event = |combat: &mut CapturedCombatObservation, fields: Vec<(u8, Value<'static>)>| {
        let identity = format!("event:{at}");
        at += 1_000;
        combat.on(ev(fields), "flow", &identity, at, None, Some(&roster));
    };
    let staff = [1i16, 0, 0, 3, 0, 0, 0, 0, 0, 0];
    let heal = |source: i32| p![252 => 6, 0 => 99, 6 => source, 2 => 10f32, 7 => 1i16];
    event(&mut combat, p![252 => 29, 0 => 10, 1 => "One", 7 => one, 40 => staff]);
    event(&mut combat, heal(10));
    event(&mut combat, p![252 => 74, 0 => "two", 1 => "hi", 4 => 20, 5 => two]);
    event(&mut combat, heal(20));
    event(&mut combat, heal(20));
    event(&mut combat, heal(20));
    event(&mut combat, p![252 => 244, 0 => two, 2 => staff]);
    event(&mut combat, p![252 => 74, 0 => "three", 1 => "hi", 4 => 30, 5 => three]);
    event(&mut combat, heal(30));
    event(&mut combat, p![252 => 90, 0 => 30, 2 => staff]);
    event(&mut combat, heal(30));
    combat.flush_unidentified();
    let snapshot = combat.snapshot().unwrap();
    let items = snapshot.item_contributions.as_ref().unwrap();
    let only = |name: &str| {
        let found: Vec<_> = items.iter().filter(|i| i.player_name == name).collect();
        found
    };
    let first = only("One");
    assert!(first.len() == 1 && first[0].item_index == Some(1) && first[0].slot == "mainhand" && first[0].kind == "ability" && first[0].healing == 10.0);
    let second = only("Two");
    assert!(second.len() == 1 && second[0].item_index == Some(1) && second[0].healing == 30.0 && diagnostic(&combat, "reattributedFromTimeline") == Some(3));
    let third = only("Three");
    let family: Vec<_> = third.iter().filter(|i| !i.tier_observed).collect();
    let exact: Vec<_> = third.iter().filter(|i| i.tier_observed).collect();
    assert!(family.len() == 1 && family[0].item_index == Some(1) && family[0].healing == 10.0 && exact.len() == 1 && exact[0].healing == 10.0);
    assert_eq!(items.iter().map(|i| i.healing).sum::<f64>(), snapshot.total_healing);
    assert_eq!(combat.timeline().iter().filter(|e| e.kind == "gear").count(), 3);
}

#[test]
fn skills_chosen_on_the_local_characters_items_explain_its_effects_and_nothing_else_guesses_a_source() {
    let catalog = catalog(
        vec![item(7587, "T4_MAIN_HOLYSTAFF@4", "Adept's Holy Staff", "mainhand", 4, 4, NONE, NONE), item(7594, "T6_MAIN_HOLYSTAFF@1", "Master's Holy Staff", "mainhand", 6, 1, NONE, NONE)],
        vec![spell(3404, "HOLYFLASH", &[]), spell(3409, "HEALINGBEAM", &["HEALINGBEAM_EFFECT"]), spell(3413, "HEALINGBEAM_EFFECT", &[])],
    );
    let (local_id, healer_id) = (new_id(), new_id());
    let mut local = local(local_id, 71, "Alpha", "one", 10, "flow");
    let roster = roster(&[(local_id, "Alpha"), (healer_id, "Healer")], 1, true);
    let mut combat = CapturedCombatObservation::new(Some(catalog));
    combat.begin_session(Some(1), None);
    event_at(&mut combat, p![252 => 30, 0 => 302, 1 => 7587, 9 => [3404i16, 3409, 3420]], 9_000_000, Some(&local), Some(&roster));
    local.observed_at_microseconds = 10_000_000;
    combat.on(WireMessage::new(3, 2, Some(0), p![253 => 2, 0 => 71, 52 => [302i16]]), "flow", "join", 10_000_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 71, 2 => 200f32, 7 => 3413i16], 11_000_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 71, 2 => 100f32, 7 => 3404i16], 11_100_000, Some(&local), Some(&roster));
    let snapshot = combat.snapshot().unwrap();
    let own: Vec<_> = snapshot.item_contributions.as_ref().unwrap().iter().filter(|i| i.player_name == "Alpha").collect();
    assert!(own.len() == 1 && own[0].item_index == Some(7587) && own[0].slot == "mainhand" && own[0].kind == "ability" && own[0].healing == 300.0);
    event_at(&mut combat, p![252 => 74, 0 => "healer", 1 => "hi", 4 => 20, 5 => healer_id], 12_000_000, Some(&local), Some(&roster));
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 20, 2 => 150f32, 7 => 3413i16], 12_100_000, Some(&local), Some(&roster));
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.item_contributions.unwrap().iter().find(|i| i.player_name == "Healer").unwrap().item_index.is_none());
}

#[test]
fn after_a_capture_restart_the_local_weapon_is_proven_by_its_durability_dropping_in_use_back_to_the_captures_start() {
    let catalog = catalog(
        vec![item(9022, "T6_MAIN_HAMMER", "Master's Hammer", "mainhand", 6, 0, EMPTY, Some(&["IRONBREAKER"])), item(9030, "T6_2H_HAMMER", "Master's Great Hammer", "mainhand", 6, 0, EMPTY, Some(&["IRONBREAKER"]))],
        vec![spell(3342, "IRONBREAKER", &[])],
    );
    let local_id = new_id();
    let local = local(local_id, 393, "Alpha", "one", 1, "flow");
    let mut combat = CapturedCombatObservation::new(Some(catalog));
    combat.begin_session(Some(1), None);
    combat.forget_identities(5_000_000);
    combat.observe_own_cast(&req(p![253 => 24, 0 => 555i64, 4 => 3342, 8 => 1]), 6_000_000);
    event_at(&mut combat, p![252 => 18, 0 => 393, 1 => 30, 2 => 3342, 4 => 1, 5 => 555i64], 6_050_000, Some(&local), None);
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 393, 2 => -50f32, 5 => 0u8, 7 => -1i16], 6_100_000, Some(&local), None);
    event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 393, 2 => -200f32, 7 => 3342i16], 6_200_000, Some(&local), None);
    assert!(combat.snapshot().unwrap().item_contributions.unwrap().iter().all(|i| i.item_index.is_none()));
    event_at(&mut combat, p![252 => 30, 0 => 395, 1 => 9022, 8 => 303_190_000i64, 9 => [3342i16]], 7_000_000, Some(&local), None);
    assert!(combat.snapshot().unwrap().item_contributions.unwrap().iter().all(|i| i.item_index.is_none()));
    event_at(&mut combat, p![252 => 30, 0 => 395, 1 => 9022, 8 => 303_180_000i64, 9 => [3342i16]], 8_000_000, Some(&local), None);
    let items = combat.snapshot().unwrap().item_contributions.unwrap();
    let own: Vec<_> = items.iter().filter(|i| i.player_name == "Alpha").collect();
    assert!(own.len() == 2 && own.iter().all(|i| i.item_index == Some(9022)) && own.iter().map(|i| i.damage).sum::<f64>() == 250.0);
    assert_eq!(combat.weapons().get(&local_id).copied(), Some(Some(9022)));
}

#[test]
fn a_join_after_a_capture_restart_proves_the_equipment_since_the_restart_unless_the_equipment_changed_in_between() {
    let catalog = catalog(
        vec![
            item(4308, "T4_ARMOR_LEATHER_MORGANA", "Adept's Stalker Jacket", "armor", 4, 0, EMPTY, Some(&["STORMSHIELD"])),
            item(4310, "T5_ARMOR_LEATHER_MORGANA", "Expert's Stalker Jacket", "armor", 5, 0, EMPTY, Some(&["STORMSHIELD"])),
        ],
        vec![spell(3952, "STORMSHIELD", &[])],
    );
    for changed in [false, true] {
        let local_id = new_id();
        let mut local = local(local_id, 393, "Alpha", "one", 1, "flow");
        let mut combat = CapturedCombatObservation::new(Some(catalog.clone()));
        combat.begin_session(Some(1), None);
        combat.forget_identities(5_000_000);
        combat.observe_own_cast(&req(p![253 => 24, 0 => 555i64, 4 => 3952, 8 => 1]), 6_000_000);
        event_at(&mut combat, p![252 => 18, 0 => 393, 1 => 30, 2 => 3952, 4 => 1, 5 => 555i64], 6_050_000, Some(&local), None);
        event_at(&mut combat, p![252 => 6, 0 => 30, 6 => 393, 2 => -300f32, 7 => 3952i16], 6_100_000, Some(&local), None);
        let chest = [0i16, 0, 0, 4308, 0, 0, 0, 0, 0, 0];
        if changed {
            event_at(&mut combat, p![252 => 90, 0 => 393, 2 => chest], 7_000_000, Some(&local), None);
        }
        event_at(&mut combat, p![252 => 90, 0 => 900, 2 => chest], 9_998_000, Some(&local), None);
        event_at(&mut combat, p![252 => 30, 0 => 903, 1 => 4308, 8 => 161_700_000i64], 9_999_000, Some(&local), None);
        local = LocalCharacterSnapshot { object_id: Some(900), observed_at_microseconds: 10_000_000, ..local };
        combat.on(WireMessage::new(3, 2, Some(0), p![253 => 2, 0 => 900i64, 52 => longs(&[0, 0, 0, 903, 0, 0, 0, 0, 0, 0])]), "flow", "join", 10_000_000, Some(&local), None);
        let items = combat.snapshot().unwrap().item_contributions.unwrap();
        let own: Vec<_> = items.iter().filter(|i| i.player_name == "Alpha").collect();
        assert_eq!(own.len(), 1);
        if changed {
            assert!(!own[0].tier_observed);
        } else {
            assert!(own[0].item_index == Some(4308) && own[0].damage == 300.0 && own[0].tier_observed);
        }
    }
}

#[test]
fn the_item_moves_of_the_first_equipment_change_prove_the_equipment_worn_since_a_capture_restart() {
    let catalog = catalog(
        vec![
            item(9022, "T6_MAIN_HAMMER", "Master's Hammer", "mainhand", 6, 0, EMPTY, Some(&["IRONBREAKER"])),
            item(9030, "T7_MAIN_HAMMER", "Grandmaster's Hammer", "mainhand", 7, 0, EMPTY, Some(&["IRONBREAKER"])),
            item(4308, "T4_ARMOR_LEATHER_MORGANA", "Adept's Stalker Jacket", "armor", 4, 0, EMPTY, Some(&["STORMSHIELD"])),
            item(4310, "T5_ARMOR_LEATHER_MORGANA", "Expert's Stalker Jacket", "armor", 5, 0, EMPTY, Some(&["STORMSHIELD"])),
        ],
        vec![spell(3342, "IRONBREAKER", &[]), spell(3952, "STORMSHIELD", &[])],
    );
    let (equipment, bag) = (new_id(), new_id());
    for requested in [true, false] {
        let local_id = new_id();
        let local = local(local_id, 393, "Alpha", "one", 1, "flow");
        let mut combat = CapturedCombatObservation::new(Some(catalog.clone()));
        combat.begin_session(Some(1), None);
        let at = |combat: &mut CapturedCombatObservation, fields, t: i64| event_at(combat, fields, t, Some(&local), None);
        combat.on(WireMessage::new(3, 2, Some(0), p![253 => 2, 0 => 393i64, 51 => equipment, 54 => bag]), "flow", "join", 500_000, Some(&local), None);
        combat.forget_identities(5_000_000);
        combat.observe_own_cast(&req(p![253 => 24, 0 => 555i64, 4 => 3342, 8 => 1]), 6_000_000);
        at(&mut combat, p![252 => 18, 0 => 393, 1 => 30, 2 => 3342, 4 => 1, 5 => 555i64], 6_050_000);
        at(&mut combat, p![252 => 6, 0 => 30, 6 => 393, 2 => -100f32, 5 => 0u8, 7 => -1i16], 6_100_000);
        at(&mut combat, p![252 => 6, 0 => 30, 6 => 393, 2 => -300f32, 7 => 3952i16], 6_200_000);
        if requested {
            combat.observe_own_cast(&req(p![253 => 30, 1 => equipment, 2 => 2, 4 => bag, 5 => 2]), 7_000_000);
        }
        at(&mut combat, p![252 => 26, 0 => 425, 2 => bag, 3 => 2], 7_200_000);
        at(&mut combat, p![252 => 90, 0 => 393, 2 => [0i16, 0, 0, 4308, 0, 0, 0, 0, 0, 0]], 7_200_000);
        combat.observe_own_cast(&req(p![253 => 30, 1 => bag, 2 => 3, 4 => equipment, 5 => 3]), 7_400_000);
        at(&mut combat, p![252 => 26, 0 => 425, 2 => equipment, 3 => 3], 7_600_000);
        at(&mut combat, p![252 => 90, 0 => 393, 2 => [9022i16, 0, 0, 4308, 0, 0, 0, 0, 0, 0]], 7_600_000);
        let items = combat.snapshot().unwrap().item_contributions.unwrap();
        let own: Vec<_> = items.iter().filter(|i| i.player_name == "Alpha").collect();
        let chest: Vec<_> = own.iter().filter(|i| i.slot == "chest" || i.slot == "armor").collect();
        assert!(chest.len() == 1 && chest[0].item_index == Some(4308) && chest[0].tier_observed && chest[0].damage == 300.0);
        let hammer: Vec<_> = own.iter().filter(|i| i.kind == "autoAttack").collect();
        assert_eq!(hammer.len(), 1);
        if requested {
            assert!(hammer[0].item_index == Some(9022) && hammer[0].damage == 100.0);
        } else {
            assert!(hammer[0].item_index.is_none());
        }
    }
}

#[test]
fn a_swap_after_a_join_is_kept_when_a_later_item_report_arrives_for_an_item_the_join_listed() {
    let catalog = catalog(
        vec![
            item(9022, "T6_MAIN_HAMMER", "Master's Hammer", "mainhand", 6, 0, EMPTY, Some(&["IRONBREAKER"])),
            item(4308, "T4_ARMOR_LEATHER_MORGANA", "Adept's Stalker Jacket", "armor", 4, 0, EMPTY, Some(&["STORMSHIELD"])),
            item(4309, "T4_ARMOR_LEATHER_MORGANA@1", "Adept's Stalker Jacket", "armor", 4, 1, EMPTY, Some(&["STORMSHIELD"])),
        ],
        vec![spell(3342, "IRONBREAKER", &[]), spell(3952, "STORMSHIELD", &[])],
    );
    let (equipment, bag, local_id) = (new_id(), new_id(), new_id());
    let local = local(local_id, 400, "Alpha", "one", 1_000_000, "flow");
    let mut combat = CapturedCombatObservation::new(Some(catalog));
    combat.begin_session(Some(1), None);
    let at = |combat: &mut CapturedCombatObservation, fields, t: i64| event_at(combat, fields, t, Some(&local), None);
    at(&mut combat, p![252 => 90, 0 => 400, 2 => [9022i16, 0, 0, 4308, 0, 0, 0, 0, 0, 0]], 999_000);
    at(&mut combat, p![252 => 30, 0 => 403, 1 => 9022, 8 => 1000i64], 999_500);
    at(&mut combat, p![252 => 30, 0 => 405, 1 => 4308, 8 => 1000i64], 999_600);
    combat.on(
        WireMessage::new(3, 2, Some(0), p![253 => 2, 0 => 400i64, 51 => equipment, 52 => longs(&[403, 0, 0, 405, 0, 0, 0, 0, 0, 0]), 54 => bag]),
        "flow",
        "join",
        1_000_000,
        Some(&local),
        None,
    );
    at(&mut combat, p![252 => 6, 0 => 30, 6 => 400, 2 => -100f32, 7 => 3952i16], 2_000_000);
    at(&mut combat, p![252 => 26, 0 => 407, 1 => 3, 2 => equipment, 3 => 3], 3_000_000);
    at(&mut combat, p![252 => 90, 0 => 400, 2 => [9022i16, 0, 0, 4309, 0, 0, 0, 0, 0, 0]], 3_000_000);
    at(&mut combat, p![252 => 26, 0 => 405, 2 => bag, 3 => 3], 3_000_000);
    at(&mut combat, p![252 => 30, 0 => 403, 1 => 9022, 8 => 990i64], 3_700_000);
    at(&mut combat, p![252 => 6, 0 => 30, 6 => 400, 2 => -200f32, 7 => 3952i16], 4_000_000);
    let items = combat.snapshot().unwrap().item_contributions.unwrap();
    let damage = |index: i32| items.iter().filter(|i| i.player_name == "Alpha" && i.item_index.unwrap_or(0) == index).map(|i| i.damage).sum::<f64>();
    assert!(damage(4308) == 100.0 && damage(4309) == 200.0);
}

#[test]
fn each_potion_enchantments_own_spell_names_the_exact_potion() {
    let catalog = catalog(
        vec![
            item(598, "T4_POTION_COOLDOWN", "Minor Poison Potion", "potion", 4, 0, EMPTY, Some(&["POTION_POISON_P1"])),
            item(599, "T4_POTION_COOLDOWN@1", "Minor Poison Potion", "potion", 4, 1, EMPTY, Some(&["POTION_POISON_P2"])),
        ],
        vec![spell(4395, "POTION_POISON_P1", &[]), spell(4400, "POTION_POISON_P2", &[])],
    );
    let first = catalog.unique_family(4395).unwrap();
    assert!(first.index == Some(598) && first.slot == "potion" && first.exact);
    let second = catalog.unique_family(4400).unwrap();
    assert!(second.index == Some(599) && second.exact);
}

/// The C# check imports encrypted game XML through GameAssetImporter (not ported); this keeps its catalog half with
/// the data that import produces.
#[test]
fn imported_area_effects_retain_the_weapon_source_of_ironbreaker_damage() {
    let sources = catalog(
        vec![item(1, "T6_MAIN_HAMMER", "T6_MAIN_HAMMER", "mainhand", 6, 0, EMPTY, NONE)],
        vec![spell(0, "IRONBREAKER", &["IRONBREAKER_EFFECT"]), spell(1, "IRONBREAKER_EFFECT", &[])],
    );
    let resolved = sources.resolve(Some(1), Some(2), true, &some(&[1]), &some(&[0]));
    assert!(resolved.index == Some(1) && resolved.slot == "mainhand" && resolved.kind == "ability");
    assert!(sources.resolve(Some(1), Some(2), true, &some(&[1]), &[]).index.is_none());
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let mut combat = CapturedCombatObservation::new(Some(sources));
    combat.on(ev(p![252 => 90, 0 => 10, 2 => [1i16], 7 => [0i16]]), "flow", "equipment", 1, Some(&local), None);
    combat.on(ev(p![252 => 6, 0 => 30, 2 => -171f32, 6 => 10, 7 => 1, 5 => 2u8]), "flow", "hit", 2, Some(&local), None);
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(snapshot.total_damage, 171.0);
    let items = snapshot.item_contributions.as_ref().unwrap();
    assert!(items.len() == 1 && items[0].damage == 171.0 && items[0].item_index == Some(1) && items[0].slot == "mainhand" && items[0].kind == "ability");
    assert!(snapshot.timeline.len() == 1 && snapshot.timeline[0].item_damage.as_ref().unwrap().values().sum::<f64>() == 171.0);
}

#[test]
fn ranged_and_melee_auto_attacks_retain_their_type_apart_from_abilities_and_cape_procs() {
    let sources = catalog(vec![item(2637, "T4_CAPEITEM_FW_THETFORD@3", "Thetford Cape", "cape", 4, 3, Some(&["CAPE"]), NONE)], vec![spell(214, "CAPE", &[]), spell(3215, "FIRE", &[])]);
    let equipment = some(&[7108, 0, 0, 0, 0, 0, 2637]);
    assert_eq!(sources.resolve(Some(-1), Some(1), true, &equipment, &[]).kind, "autoAttack");
    assert_eq!(sources.resolve(Some(-1), Some(0), true, &equipment, &[]).kind, "autoAttack");
    assert_eq!(sources.resolve(Some(-1), Some(1), false, &equipment, &[]).kind, "unknown");
    assert_eq!(sources.resolve(Some(-1), Some(5), true, &equipment, &[]).kind, "unknown");
    assert_eq!(sources.resolve(None, Some(1), true, &equipment, &[]).kind, "unknown");
    let bare = sources.resolve(Some(-1), Some(1), true, &[], &[]);
    assert!(bare.kind == "autoAttack" && bare.index.is_none());
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let mut combat = CapturedCombatObservation::new(Some(sources));
    combat.on(ev(p![252 => 90, 0 => 10, 2 => [7108i16, 0, 0, 0, 0, 0, 2637], 7 => [3215i16]]), "flow", "equipment", 1, Some(&local), None);
    let batch = ev(p![252 => 7, 0 => 30, 2 => floats(&[-32.0, -32.0, -32.0, -32.0, -106.0]), 6 => [10i16, 10, 10, 10, 10], 7 => [-1i16, -1, -1, 3215, 214], 5 => vec![1u8, 1, 1, 3, 2]]);
    combat.observe(&batch, &c("flow"), &"hits".into(), 2, Some(&local), None);
    combat.observe(&batch, &c("flow"), &"hits".into(), 2, Some(&local), None);
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(snapshot.total_damage, 234.0);
    let items = snapshot.item_contributions.as_ref().unwrap();
    let auto: Vec<_> = items.iter().filter(|i| i.kind == "autoAttack").collect();
    assert!(auto.len() == 1 && auto[0].damage == 96.0 && auto[0].item_index == Some(7108) && auto[0].slot == "mainhand");
    assert_eq!(items.iter().find(|i| i.kind == "ability").unwrap().damage, 32.0);
    assert_eq!(items.iter().find(|i| i.kind == "passive").unwrap().damage, 106.0);
    assert_eq!(items.iter().map(|i| i.damage).sum::<f64>(), snapshot.total_damage);
    assert_eq!(snapshot.timeline[0].item_damage.as_ref().unwrap().values().sum::<f64>(), snapshot.total_damage);
}

#[test]
fn cape_passive_chains_resolve_only_for_the_observed_equipped_cape() {
    let sources = catalog(
        vec![item(2500, "T4_CAPEITEM_FW_THETFORD", "Thetford Cape", "cape", 4, 0, Some(&["PASSIVE_CAPE_THETFORD"]), NONE)],
        vec![spell(100, "PASSIVE_CAPE_THETFORD", &["CHAIN"]), spell(101, "CHAIN", &["EFFECT"]), spell(102, "EFFECT", &[])],
    );
    let equipment = some(&[7108, 0, 0, 0, 0, 0, 2500]);
    let cape = sources.resolve(Some(102), Some(3), true, &equipment, &[]);
    assert!(cape.index == Some(2500) && cape.slot == "cape");
    assert!(sources.resolve(Some(999), Some(3), true, &equipment, &[]).index.is_none());
    assert!(sources.resolve(Some(102), Some(3), true, &some(&[7108]), &[]).index.is_none());
    assert!(sources.resolve(Some(102), Some(3), true, &some(&[7108, 0, 0, 0, 0, 0, 9999]), &[]).index.is_none());
    let auto = sources.resolve(Some(-1), Some(0), true, &equipment, &[]);
    assert!(auto.index == Some(7108) && auto.slot == "mainhand");
    assert!(sources.resolve(Some(102), Some(3), true, &equipment, &some(&[100])).index.is_none());
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let mut combat = CapturedCombatObservation::new(Some(sources));
    combat.on(ev(p![252 => 90, 0 => 10, 2 => [7108i16, 0, 0, 0, 0, 0, 2500]]), "flow", "equipment", 1, Some(&local), None);
    combat.on(
        ev(p![252 => 7, 0 => 30, 2 => objects(vec![(-32f32).into(), (-138f32).into()]), 6 => ints(&[10, 10]), 7 => ints(&[-1, 102]), 5 => ints(&[0, 3])]),
        "flow",
        "hits",
        2,
        Some(&local),
        None,
    );
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(snapshot.total_damage, 170.0);
    let items = snapshot.item_contributions.as_ref().unwrap();
    assert_eq!(items.iter().find(|i| i.slot == "cape").unwrap().damage, 138.0);
    assert_eq!(items.iter().find(|i| i.slot == "mainhand").unwrap().damage, 32.0);
    assert_eq!(snapshot.timeline[0].item_damage.as_ref().unwrap().values().sum::<f64>(), 170.0);
}

#[test]
fn item_attribution_follows_chest_effects_without_counting_them_as_weapon_damage() {
    let sources = catalog(vec![], vec![spell(3952, "STORMSHIELD", &["STORMSHIELD_EFFECT"]), spell(3953, "STORMSHIELD_EFFECT", &[]), spell(3359, "HAMMERWHIRLWIND2", &[])]);
    let equipment = some(&[9022, 0, 0, 4308, 0]);
    let spells = some(&[-1, -1, 3359, 3952]);
    let chest = sources.resolve(Some(3953), Some(3), true, &equipment, &spells);
    assert!(chest.index == Some(4308) && chest.slot == "chest");
    let weapon = sources.resolve(Some(3359), Some(3), true, &equipment, &spells);
    assert!(weapon.index == Some(9022) && weapon.slot == "mainhand");
    let auto = sources.resolve(Some(-1), Some(0), true, &equipment, &spells);
    assert!(auto.index == Some(9022) && auto.slot == "mainhand");
    assert!(sources.resolve(Some(-1), Some(8), false, &equipment, &spells).index.is_none());
    assert!(sources.resolve(Some(9999), Some(3), true, &equipment, &spells).index.is_none());
    assert!(sources.resolve(Some(3953), Some(3), true, &equipment, &some(&[3952, -1, -1, 3952])).index.is_none());
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let mut combat = CapturedCombatObservation::new(Some(sources));
    combat.on(ev(p![252 => 90, 0 => 10, 2 => [9022i16, 0, 0, 4308, 0], 7 => [-1i16, -1, 3359, 3952]]), "flow", "equipment", 1, Some(&local), None);
    combat.on(
        ev(p![252 => 7, 0 => 30, 2 => objects(vec![(-40f32).into(), (-32f32).into()]), 6 => ints(&[10, 10]), 7 => ints(&[-1, 3953]), 5 => ints(&[0, 3])]),
        "flow",
        "hits",
        2,
        Some(&local),
        None,
    );
    let snapshot = combat.snapshot().unwrap();
    assert_eq!(snapshot.total_damage, 72.0);
    let items = snapshot.item_contributions.as_ref().unwrap();
    assert_eq!(items.iter().find(|i| i.slot == "chest").unwrap().damage, 32.0);
    assert_eq!(items.iter().find(|i| i.slot == "mainhand").unwrap().damage, 40.0);
    assert_eq!(snapshot.timeline[0].item_damage.as_ref().unwrap().values().sum::<f64>(), 72.0);
    let weapons = snapshot.weapon_contributions.as_ref().unwrap();
    assert!(weapons.len() == 1 && weapons[0].damage == 72.0);
}

#[test]
fn weapon_contributions_retain_the_weapon_at_each_hit_across_swaps_and_same_second_events() {
    let id = new_id();
    let local = local(id, 10, "Alpha", "map", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    let equipment = |combat: &mut CapturedCombatObservation, index: i16, at: i64| {
        combat.on(ev(p![252 => 90, 0 => 10, 2 => [index]]), "flow", &format!("equipment:{at}"), at, Some(&local), None)
    };
    let hit = |combat: &mut CapturedCombatObservation, amount: f32, identity: &str, at: i64| {
        combat.on(ev(p![252 => 6, 0 => 30, 6 => 10, 2 => amount]), "flow", identity, at, Some(&local), None)
    };
    equipment(&mut combat, 9022, 1_000_000);
    hit(&mut combat, -100.0, "one", 1_100_000);
    equipment(&mut combat, 7108, 1_200_000);
    hit(&mut combat, -200.0, "two", 1_300_000);
    hit(&mut combat, 40.0, "heal", 1_400_000);
    equipment(&mut combat, 9022, 1_500_000);
    hit(&mut combat, -50.0, "three", 1_600_000);
    hit(&mut combat, -50.0, "three", 1_600_000);
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.total_damage == 350.0 && snapshot.total_healing == 40.0);
    let weapons = snapshot.weapon_contributions.as_ref().unwrap();
    assert_eq!(weapons.iter().find(|w| w.weapon_index == Some(9022)).unwrap().damage, 150.0);
    let staff = weapons.iter().find(|w| w.weapon_index == Some(7108)).unwrap();
    assert!(staff.damage == 200.0 && staff.healing == 40.0);
    assert_eq!(snapshot.timeline.len(), 1);
    assert_eq!(snapshot.timeline[0].weapon_damage.as_ref().unwrap().values().sum::<f64>(), 350.0);
    assert_eq!(snapshot.timeline[0].weapon_healing.as_ref().unwrap().values().sum::<f64>(), 40.0);
    let mut copy = snapshot.timeline[0].clone();
    Arc::make_mut(&mut copy).weapon_damage.as_mut().unwrap().clear();
    assert_eq!(combat.snapshot().unwrap().timeline[0].weapon_damage.as_ref().unwrap().values().sum::<f64>(), 350.0);
}

#[test]
fn captured_party_membership_survives_zoning_and_applies_leave_and_rejoin() {
    let (local_id, other_id) = (new_id(), new_id());
    let local = local(local_id, 1, "Alpha", "1301", 1, "one");
    let mut roster = PartyRosterObservation::new();
    let join = ev(p![252 => 230i16, 8 => guid_bytes(&[local_id, other_id]), 9 => strings(&["Alpha", "Beta"])]);
    roster.observe(&join, Some(&local), 2);
    assert!(roster.snapshot().is_some_and(|s| s.complete && s.members.len() == 2));
    let moved = LocalCharacterSnapshot { map_id: Some("1302".into()), connection: c("two"), ..local.clone() };
    roster.observe(&resp(p![253 => 2]), Some(&moved), 3);
    assert_eq!(roster.snapshot().unwrap().members.len(), 2);
    roster.observe(&ev(p![252 => 234, 1 => other_id]), Some(&local), 4);
    assert_eq!(roster.snapshot().unwrap().members.len(), 1);
    roster.observe(&join, Some(&local), 2);
    assert_eq!(roster.snapshot().unwrap().members.len(), 1);
    roster.observe(&ev(p![252 => 232, 1 => other_id, 2 => "Beta"]), Some(&local), 5);
    assert_eq!(roster.snapshot().unwrap().members.len(), 2);
    roster.observe(&ev(p![252 => 234, 1 => local_id]), Some(&local), 6);
    assert!(roster.snapshot().unwrap().members.is_empty());
}

#[test]
fn malformed_and_unrelated_party_rosters_cannot_replace_known_membership() {
    let id = new_id();
    let local = local(id, 1, "Alpha", "1301", 1, "one");
    let mut roster = PartyRosterObservation::new();
    roster.observe(&ev(p![252 => 230, 8 => id, 9 => strings(&["Alpha"])]), Some(&local), 1);
    roster.observe(&ev(p![252 => 230, 8 => vec![0u8; 17], 9 => strings(&["Invalid"])]), Some(&local), 2);
    roster.observe(&ev(p![252 => 230, 8 => new_id(), 9 => strings(&["Other"])]), Some(&local), 3);
    let members = &roster.snapshot().unwrap().members;
    assert!(members.len() == 1 && members[0].id == id);
    let other = LocalCharacterSnapshot { character_id: new_id(), ..local.clone() };
    roster.observe(&resp(vec![]), Some(&other), 4);
    assert!(roster.snapshot().is_none());
}

#[test]
fn local_identity_requires_a_successful_join_and_ignores_nearby_players() {
    let mut state = LocalCharacterObservation::new();
    let id = new_id();
    let join = resp(p![253 => 2, 1 => id, 2 => "Alpha", 8 => "1301", 58 => "Guild", 79 => "Alliance"]);
    let server = c("server");
    state.observe(&WireMessage { return_code: Some(1), ..join.clone() }, &server, 1);
    assert!(state.snapshot().is_none());
    state.observe(&join, &server, 2);
    let snapshot = state.snapshot().unwrap();
    assert!(snapshot.name == "Alpha" && snapshot.guild.as_deref() == Some("Guild") && snapshot.alliance.as_deref() == Some("Alliance") && snapshot.map_id.as_deref() == Some("1301"));
    state.observe(&ev(p![252 => 29, 7 => new_id(), 8 => "Other"]), &server, 3);
    assert_eq!(state.snapshot().unwrap().guild.as_deref(), Some("Guild"));
    state.observe(&ev(p![252 => 29, 7 => id, 8 => "", 51 => ""]), &server, 4);
    assert!(state.snapshot().unwrap().guild.as_deref() == Some("") && state.snapshot().unwrap().alliance.as_deref() == Some(""));
    state.observe(&join, &server, 1);
    assert_eq!(state.snapshot().unwrap().guild.as_deref(), Some(""));
}

#[test]
fn zone_transitions_invalidate_the_old_map_until_a_new_join() {
    let mut state = LocalCharacterObservation::new();
    state.observe(&resp(p![253 => 2, 1 => new_id(), 2 => "Alpha", 8 => "1301"]), &c("server"), 1);
    let change = resp(p![253 => 41, 0 => "1302"]);
    state.observe(&change, &c("other"), 2);
    assert_eq!(state.snapshot().unwrap().map_id.as_deref(), Some("1301"));
    state.observe(&change, &c("server"), 3);
    let snapshot = state.snapshot().unwrap();
    assert!(snapshot.map_id.is_none() && snapshot.destination_map_id.as_deref() == Some("1302") && snapshot.location_state == "changing");
    state.observe(&resp(p![253 => 2, 1 => new_id(), 2 => "Beta", 8 => "1302"]), &c("next"), 4);
    let snapshot = state.snapshot().unwrap();
    assert!(snapshot.name == "Beta" && snapshot.map_id.as_deref() == Some("1302") && snapshot.guild.is_none() && snapshot.alliance.is_none() && snapshot.destination_map_id.is_none());
}

fn observed(name: &str, weapon: Option<&str>) -> ObservedCharacter {
    ObservedCharacter { id: new_id(), name: name.into(), weapon_id: weapon.map(Into::into) }
}

#[test]
fn a_zone_change_preserves_roster_but_invalidates_object_bindings() {
    let mut state = PartyObservation::new();
    let context = state.context();
    let member = observed("Alpha", Some("T4_MAIN_SWORD"));
    state.replace_party(context, 1, vec![member.clone()]).unwrap();
    state.observe(context, 2, 42, member.clone()).unwrap();
    assert_eq!(state.resolve(context, 42).unwrap().is_party_member, Some(true));
    let next = state.enter_zone();
    assert!(state.snapshot().members.len() == 1 && state.resolve(next, 42).is_none());
    assert!(!state.observe(context, 3, 42, member.clone()).unwrap());
    assert!(state.observe(next, 4, 93, member).unwrap());
    assert_eq!(state.resolve(next, 93).unwrap().is_party_member, Some(true));
}

#[test]
fn party_reconstruction_does_not_erase_the_current_weapon_or_binding() {
    let mut state = PartyObservation::new();
    let context = state.context();
    let member = observed("Alpha", Some("T4_MAIN_SWORD"));
    state.observe(context, 1, 42, member.clone()).unwrap();
    assert!(state.resolve(context, 42).unwrap().is_party_member.is_none());
    state.replace_party(context, 2, vec![ObservedCharacter { weapon_id: None, ..member.clone() }]).unwrap();
    assert_eq!(state.resolve(context, 42).unwrap().character.weapon_id.as_deref(), Some("T4_MAIN_SWORD"));
    state.replace_party(context, 3, vec![]).unwrap();
    assert_eq!(state.resolve(context, 42).unwrap().is_party_member, Some(false));
    state.replace_party(context, 4, vec![member]).unwrap();
    assert_eq!(state.resolve(context, 42).unwrap().is_party_member, Some(true));
}

#[test]
fn object_reuse_and_stale_observations_cannot_recover_an_old_identity() {
    let mut state = PartyObservation::new();
    let context = state.context();
    let (first, second) = (observed("Alpha", Some("Sword")), observed("Beta", Some("Staff")));
    state.observe(context, 5, 42, first.clone()).unwrap();
    state.observe(context, 6, 42, second.clone()).unwrap();
    assert!(!state.observe(context, 4, 42, first.clone()).unwrap());
    assert!(!state.observe(context, 4, 43, first.clone()).unwrap());
    assert_eq!(state.resolve(context, 42).unwrap().character.id, second.id);
    assert_eq!(state.snapshot().binding_conflicts, 1);
    let next = state.begin_connection();
    assert!(state.resolve(next, 42).is_none() && !state.snapshot().roster_complete);
    assert!(!state.observe(context, 99, 42, first).unwrap());
}

#[test]
fn invalid_roster_replacement_is_atomic_and_older_snapshots_are_rejected() {
    let mut state = PartyObservation::new();
    let context = state.context();
    let member = observed("Alpha", None);
    state.replace_party(context, 5, vec![member.clone()]).unwrap();
    assert!(!state.replace_party(context, 4, vec![]).unwrap());
    assert_eq!(state.replace_party(context, 6, vec![member.clone(), member]), Err(PartyError::InvalidRoster));
    assert_eq!(state.snapshot().members.len(), 1);
}

#[test]
fn protocol18_reads_scalar_event_values_and_compressed_integers() {
    let decoded = decode(&[0xf3, 4, 1, 4, 252, 11, 6, 0, 9, 84, 2, 12, 50, 6, 10, 86]);
    assert_eq!(decoded.status, "decodedMessage");
    let message = decoded.message.unwrap();
    assert_eq!(message.get(252), Some(&Value::Int(6)));
    assert_eq!(message.get(0), Some(&Value::Int(42)));
    assert_eq!(message.get(2), Some(&Value::Int(-50)));
    assert_eq!(message.get(6), Some(&Value::Long(43)));
}

#[test]
fn protocol18_decodes_party_names_arrays_and_guid_byte_arrays() {
    let result = decode(&[0xf3, 4, 1, 2, 8, 67, 2, 1, 2, 9, 71, 2, 1, 65, 1, 66]);
    assert_eq!(result.status, "decodedMessage");
    let message = result.message.unwrap();
    assert_eq!(message.get(8), Some(&Value::from(vec![1u8, 2])));
    assert_eq!(message.get(9), Some(&strings(&["A", "B"])));
}

#[test]
fn protocol18_distinguishes_unsupported_encrypted_and_malformed_messages() {
    assert_eq!(decode(&[0xf3, 0x84]).status, "encryptedMessage");
    assert_eq!(decode(&[0xf3, 4, 1, 1, 0, 255]).status, "unsupportedValueType");
    assert_eq!(decode(&[0xf3, 4, 1, 1, 0, 9, 255, 255, 255, 255, 127]).status, "malformedMessage");
    assert_eq!(decode(&[0xf3, 4, 1, 2, 0, 30, 0, 30]).status, "malformedMessage");
    assert_eq!(decode(&[0xf3, 4, 1, 0, 0]).status, "malformedMessage");
}

#[test]
fn protocol18_random_messages_cannot_bypass_limits_or_crash_parsing() {
    let mut state = 21u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..10_000 {
        let count = 2 + (next() % 298) as usize;
        let mut bytes: Vec<u8> = (0..count).map(|_| next() as u8).collect();
        bytes[0] = 0xf3;
        bytes[1] = 4;
        decode(&bytes);
    }
}

/// Encoding with the test kit and decoding again yields the same message, so the fixtures exercise the real decoder.
#[test]
fn protocol18_round_trips_the_fixture_encoder() {
    let message = WireMessage::new(
        3,
        7,
        Some(-2),
        p![0 => 1u8, 1 => -3i16, 2 => -70000, 3 => i64::MIN + 1, 4 => 1.5f32, 5 => -2.25f64, 6 => "é", 7 => vec![1u8, 2], 8 => strings(&["x"]), 9 => Value::bools(&[true, false, true]), 10 => Value::Null],
    );
    let decoded = decode(&meter_core::testkit::encode(&message));
    assert_eq!(decoded.status, "decodedMessage");
    assert_eq!(decoded.message.unwrap(), message);
}

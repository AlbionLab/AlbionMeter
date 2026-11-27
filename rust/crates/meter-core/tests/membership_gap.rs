//! tests/AlbionMeter.Tests/MembershipGapChecks.cs: effects waiting for a zone join or for membership proof.

mod common;

use common::*;

fn join(id: Guid, map: &str) -> WireMessage<'static> {
    resp(p![253 => 2, 0 => 10, 1 => id, 8 => map, 66 => "1315"])
}

#[test]
fn a_rejected_dungeon_entrance_restores_deferred_effects_to_the_current_floor() {
    let local_id = new_id();
    let local = local(local_id, 10, "Alpha", "@RANDOMDUNGEON@one", 1_000_000, "flow");
    let mut dungeon = DungeonCombatObservation::new(None);
    dungeon.on(join(local_id, "@RANDOMDUNGEON@one"), "flow", "join", 1_000_000, Some(&local), None);
    dungeon.observe_own_cast(&req(p![253 => 237]), 2_000_000);
    dungeon.on(ev(p![252 => 6, 0 => 900, 6 => 10, 2 => -100f32]), "flow", "deferred", 2_100_000, Some(&local), None);
    dungeon.on(WireMessage::new(3, 1, Some(1), p![253 => 237]), "flow", "rejected", 2_200_000, Some(&local), None);
    dungeon.on(ev(p![252 => 6, 0 => 900, 6 => 10, 2 => -50f32]), "flow", "continued", 3_000_000, Some(&local), None);
    dungeon.flush_unidentified();
    let runs = dungeon.snapshot();
    assert!(runs.len() == 1 && runs[0].floors.len() == 1 && runs[0].totals.total_damage == 150.0);
}

#[test]
fn paused_destination_effects_stay_excluded_after_the_floor_join_and_resume() {
    let (local_id, member_id) = (new_id(), new_id());
    let mut local = local(local_id, 10, "Alpha", "@RANDOMDUNGEON@one", 1_000_000, "old");
    let party = roster(&[(local_id, "Alpha"), (member_id, "Mate")], 1, true);
    let mut dungeon = DungeonCombatObservation::new(None);
    dungeon.on(join(local_id, "@RANDOMDUNGEON@one"), "old", "first", 1_000_000, Some(&local), Some(&party));
    dungeon.set_paused(true, 2_000_000);
    let event = |dungeon: &mut DungeonCombatObservation, local: &LocalCharacterSnapshot, fields, at: i64| {
        dungeon.on(ev(fields), "next", &format!("event:{at}"), at, Some(local), Some(&party))
    };
    event(&mut dungeon, &local, p![252 => 29, 0 => 402, 1 => "Mate", 7 => member_id], 5_000_000);
    event(&mut dungeon, &local, p![252 => 6, 0 => 900, 6 => 402, 2 => -100f32], 5_100_000);
    local = LocalCharacterSnapshot { connection: c("next"), map_id: Some("@RANDOMDUNGEON@two".into()), observed_at_microseconds: 5_300_000, ..local };
    dungeon.on(join(local_id, "@RANDOMDUNGEON@two"), "next", "second", 5_300_000, Some(&local), Some(&party));
    let floor = dungeon.snapshot().remove(0).floors.remove(1);
    assert!(floor.totals.total_damage == 0.0 && floor.totals.elapsed_seconds == 0.0);
    dungeon.set_paused(false, 6_000_000);
    event(&mut dungeon, &local, p![252 => 6, 0 => 900, 6 => 402, 2 => -50f32], 7_000_000);
    assert_eq!(dungeon.snapshot()[0].floors[1].totals.total_damage, 50.0);
}

#[test]
fn destination_traffic_before_join_belongs_to_the_new_floor_for_both_named_and_late_identified_players() {
    for reused_connection in [false, true] {
        for named_before_hit in [false, true] {
            let destination = if reused_connection { "old" } else { "next" };
            let (local_id, member_id, previous_actor_id) = (new_id(), new_id(), new_id());
            let mut local = local(local_id, 10, "Alpha", "@RANDOMDUNGEON@first", 1_000_000, "old");
            let party = roster(&[(local_id, "Alpha"), (member_id, "Mate"), (previous_actor_id, "Previous actor")], 1, true);
            let mut dungeon = DungeonCombatObservation::new(None);
            let event = |dungeon: &mut DungeonCombatObservation, local: &LocalCharacterSnapshot, connection: &str, fields, at: i64| {
                dungeon.on(ev(fields), connection, &format!("event:{at}"), at, Some(local), Some(&party))
            };
            let join_at = |dungeon: &mut DungeonCombatObservation, local: &mut LocalCharacterSnapshot, connection: &str, map: &str, at: i64| {
                *local = LocalCharacterSnapshot { connection: c(connection), map_id: Some(map.into()), observed_at_microseconds: at, ..local.clone() };
                dungeon.on(join(local_id, map), connection, &format!("join:{at}"), at, Some(local), Some(&party));
            };
            join_at(&mut dungeon, &mut local, "old", "@RANDOMDUNGEON@first", 1_000_000);
            event(&mut dungeon, &local, "old", p![252 => 29, 0 => 402, 1 => "Previous actor", 7 => previous_actor_id], 1_100_000);
            event(&mut dungeon, &local, "old", p![252 => 6, 0 => 900, 6 => 402, 2 => -40f32], 2_000_000);
            if reused_connection {
                dungeon.observe_own_cast(&req(p![253 => 237]), 4_900_000);
            }
            event(&mut dungeon, &local, destination, p![252 => 670, 0 => 402], 5_000_000);
            if named_before_hit {
                event(&mut dungeon, &local, destination, p![252 => 29, 0 => 402, 1 => "Mate", 7 => member_id], 5_010_000);
            }
            event(&mut dungeon, &local, destination, p![252 => 90, 0 => 402, 2 => [6661i16]], 5_050_000);
            event(&mut dungeon, &local, destination, p![252 => 6, 0 => 900, 6 => 402, 2 => -100f32], 5_100_000);
            event(&mut dungeon, &local, destination, p![252 => 6, 0 => 10, 6 => 402, 2 => 25f32], 5_150_000);
            assert_eq!(dungeon.snapshot()[0].floors[0].totals.total_damage, 40.0);
            join_at(&mut dungeon, &mut local, destination, "@RANDOMDUNGEON@second", 5_300_000);
            if !named_before_hit {
                event(&mut dungeon, &local, destination, p![252 => 29, 0 => 402, 1 => "Mate", 7 => member_id], 6_000_000);
            }
            dungeon.flush_unidentified();
            let run = dungeon.snapshot().remove(0);
            assert!(run.floors[0].totals.total_damage == 40.0 && run.floors[0].totals.total_healing == 0.0);
            assert_eq!(run.floors[0].totals.elapsed_seconds, 4.0);
            assert!(run.floors[1].totals.total_damage == 100.0 && run.floors[1].totals.total_healing == 25.0);
            let members = &run.floors[1].encounters[0].members;
            assert_eq!(members.iter().find(|m| m.id == member_id.to_string()).unwrap().damage, 100.0);
            assert_eq!(members.iter().find(|m| m.id == previous_actor_id.to_string()).unwrap().damage, 0.0);
            assert!(run.totals.total_damage == 140.0 && run.totals.total_healing == 25.0);
            dungeon.flush_unidentified();
            assert_eq!(dungeon.snapshot()[0].totals.total_damage, 140.0);
        }
    }
}

#[test]
fn editing_a_remembered_roster_does_not_confirm_the_remaining_members_in_a_new_capture() {
    let (local_id, member_id, leaving_id) = (new_id(), new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let mut party = PartyRosterObservation::new();
    assert!(party.restore(roster(&[(local_id, "Alpha"), (member_id, "Mate"), (leaving_id, "Leaving")], 1, true)));
    party.observe(&ev(p![252 => 234, 1 => leaving_id]), Some(&local), 5_000_000);
    let snapshot = party.snapshot().unwrap();
    assert!(snapshot.observed_at_microseconds == 5_000_000 && snapshot.members.len() == 2);
    assert!(!snapshot.is_confirmed_since(Some(1_000_000)));
    assert!(!snapshot.is_confirmed_since(None));
    party.observe(&ev(p![252 => 230, 8 => guid_bytes(&[local_id, member_id]), 9 => strings(&["Alpha", "Mate"])]), Some(&local), 6_000_000);
    assert!(party.snapshot().unwrap().is_confirmed_since(Some(1_000_000)));
    assert!(party.snapshot().unwrap().with_source("user-confirmed").is_confirmed_since(None));
}

#[test]
fn identifying_a_pending_actor_before_membership_proof_preserves_damage_and_healing_for_either_proof_type() {
    for full_list in [false, true] {
        let (local_id, member_id) = (new_id(), new_id());
        let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
        let mut combat = CapturedCombatObservation::new(None);
        combat.begin_session(Some(1), None);
        combat.forget_identities(1_000_000);
        let event = |combat: &mut CapturedCombatObservation, fields, at: i64, party: Option<&PartyRosterSnapshot>| {
            combat.on(ev(fields), "flow", &format!("event:{at}"), at, Some(&local), party)
        };
        event(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -100f32], 2_000_000, None);
        event(&mut combat, p![252 => 6, 0 => 10, 6 => 402, 2 => 25f32], 2_100_000, None);
        event(&mut combat, p![252 => 29, 0 => 402, 1 => "Mate", 7 => member_id], 3_000_000, None);
        let snapshot = combat.snapshot().unwrap();
        assert!(snapshot.total_damage == 0.0 && snapshot.total_healing == 0.0);
        if full_list {
            combat.set_party(Some(&roster(&[(local_id, "Alpha"), (member_id, "Mate")], 4_000_000, true)));
        } else {
            event(&mut combat, p![252 => 244, 0 => member_id, 2 => [6661i16, 0, 5347, 5005, 5397, 2900, 2662, 3098]], 4_000_000, None);
        }
        let snapshot = combat.snapshot().unwrap();
        assert!(snapshot.total_damage == 100.0 && snapshot.total_healing == 25.0);
        assert_eq!(snapshot.members.iter().find(|m| m.id == member_id.to_string()).unwrap().damage, 100.0);
        combat.flush_unidentified();
        let snapshot = combat.snapshot().unwrap();
        assert!(snapshot.total_damage == 100.0 && snapshot.total_healing == 25.0 && snapshot.event_count == 2);
    }
}

#[test]
fn identifying_an_outsider_never_carries_its_damage_into_a_party_joined_later() {
    let (local_id, member_id) = (new_id(), new_id());
    let local = local(local_id, 10, "Alpha", "dungeon", 1, "flow");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let alone = roster(&[(local_id, "Alpha")], 1, true);
    let event = |combat: &mut CapturedCombatObservation, fields, at: i64, party: &PartyRosterSnapshot| {
        combat.on(ev(fields), "flow", &format!("event:{at}"), at, Some(&local), Some(party))
    };
    event(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -100f32], 2_000_000, &alone);
    event(&mut combat, p![252 => 29, 0 => 402, 1 => "Mate", 7 => member_id], 3_000_000, &alone);
    let together = roster(&[(local_id, "Alpha"), (member_id, "Mate")], 4_000_000, true);
    combat.set_party(Some(&together));
    event(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -200f32], 3_500_000, &together);
    event(&mut combat, p![252 => 6, 0 => 900, 6 => 402, 2 => -50f32], 5_000_000, &together);
    combat.flush_unidentified();
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.total_damage == 50.0 && snapshot.event_count == 1);
}

#[test]
fn unidentified_destination_effects_and_equipment_before_join_remain_attributable_after_join() {
    let (local_id, member_id) = (new_id(), new_id());
    let mut local = local(local_id, 10, "Alpha", "old", 1, "old");
    let mut combat = CapturedCombatObservation::new(None);
    combat.begin_session(Some(1), None);
    let party = roster(&[(local_id, "Alpha"), (member_id, "Mate")], 1, true);
    let event = |combat: &mut CapturedCombatObservation, local: &LocalCharacterSnapshot, connection: &str, fields, at: i64| {
        combat.on(ev(fields), connection, &format!("event:{at}"), at, Some(local), Some(&party))
    };
    event(&mut combat, &local, "old", p![252 => 6, 0 => 900, 6 => 402, 2 => -500f32], 1_000_000);
    event(&mut combat, &local, "next", p![252 => 670, 0 => 402], 5_000_000);
    event(&mut combat, &local, "next", p![252 => 90, 0 => 402, 2 => [6661i16]], 5_050_000);
    event(&mut combat, &local, "next", p![252 => 6, 0 => 900, 6 => 402, 2 => -100f32], 5_100_000);
    event(&mut combat, &local, "next", p![252 => 6, 0 => 10, 6 => 402, 2 => 25f32], 5_150_000);
    local = LocalCharacterSnapshot { object_id: Some(432), connection: c("next"), observed_at_microseconds: 5_300_000, ..local };
    combat.on(resp(p![253 => 2, 0 => 432]), "next", "join", 5_300_000, Some(&local), Some(&party));
    assert_eq!(diagnostic(&combat, "pendingUnidentified"), Some(2));
    event(&mut combat, &local, "next", p![252 => 29, 0 => 402, 1 => "Mate", 7 => member_id], 6_000_000);
    combat.flush_unidentified();
    let snapshot = combat.snapshot().unwrap();
    assert!(snapshot.total_damage == 100.0 && snapshot.total_healing == 25.0);
    assert_eq!(snapshot.members.iter().find(|m| m.id == member_id.to_string()).unwrap().damage, 100.0);
    assert!(snapshot.members.iter().all(|m| !m.id.starts_with("unidentified:")));
    assert_eq!(snapshot.weapon_contributions.as_ref().unwrap().iter().find(|w| w.weapon_index == Some(6661)).unwrap().damage, 100.0);
    assert_eq!(diagnostic(&combat, "pendingUnidentified"), Some(0));
}

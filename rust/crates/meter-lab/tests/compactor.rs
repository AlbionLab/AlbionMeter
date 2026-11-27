//! Parity with the C# SessionCompactor and SuggestTitle: the fixtures are the C# outputs for the same inputs.

use meter_lab::{suggest_title, SessionCompactor, SourceDungeon, SourceFloor, SourceItem, SourceMember, SourceSecond, SourceSnapshot, SourceWeapon};
use meter_store::{json, DateTimeOffset};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()).unwrap()
}

fn member(id: &str, name: &str, membership: &str) -> SourceMember {
    SourceMember { id: id.into(), name: name.into(), weapon: "Great Hammer".into(), family: "hammer".into(), color: "#aabbcc".into(), membership: membership.into(), weapon_id: Some("T6_2H_HAMMER".into()) }
}

fn amounts(values: &[(&str, f64)]) -> Vec<(String, f64)> {
    values.iter().map(|(key, value)| ((*key).to_owned(), *value)).collect()
}

fn second(second: i32, damage: f64, healing: f64, sources: bool) -> SourceSecond {
    SourceSecond {
        second,
        damage: amounts(&[("a", damage), ("b", 0.0)]),
        healing: amounts(&[("b", healing)]),
        weapon_damage: sources.then(|| amounts(&[("a:9022", damage)])),
        weapon_healing: sources.then(|| amounts(&[("b:7108", healing), ("ghost:1", 3.4)])),
        item_damage: sources.then(|| amounts(&[("a:chest:4308", damage / 2.0), ("zz:x", 1.6)])),
        item_healing: None,
    }
}

fn record(started: DateTimeOffset, elapsed: f64, status: &str, members: &[SourceMember], timeline: Vec<SourceSecond>) -> SourceSnapshot {
    SourceSnapshot {
        id: "rec".into(),
        started_at: started,
        elapsed,
        status: status.into(),
        events: timeline.len() as i64 * 3,
        members: members.to_vec(),
        weapons: vec![
            SourceWeapon { id: "a:9022".into(), player_id: "a".into(), index: Some(9022), name: "Great Hammer".into(), unique_id: Some("T6_2H_HAMMER".into()) },
            SourceWeapon { id: "b:7108".into(), player_id: "b".into(), index: Some(7108), name: "Holy Staff".into(), unique_id: None },
        ],
        items: vec![SourceItem {
            id: "a:chest:4308".into(),
            player_id: "a".into(),
            slot: "chest".into(),
            index: Some(4308),
            name: "Cleric Robe".into(),
            kind: "ability".into(),
            tier_observed: true,
            unique_id: Some("T6_ARMOR_CLOTH_SET2".into()),
        }],
        timeline,
    }
}

fn plus(time: DateTimeOffset, seconds: i64) -> DateTimeOffset {
    DateTimeOffset { utc_ticks: time.utc_ticks + seconds * 10_000_000, ..time }
}

#[test]
fn deltas_match_the_csharp_compactor() {
    let t0 = DateTimeOffset::parse("2026-11-20T10:00:00.123+00:00").unwrap();
    let party = [member("a", "Alpha", "active"), member("b", "Beta", "active")];
    let left = [member("a", "Alpha", "active"), member("b", "Beta", "left"), member("c", "Gamma", "unconfirmed")];
    let floor = SourceSnapshot {
        id: "enc-1".into(),
        started_at: plus(t0, 1),
        elapsed: 1.0,
        status: "completed".into(),
        events: 2,
        members: vec![member("a", "Alpha 2", "active")],
        weapons: vec![],
        items: vec![],
        timeline: vec![second(0, 7.0, 0.0, false)],
    };
    let dungeons = [SourceDungeon {
        id: "run-1".into(),
        name: "Random dungeon".into(),
        started_at: t0,
        status: "running".into(),
        entrance: Some("Hightree Lake".into()),
        floors: vec![SourceFloor { id: "floor-1".into(), number: 1, map: Some("Floor A".into()), snaps: vec!["enc-1".into()] }],
    }];
    let paused = || record(t0, 3.2, "paused", &left, vec![second(0, 100.4, 20.5, true), second(1, 60.0, 0.0, true), second(3, 9.0, 9.0, true)]);
    let mut compactor = SessionCompactor::new();
    let mut deltas = vec![
        compactor.next("running", t0, Some("a"), &record(t0, 1.5, "running", &party, vec![second(0, 100.4, 20.5, true), second(1, 50.0, 0.0, true)]), &[], &[], Some("place-1")),
        compactor.next("running", t0, Some("a"), &record(t0, 1.5, "running", &party, vec![second(0, 100.4, 20.5, true), second(1, 50.0, 0.0, true)]), &[], &[], Some("place-1")),
        compactor.next("running", t0, Some("a"), &record(t0, 2.5, "running", &left, vec![second(0, 100.4, 20.5, true), second(1, 60.0, 0.0, true), second(2, 0.0, 0.0, false)]), &[], &[], Some("place-1")),
        compactor.next("running", t0, Some("a"), &paused(), std::slice::from_ref(&floor), &dungeons, Some("place-2")),
        compactor.next("paused", t0, Some("a"), &paused(), std::slice::from_ref(&floor), &dungeons, Some("place-2")),
    ];
    compactor.reset();
    deltas.push(compactor.next("paused", t0, Some("a"), &paused(), std::slice::from_ref(&floor), &dungeons, Some("place-2")));
    deltas.push(compactor.next("running", plus(t0, 30), None, &record(t0, 0.5, "running", &party, vec![second(0, 1.0, 1.0, true)]), &[], &[], Some("place-3")));
    deltas.push(compactor.next("completed", plus(t0, 30), None, &record(t0, 0.5, "completed", &party, vec![second(0, 1.0, 1.0, true)]), &[], &[], None));

    let expected = fixture("compactor.json");
    let expected = expected.as_array().unwrap();
    assert_eq!(deltas.len(), expected.len());
    for (step, (delta, expected)) in deltas.iter().zip(expected).enumerate() {
        // Through the C#-style writer, so 100.0 compares as the C# 100.
        let actual: Value = serde_json::from_str(&json::to_string(delta).unwrap()).unwrap();
        assert_eq!(&actual, expected, "delta {step}");
    }
}

#[test]
fn suggested_titles_match_the_csharp_ones() {
    let player = |id: &str, name: &str, family: &str| SourceMember { id: id.into(), name: name.into(), weapon: "Weapon".into(), family: family.into(), color: "#ffffff".into(), membership: "active".into(), weapon_id: None };
    let record = |members: Vec<SourceMember>| SourceSnapshot { id: "r".into(), started_at: DateTimeOffset::default(), elapsed: 60.0, status: "running".into(), events: 1, members, weapons: vec![], items: vec![], timeline: vec![] };
    let run = |name: &str, entrance: Option<&str>| SourceDungeon { id: name.into(), name: name.into(), started_at: DateTimeOffset::default(), status: "running".into(), entrance: entrance.map(Into::into), floors: vec![] };
    let party: Vec<SourceMember> = (1..=18).map(|index| player(&format!("p{index}"), &format!("Player{index}"), "Holy Staff")).collect();
    let titles = [
        suggest_title(Some("p1"), &record(party.clone()), &[run("Random dungeon", Some("Hightree Cliffs"))]),
        suggest_title(Some("k"), &record(vec![player("k", "KPey", "Holy Staff")]), &[run("Expedition", None), run("Expedition", None)]),
        suggest_title(Some("k"), &record(vec![player("k", "KPey", "Cursed Staff")]), &[]),
        suggest_title(Some("k"), &record(vec![player("k", "KPey", "unknown")]), &[]),
        suggest_title(Some("p1"), &record(party[..5].to_vec()), &[]),
        suggest_title(Some("x"), &record(vec![]), &[]),
        suggest_title(Some("x"), &record(vec![player("k", &"é".repeat(90), "Holy Staff")]), &[]),
    ];
    assert_eq!(serde_json::to_value(titles).unwrap(), fixture("titles.json"));
}

//! Synthetic captures built from the C# test fixtures: the differential harness (`examples/synth.rs`) and the
//! benches replay them until real captures are available.

use super::{encode, fragment, guid_bytes, ip, message_frame, msg, photon, put16, request_frame, response, strings, udp};
use crate::CaptureFrame;
use crate::capture::protocol18::{Value, WireMessage};
use crate::cs::Guid;

/// Every scenario with its file name.
pub fn all() -> Vec<(&'static str, Vec<CaptureFrame>)> {
    vec![("solo-dungeon", solo()), ("party-dungeon", party()), ("transport", transport()), ("stress-30min", stress(30))]
}

#[derive(Default)]
struct Scenario {
    frames: Vec<CaptureFrame>,
    server: u32,
    client: u32,
}

impl Scenario {
    fn server(&mut self, message: WireMessage, at: i64) {
        self.server += 1;
        self.frames.push(message_frame(&message, at, self.server));
    }
    fn client(&mut self, message: WireMessage, at: i64) {
        self.client += 1;
        self.frames.push(request_frame(&message, at, self.client));
    }
    fn raw(&mut self, data: Vec<u8>, at: i64) {
        self.frames.push(CaptureFrame { timestamp_us: at, original_length: data.len() as u32, link_type: 101, data });
    }
}

fn id(n: u8) -> Guid {
    Guid([n, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, n])
}

fn join(object: i32, character: Guid, name: &str, map: &str, entrance: &str) -> WireMessage<'static> {
    msg(3, vec![(253, Value::Byte(2)), (0, object.into()), (1, character.into()), (2, name.into()), (8, map.into()), (66, entrance.into())])
}

fn hit(target: i32, source: i32, delta: f32, spell: i32, origin: u8) -> WireMessage<'static> {
    msg(4, vec![(252, Value::Byte(6)), (0, target.into()), (6, source.into()), (2, delta.into()), (7, spell.into()), (5, origin.into())])
}

fn announce(object: i32, character: Guid, name: &str, gear: &[i16]) -> WireMessage<'static> {
    msg(4, vec![(252, Value::Byte(29)), (0, object.into()), (1, name.into()), (7, character.into()), (40, gear.into())])
}

fn equipment(object: i32, gear: &[i16], spells: &[i16]) -> WireMessage<'static> {
    msg(4, vec![(252, Value::Byte(90)), (0, object.into()), (2, gear.into()), (7, spells.into())])
}

fn party_list(ids: &[Guid], names: &[&str], party: i32) -> WireMessage<'static> {
    msg(4, vec![(252, Value::Short(230)), (0, party.into()), (8, guid_bytes(ids)), (9, strings(names))])
}

pub fn solo() -> Vec<CaptureFrame> {
    let mut s = Scenario::default();
    let me = id(1);
    s.server(join(10, me, "Alpha", "@RANDOMDUNGEON@one", "1315"), 1_100_000);
    s.server(equipment(10, &[7346, 0, 4224, 5452, 5472, 2909, 2587, 3098], &[]), 1_150_000);
    s.server(hit(30, 10, -900.0, -1, 0), 1_200_000);
    s.server(hit(30, 10, -500.0, 3338, 0), 2_000_000);
    s.server(hit(10, 10, 40.0, 3338, 0), 2_500_000);
    s.server(hit(30, 10, -123.25, 99999, 0), 3_999_999);
    s.server(msg(4, vec![(252, Value::Byte(7)), (0, 30.into()), (2, Value::from(vec![Value::Float(-10.0), Value::Float(-20.0)])), (6, Value::from(vec![Value::Int(10), Value::Int(10)])), (7, Value::from(vec![Value::Int(-1), Value::Int(-1)])), (5, Value::from(vec![Value::Byte(0), Value::Byte(1)]))]), 4_100_000);
    s.server(msg(3, vec![(253, Value::Byte(41)), (0, "@RANDOMDUNGEON@two".into()), (1, "Crypt-SOLO".into())]), 5_000_000);
    s.server(join(11, me, "Alpha", "@RANDOMDUNGEON@two", "1315"), 6_000_000);
    s.server(hit(31, 11, -250.0, -1, 0), 7_000_000);
    s.server(msg(3, vec![(253, Value::Byte(41)), (0, "1315".into())]), 8_000_000);
    s.server(join(12, me, "Alpha", "1315", "1301"), 9_000_000);
    s.server(hit(32, 12, -75.0, -1, 0), 10_000_000);
    s.frames
}

pub fn party() -> Vec<CaptureFrame> {
    let mut s = Scenario::default();
    let (me, mate, healer, stranger) = (id(1), id(2), id(3), id(4));
    s.server(join(10, me, "Alpha", "@RANDOMDUNGEON@one", "1315"), 1_000_000);
    s.server(party_list(&[me, mate, healer], &["Alpha", "Mate", "Healer"], 43846), 1_100_000);
    s.server(announce(402, mate, "Mate", &[6661, 0, 5347, 5005, 5397, 2900, 2662, 3098]), 1_200_000);
    s.server(announce(403, stranger, "Stranger", &[7696]), 1_250_000);
    s.server(msg(4, vec![(252, Value::Short(670)), (0, 404.into())]), 1_300_000);
    s.server(hit(900, 402, -100.0, -1, 0), 2_000_000);
    s.server(hit(900, 403, -999.0, -1, 0), 2_100_000);
    s.server(hit(900, 404, -60.0, 3404, 0), 2_200_000);
    s.server(hit(402, 404, 80.0, 3404, 0), 2_300_000);
    s.server(hit(900, 10, -40.0, -1, 0), 2_400_000);
    s.server(msg(4, vec![(252, Value::Byte(74)), (0, "healer".into()), (1, "hello".into()), (4, 404.into()), (5, healer.into())]), 3_000_000);
    s.server(hit(900, 404, -30.0, 3404, 1), 3_500_000);
    s.server(equipment(402, &[7346, 0, 5347, 5005, 5397, 2900, 2662, 3098], &[3338]), 4_000_000);
    s.server(hit(900, 402, -500.0, 3338, 0), 4_100_000);
    s.server(msg(4, vec![(252, Value::Byte(44)), (0, 1955.into()), (3, 405.into()), (6, mate.into()), (15, 43846.into())]), 4_200_000);
    s.server(msg(4, vec![(252, Value::Short(235)), (0, 43846.into()), (1, healer.into())]), 5_000_000);
    s.server(hit(900, 404, -25.0, 3404, 0), 5_500_000);
    s.client(msg(2, vec![(253, Value::Byte(24)), (0, Value::Long(777)), (4, 3404.into())]), 6_000_000);
    s.server(msg(4, vec![(252, Value::Byte(18)), (0, 406.into()), (1, 30.into()), (2, 3404.into()), (5, Value::Long(777))]), 6_100_000);
    s.server(hit(900, 406, -45.0, 3404, 0), 6_200_000);
    s.server(response(1, vec![(253, Value::Byte(41))]), 6_500_000);
    s.server(msg(3, vec![(253, Value::Byte(41)), (0, "@RANDOMDUNGEON@two".into())]), 7_000_000);
    s.server(hit(900, 402, -11.0, -1, 0), 7_100_000);
    s.server(join(20, me, "Alpha", "@RANDOMDUNGEON@two", "1315"), 7_500_000);
    s.server(announce(502, mate, "Mate", &[6661, 0, 5347, 5005, 5397, 2900, 2662, 3098]), 7_600_000);
    s.server(hit(900, 502, -333.0, -1, 0), 8_000_000);
    s.frames
}

pub fn transport() -> Vec<CaptureFrame> {
    let mut s = Scenario::default();
    let me = id(1);
    s.server(join(10, me, "Alpha", "1301", "1301"), 1_000_000);
    // A retransmitted command, then IPv4 fragments out of order.
    let duplicate = message_frame(&hit(30, 10, -10.0, -1, 0), 2_000_000, 50);
    s.frames.push(duplicate.clone());
    s.frames.push(CaptureFrame { timestamp_us: 2_100_000, ..duplicate });
    let datagram = udp(&photon(&encode(&hit(30, 10, -20.0, -1, 0)), 51));
    let (head, tail) = datagram.split_at(16);
    let mut second = ip(tail, 2, 7);
    let mut first = ip(head, 0x2000, 7);
    for packet in [&mut first, &mut second] {
        packet[12..16].copy_from_slice(&[192, 0, 0, 1]);
        packet[16..20].copy_from_slice(&[192, 0, 0, 2]);
    }
    s.raw(second, 2_200_000);
    s.raw(first, 2_300_000);
    // A Photon message split in two fragments.
    let payload = encode(&hit(30, 10, -30.0, -1, 0));
    let (a, b) = payload.split_at(payload.len() / 2);
    for (number, offset, part) in [(1u32, a.len() as u32, b), (0, 0, a)] {
        let mut packet = ip(&udp(&fragment(part, number, offset, payload.len() as u32)), 0, 0);
        packet[12..16].copy_from_slice(&[192, 0, 0, 1]);
        packet[16..20].copy_from_slice(&[192, 0, 0, 2]);
        s.raw(packet, 2_400_000 + i64::from(number));
    }
    // IPv6, malformed, truncated, non-game and encrypted traffic.
    let six = udp(&photon(&encode(&hit(30, 10, -40.0, -1, 0)), 52));
    let mut v6 = vec![0u8; 40 + six.len()];
    v6[0] = 0x60;
    put16(&mut v6, 4, six.len() as u16);
    v6[6] = 17;
    v6[23] = 1;
    v6[39] = 2;
    v6[40..].copy_from_slice(&six);
    s.raw(v6, 2_500_000);
    s.raw(vec![0x45, 0, 0], 2_600_000);
    let mut other = ip(&udp(&[1, 2, 3]), 0, 0);
    put16(&mut other, 20, 443);
    put16(&mut other, 22, 50_000);
    s.raw(other, 2_700_000);
    let mut encrypted = ip(&udp(&photon(&[0xf3, 0x83, 1], 53)), 0, 0);
    encrypted[12..16].copy_from_slice(&[192, 0, 0, 1]);
    s.raw(encrypted, 2_800_000);
    s.raw(ip(&udp(&photon(&[0xf3, 4, 1, 1, 0, 99], 54)), 0, 0), 2_900_000);
    let mut short = message_frame(&hit(30, 10, -50.0, -1, 0), 3_000_000, 55);
    short.original_length += 10;
    s.frames.push(short);
    s.server(hit(30, 10, -60.0, -1, 0), 1_500_000);
    s.frames
}

/// A long deterministic fight: a five-player party, mobs, weapon swaps, unnamed members named later, floors and
/// city visits, so float sums, contribution order and dictionary slot reuse are exercised at scale.
pub fn stress(minutes: i64) -> Vec<CaptureFrame> {
    let mut s = Scenario::default();
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |below: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % below
    };
    let members = [id(1), id(2), id(3), id(4), id(5)];
    let names = ["Alpha", "Bravo", "Charlie", "Delta", "Echo"];
    let weapons = [7346i16, 6661, 9022, 7108, 7587, 4308, 2637];
    let spells = [-1, 3338, 3404, 3409, 3413, 3342, 3952, 3953, 214, 999_999];
    let mut at = 1_000_000i64;
    let mut floor = 0;
    let mut objects = [10, 402, 403, 404, 405];
    let enter = |s: &mut Scenario, floor: i32, at: i64, objects: &mut [i32; 5]| {
        let map = if floor % 4 == 3 { "1315".to_owned() } else { format!("@RANDOMDUNGEON@floor{floor}") };
        s.server(msg(3, vec![(253, Value::Byte(41)), (0, map.as_str().into())]), at);
        for (n, object) in objects.iter_mut().enumerate() {
            *object = 1000 * (floor + 1) + n as i32;
        }
        s.server(join(objects[0], members[0], "Alpha", &map, "1315"), at + 500_000);
        s.server(party_list(&members, &names, 77), at + 600_000);
        for n in 1..5 {
            if (floor + n as i32) % 3 != 0 {
                s.server(announce(objects[n], members[n], names[n], &[weapons[n], 0, 5347, 5005, 5397, 2900, 2662, 3098]), at + 700_000);
            } else {
                s.server(msg(4, vec![(252, Value::Short(670)), (0, objects[n].into())]), at + 700_000);
            }
        }
    };
    enter(&mut s, floor, at, &mut objects);
    let end = at + minutes * 60_000_000;
    while at < end {
        at += 20_000 + next(200_000) as i64;
        match next(100) {
            0 => {
                floor += 1;
                enter(&mut s, floor, at, &mut objects);
                at += 1_000_000;
            }
            1..=3 => {
                let n = next(5) as usize;
                let weapon = weapons[next(weapons.len() as u64) as usize];
                s.server(equipment(objects[n], &[weapon, 0, 4224, 5452, 5472, 2909, 2587, 3098], &[3404, 3409]), at);
            }
            4 => {
                let n = 1 + next(4) as usize;
                s.server(msg(4, vec![(252, Value::Byte(74)), (0, names[n].into()), (1, "hi".into()), (4, objects[n].into()), (5, members[n].into())]), at);
            }
            5..=9 => s.server(hit(objects[next(5) as usize], 5000 + next(20) as i32, -(next(5000) as f32) - 1.0, -1, 0), at),
            _ => {
                let source = objects[next(5) as usize];
                let spell = spells[next(spells.len() as u64) as usize];
                let amount = (next(100_000) as f32) / 7.0 + 0.5;
                let delta = if next(4) == 0 { amount } else { -amount };
                if next(10) == 0 {
                    let count = 2 + next(4) as usize;
                    let values: Vec<Value> = (0..count).map(|k| Value::Float(delta + k as f32)).collect();
                    let sources: Vec<Value> = (0..count).map(|_| Value::Int(source)).collect();
                    let kinds: Vec<Value> = (0..count).map(|k| Value::Int(spells[k % spells.len()])).collect();
                    s.server(msg(4, vec![(252, Value::Byte(7)), (0, 30.into()), (2, Value::from(values)), (6, Value::from(sources)), (7, Value::from(kinds))]), at);
                } else {
                    s.server(hit(30, source, delta, spell, next(3) as u8), at);
                }
            }
        }
    }
    s.frames
}


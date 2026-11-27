//! Packet fixtures, as the C# tests build them: Protocol18 messages wrapped in Photon, UDP, IPv4 and a raw-IP frame.
//! Used by the tests, the synthetic captures of the differential harness and the examples.

use crate::capture::protocol18::{Value, WireMessage};
pub mod scenarios;

use crate::CaptureFrame;
use crate::cs::Guid;


/// Builds a message: `msg(4, &[(252, 6.into()), (0, 30.into())])`.
pub fn msg(kind: u8, parameters: Vec<(u8, Value<'static>)>) -> WireMessage<'static> {
    WireMessage::new(kind, 1, if kind == 3 { Some(0) } else { None }, parameters)
}

/// A response with an explicit return code.
pub fn response(return_code: i16, parameters: Vec<(u8, Value<'static>)>) -> WireMessage<'static> {
    WireMessage::new(3, 1, Some(return_code), parameters)
}

/// `new object[] { "a", "b" }`
pub fn strings(values: &[&str]) -> Value<'static> {
    Value::from(values.iter().map(|&v| Value::from(v)).collect::<Vec<_>>())
}

pub fn guid_bytes(ids: &[Guid]) -> Value<'static> {
    Value::from(ids.iter().flat_map(|id| id.to_bytes()).collect::<Vec<u8>>())
}

fn var(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push((value & 127) as u8 | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Protocol18 encoding of a value (with its type byte).
pub fn encode_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.push(8),
        Value::Bool(b) => out.push(if *b { 28 } else { 27 }),
        Value::Byte(b) => out.extend_from_slice(&[3, *b]),
        Value::Short(s) => {
            out.push(4);
            out.extend_from_slice(&s.to_le_bytes());
        }
        Value::Int(i) => {
            out.push(9);
            var(out, (((*i) << 1) ^ ((*i) >> 31)) as u32 as u64);
        }
        Value::Long(l) => {
            out.push(10);
            var(out, (((*l) << 1) ^ ((*l) >> 63)) as u64);
        }
        Value::Float(f) => {
            out.push(5);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Double(d) => {
            out.push(6);
            out.extend_from_slice(&d.to_le_bytes());
        }
        Value::String(s) => {
            out.push(7);
            var(out, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Bytes(b) => {
            out.push(67);
            var(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::Bools(count, packed) => {
            out.push(66);
            var(out, *count as u64);
            out.extend_from_slice(packed);
        }
        Value::Array(values) => {
            out.push(23);
            var(out, values.len() as u64);
            for value in values.iter() {
                encode_value(out, value);
            }
        }
        Value::Map(pairs) => {
            out.push(21);
            var(out, pairs.len() as u64);
            for (k, v) in pairs.iter() {
                encode_value(out, k);
                encode_value(out, v);
            }
        }
        Value::Opaque(kind, bytes) => {
            out.extend_from_slice(&[19, *kind]);
            var(out, bytes.len() as u64);
            out.extend_from_slice(bytes);
        }
        Value::Opaques(kind, values) => {
            out.push(83);
            var(out, values.len() as u64);
            out.push(*kind);
            for bytes in values.iter() {
                var(out, bytes.len() as u64);
                out.extend_from_slice(bytes);
            }
        }
        Value::Message(messages) => {
            out.push(22 + messages[0].kind);
            encode_body(out, &messages[0]);
        }
    }
}

fn encode_body(out: &mut Vec<u8>, message: &WireMessage) {
    out.push(message.code);
    if message.kind == 3 {
        out.extend_from_slice(&message.return_code.unwrap_or(0).to_le_bytes());
        out.push(8);
    }
    out.push(message.parameters.len() as u8);
    for (key, value) in message.parameters.iter() {
        out.push(*key);
        encode_value(out, value);
    }
}

/// The Protocol18 payload of a message (`0xf3`, kind, body).
pub fn encode(message: &WireMessage) -> Vec<u8> {
    let mut out = vec![0xf3, message.kind];
    encode_body(&mut out, message);
    out
}

pub fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

pub fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// `CaptureChecks.Udp`: 5056 → 12345.
pub fn udp(payload: &[u8]) -> Vec<u8> {
    udp_ports(payload, 5056, 12345)
}

pub fn udp_ports(payload: &[u8], source: u16, destination: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 8 + payload.len()];
    put16(&mut bytes, 0, source);
    put16(&mut bytes, 2, destination);
    let length = bytes.len() as u16;
    put16(&mut bytes, 4, length);
    bytes[8..].copy_from_slice(payload);
    bytes
}

/// `CaptureChecks.Ip`: 192.0.2.1 → 192.0.2.2.
pub fn ip(payload: &[u8], flags: u16, id: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 20 + payload.len()];
    bytes[0] = 0x45;
    bytes[9] = 17;
    let length = bytes.len() as u16;
    put16(&mut bytes, 2, length);
    put16(&mut bytes, 4, id);
    put16(&mut bytes, 6, flags);
    bytes[12..16].copy_from_slice(&[192, 0, 2, 1]);
    bytes[16..20].copy_from_slice(&[192, 0, 2, 2]);
    bytes[20..].copy_from_slice(payload);
    bytes
}

/// `CaptureChecks.Photon`: one reliable command on channel 0, peer 0, challenge 1.
pub fn photon(payload: &[u8], sequence: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 24 + payload.len()];
    bytes[3] = 1;
    bytes[11] = 1;
    bytes[12] = 6;
    put32(&mut bytes, 16, 12 + payload.len() as u32);
    put32(&mut bytes, 20, sequence);
    bytes[24..].copy_from_slice(payload);
    bytes
}

/// `CaptureChecks.Fragment`.
pub fn fragment(payload: &[u8], number: u32, offset: u32, total: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 44 + payload.len()];
    bytes[3] = 1;
    bytes[11] = 1;
    bytes[12] = 8;
    put32(&mut bytes, 16, 32 + payload.len() as u32);
    put32(&mut bytes, 20, number + 10);
    put32(&mut bytes, 24, 10);
    put32(&mut bytes, 28, 2);
    put32(&mut bytes, 32, number);
    put32(&mut bytes, 36, total);
    put32(&mut bytes, 40, offset);
    bytes[44..].copy_from_slice(payload);
    bytes
}

/// A raw-IP (link type 101) frame at 1 s.
pub fn frame(bytes: Vec<u8>) -> CaptureFrame {
    CaptureFrame { timestamp_us: 1_000_000, original_length: bytes.len() as u32, link_type: 101, data: bytes }
}

/// `ManualRecordingChecks.Frame`: a server message as one Photon reliable command from 192.0.0.1:5056.
pub fn message_frame(message: &WireMessage, at: i64, sequence: u32) -> CaptureFrame {
    let packet = ip(&udp(&photon(&encode(message), sequence)), 0, 0);
    let mut packet = packet;
    packet[12..16].copy_from_slice(&[192, 0, 0, 1]);
    packet[16..20].copy_from_slice(&[192, 0, 0, 2]);
    CaptureFrame { timestamp_us: at, original_length: packet.len() as u32, link_type: 101, data: packet }
}

/// A client request (12345 → 5055), as the game sends its own casts.
pub fn request_frame(message: &WireMessage, at: i64, sequence: u32) -> CaptureFrame {
    let mut packet = ip(&udp_ports(&photon(&encode(message), sequence), 12345, 5055), 0, 0);
    packet[12..16].copy_from_slice(&[192, 0, 0, 2]);
    packet[16..20].copy_from_slice(&[192, 0, 0, 1]);
    CaptureFrame { timestamp_us: at, original_length: packet.len() as u32, link_type: 101, data: packet }
}

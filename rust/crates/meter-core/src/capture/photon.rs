//! Photon transport: command framing, retransmission dedup and fragment reassembly (`PhotonTransport.cs`).

use super::network::{Flow, UdpDatagram, is_game_port};
use crate::cs::CsMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

/// Interned Photon connection (see [`PhotonTransport`]); `Arc<str>` so observations can key on it without copying.
pub type Connection = Arc<str>;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum ConnectionKey {
    Flow(Flow, u16),
    Route { server: bool, peer: u16, challenge: u32 },
}

/// A command's identity without its connection text: `connection:channel:type:sequence[:extra]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CommandKey {
    pub connection: u32,
    pub channel: u8,
    pub kind: u8,
    pub number: u32,
    pub extra: Option<u32>,
}

/// One Photon message. `identity` formats exactly as the C# string identity.
#[derive(Clone, Debug)]
pub struct PhotonMessage {
    pub command: CommandKey,
    pub connection: Connection,
    pub timestamp_us: i64,
    payload: (usize, usize),
}

/// The message identity text, built only when an event id needs it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Identity {
    pub connection: Connection,
    pub channel: u8,
    pub kind: u8,
    pub number: u32,
    pub extra: Option<u32>,
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}:{}", self.connection, self.channel, self.kind, self.number)?;
        if let Some(extra) = self.extra {
            write!(f, ":{extra}")?;
        }
        Ok(())
    }
}

impl PhotonMessage {
    pub fn identity(&self) -> Identity {
        Identity {
            connection: self.connection.clone(),
            channel: self.command.channel,
            kind: self.command.kind,
            number: self.command.number,
            extra: self.command.extra,
        }
    }
}

struct Assembly {
    bytes: Vec<u8>,
    received: Vec<bool>,
    parts: HashMap<u32, (u32, usize)>,
    expected_fragments: u32,
    count: usize,
    timestamp: i64,
}

type AssemblyKey = (u32, u8, u32);

const WINDOW: usize = 32_768;

/// Decodes Photon packets. Message payloads are copied into one reused arena per datagram, so steady-state decoding does
/// not allocate; read them with [`PhotonTransport::payload`] until the next `decode`.
#[derive(Default)]
pub struct PhotonTransport {
    connections: HashMap<ConnectionKey, u32>,
    names: Vec<Connection>,
    assemblies: CsMap<AssemblyKey, Assembly>,
    seen: HashSet<CommandKey>,
    seen_order: VecDeque<CommandKey>,
    latest_timestamp: i64,
    arena: Vec<u8>,
    messages: Vec<PhotonMessage>,
    pub duplicates: i64,
    pub expired_assemblies: i64,
    pub evicted_assemblies: i64,
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn validate(mut bytes: &[u8]) -> Option<&'static str> {
    if bytes.is_empty() {
        return Some("malformedPhoton");
    }
    while !bytes.is_empty() {
        if bytes.len() < 12 || bytes[3] == 0 {
            return Some("malformedPhoton");
        }
        if bytes[2] != 0 {
            return Some(if bytes[2] == 1 { "encryptedPhoton" } else { "unsupportedPhotonFlags" });
        }
        let mut offset = 12usize;
        for _ in 0..bytes[3] {
            if bytes.len() - offset < 12 {
                return Some("malformedPhoton");
            }
            let length = u32_at(bytes, offset + 4) as usize;
            let kind = bytes[offset];
            if length < 12 || length > bytes.len() - offset || !(1..=12).contains(&kind) || (kind == 7 && length < 16) || (kind == 8 && length < 32) {
                return Some("malformedPhoton");
            }
            offset += length;
        }
        bytes = &bytes[offset..];
    }
    None
}

impl PhotonTransport {
    /// The dedup window (32 768 commands) is allocated up front, so a long capture never grows it packet by packet.
    pub fn new() -> Self {
        Self { seen: HashSet::with_capacity(WINDOW + 1), seen_order: VecDeque::with_capacity(WINDOW + 1), ..Self::default() }
    }

    pub fn pending_assemblies(&self) -> usize {
        self.assemblies.len()
    }

    pub fn payload(&self, message: &PhotonMessage) -> &[u8] {
        &self.arena[message.payload.0..message.payload.1]
    }

    pub fn connection_name(&self, id: u32) -> &Connection {
        &self.names[id as usize]
    }

    /// One Photon connection, named by its peer id and challenge and the direction, never by addresses: a route changer
    /// (ExitLag, NoPing…) moves the same connection to another IP or port, and copies sent over several routes are
    /// still the same commands. Only a packet without a challenge falls back to the network flow.
    fn connection(&mut self, datagram: &UdpDatagram<'_>, peer: u16, challenge: u32) -> u32 {
        let key = if challenge == 0 {
            ConnectionKey::Flow(datagram.flow(), peer)
        } else {
            ConnectionKey::Route { server: is_game_port(datagram.source_port), peer, challenge }
        };
        if let Some(&id) = self.connections.get(&key) {
            return id;
        }
        let name: Connection = match key {
            ConnectionKey::Flow(flow, peer) => format!("{flow}:{peer}:0").into(),
            ConnectionKey::Route { server, peer, challenge } => format!("{}:{peer}:{challenge}", if server { "server" } else { "client" }).into(),
        };
        let id = self.names.len() as u32;
        self.names.push(name);
        self.connections.insert(key, id);
        id
    }

    fn remember(&mut self, key: CommandKey) -> bool {
        if !self.seen.insert(key) {
            self.duplicates += 1;
            return false;
        }
        self.seen_order.push_back(key);
        if self.seen_order.len() > WINDOW {
            let oldest = self.seen_order.pop_front().unwrap();
            self.seen.remove(&oldest);
        }
        true
    }

    fn push(&mut self, command: CommandKey, timestamp: i64, payload: &[u8]) {
        let start = self.arena.len();
        self.arena.extend_from_slice(payload);
        let connection = self.names[command.connection as usize].clone();
        self.messages.push(PhotonMessage { command, connection, timestamp_us: timestamp, payload: (start, self.arena.len()) });
    }

    /// Decodes a datagram and returns its status; [`Self::messages`] then holds the messages it carried (also those
    /// decoded before a fragment error).
    pub fn decode(&mut self, datagram: &UdpDatagram<'_>) -> &'static str {
        self.arena.clear();
        self.messages.clear();
        self.decode_into(datagram)
    }

    pub fn messages(&self) -> &[PhotonMessage] {
        &self.messages
    }

    fn decode_into(&mut self, datagram: &UdpDatagram<'_>) -> &'static str {
        self.latest_timestamp = self.latest_timestamp.max(datagram.timestamp_us);
        let latest = self.latest_timestamp;
        let before = self.assemblies.len();
        self.assemblies.retain(|_, assembly| latest - assembly.timestamp <= 15_000_000);
        self.expired_assemblies += (before - self.assemblies.len()) as i64;
        let mut bytes = datagram.payload;
        if let Some(status) = validate(bytes) {
            return status;
        }
        while !bytes.is_empty() {
            let connection = self.connection(datagram, u16::from_be_bytes([bytes[0], bytes[1]]), u32_at(bytes, 8));
            let count = bytes[3];
            let mut offset = 12usize;
            let mut commands = [(0usize, 0usize); 255];
            for command in commands.iter_mut().take(usize::from(count)) {
                let length = u32_at(bytes, offset + 4) as usize;
                *command = (offset, length);
                offset += length;
            }
            for &(start, length) in &commands[..usize::from(count)] {
                let data = &bytes[start..start + length];
                let (kind, channel, sequence) = (data[0], data[1], u32_at(data, 8));
                if kind == 6 || kind == 7 {
                    let header = if kind == 6 { 12 } else { 16 };
                    let command =
                        CommandKey { connection, channel, kind, number: sequence, extra: (kind == 7).then(|| u32_at(data, 12)) };
                    if data.len() == header || !self.remember(command) {
                        continue;
                    }
                    self.push(command, datagram.timestamp_us, &data[header..]);
                } else if kind == 8 {
                    let (start, fragments) = (u32_at(data, 12), u32_at(data, 16));
                    let (number, total, position) = (u32_at(data, 20), u32_at(data, 24), u32_at(data, 28));
                    let payload = &data[32..];
                    let key = (connection, channel, start);
                    let command = CommandKey { connection, channel, kind: 8, number: start, extra: None };
                    if total == 0 || total > 1_048_576 || fragments == 0 || fragments > 4096 || number >= fragments
                        || position >= total || payload.is_empty() || payload.len() > (total - position) as usize
                    {
                        self.assemblies.remove(&key);
                        return "malformedPhotonFragment";
                    }
                    if self.seen.contains(&command) {
                        self.duplicates += 1;
                        continue;
                    }
                    if !self.assemblies.contains_key(&key) {
                        if self.assemblies.len() >= 16 {
                            let oldest = self.assemblies.iter().min_by_key(|(_, assembly)| assembly.timestamp).map(|(key, _)| *key).unwrap();
                            self.assemblies.remove(&oldest);
                            self.evicted_assemblies += 1;
                        }
                        self.assemblies.insert(
                            key,
                            Assembly {
                                bytes: vec![0; total as usize],
                                received: vec![false; total as usize],
                                parts: HashMap::new(),
                                expected_fragments: fragments,
                                count: 0,
                                timestamp: latest,
                            },
                        );
                    }
                    let assembly = self.assemblies.get_mut(&key).unwrap();
                    if assembly.bytes.len() != total as usize
                        || assembly.expected_fragments != fragments
                        || assembly.parts.get(&number).is_some_and(|&previous| previous != (position, payload.len()))
                    {
                        self.assemblies.remove(&key);
                        return "photonFragmentConflict";
                    }
                    for (index, &byte) in payload.iter().enumerate() {
                        let target = position as usize + index;
                        if assembly.received[target] {
                            if assembly.bytes[target] != byte {
                                self.assemblies.remove(&key);
                                return "photonFragmentConflict";
                            }
                        } else {
                            assembly.received[target] = true;
                            assembly.bytes[target] = byte;
                            assembly.count += 1;
                        }
                    }
                    assembly.parts.insert(number, (position, payload.len()));
                    if assembly.count == total as usize && assembly.parts.len() == fragments as usize {
                        let assembly = self.assemblies.remove(&key).unwrap();
                        self.remember(command);
                        self.push(command, datagram.timestamp_us, &assembly.bytes);
                    }
                }
            }
            bytes = &bytes[offset..];
        }
        "photon"
    }
}

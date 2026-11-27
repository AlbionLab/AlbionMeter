//! Link layer, IPv4/IPv6 and UDP decoding with IPv4 fragment reassembly (`NetworkDecoder.cs`).

use crate::cs::CsMap;
use meter_capture::CaptureFrame;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One UDP datagram on an Albion port. The payload borrows the frame, or the decoder's reassembly buffer.
#[derive(Clone, Copy, Debug)]
pub struct UdpDatagram<'a> {
    pub timestamp_us: i64,
    pub source: IpAddr,
    pub destination: IpAddr,
    pub source_port: u16,
    pub destination_port: u16,
    pub payload: &'a [u8],
}

/// The network flow of a datagram; formats as the C# `Flow` (`source:port>destination:port`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Flow {
    pub source: IpAddr,
    pub source_port: u16,
    pub destination: IpAddr,
    pub destination_port: u16,
}

impl fmt::Display for Flow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}>{}:{}", self.source, self.source_port, self.destination, self.destination_port)
    }
}

impl UdpDatagram<'_> {
    pub fn flow(&self) -> Flow {
        Flow { source: self.source, source_port: self.source_port, destination: self.destination, destination_port: self.destination_port }
    }
}

pub struct NetworkResult<'a> {
    pub status: &'static str,
    pub datagram: Option<UdpDatagram<'a>>,
}

fn status(status: &'static str) -> NetworkResult<'static> {
    NetworkResult { status, datagram: None }
}

struct FragmentSet {
    timestamp: i64,
    bytes: Box<[u8]>,
    received: Box<[bool]>,
    count: usize,
    highest: usize,
    length: Option<usize>,
}

type FragmentKey = (Ipv4Addr, Ipv4Addr, u16);

/// Fragment buffers are pooled: an assembly that completes or expires hands its 64 KiB back instead of freeing it.
#[derive(Default)]
pub struct NetworkDecoder {
    fragments: CsMap<FragmentKey, FragmentSet>,
    pool: Vec<FragmentSet>,
    assembled: Option<FragmentSet>,
    latest_timestamp: i64,
    pub expired_fragments: i64,
    pub evicted_fragments: i64,
}

pub fn is_game_port(port: u16) -> bool {
    matches!(port, 5055 | 5056 | 5058)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

impl NetworkDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_fragments(&self) -> usize {
        self.fragments.len()
    }

    fn release(&mut self, set: FragmentSet) {
        if self.pool.len() < 8 {
            self.pool.push(set);
        }
    }

    fn drop_set(&mut self, key: &FragmentKey) {
        if let Some(set) = self.fragments.remove(key) {
            self.release(set);
        }
    }

    pub fn decode<'a>(&'a mut self, frame: &'a CaptureFrame) -> NetworkResult<'a> {
        self.latest_timestamp = self.latest_timestamp.max(frame.timestamp_us);
        if let Some(set) = self.assembled.take() {
            self.release(set);
        }
        let latest = self.latest_timestamp;
        let expired: Vec<FragmentKey> =
            self.fragments.iter().filter(|(_, set)| latest - set.timestamp > 15_000_000).map(|(key, _)| *key).collect();
        for key in expired {
            self.drop_set(&key);
            self.expired_fragments += 1;
        }
        if (frame.data.len() as u64) < u64::from(frame.original_length) {
            return status("truncated");
        }
        let bytes = frame.data.as_slice();
        let (offset, protocol) = match frame.link_type {
            1 => {
                if bytes.len() < 14 {
                    return status("malformed");
                }
                let (mut offset, mut protocol) = (14, u16_at(bytes, 12));
                let mut count = 0;
                while protocol == 0x8100 || protocol == 0x88a8 {
                    if count >= 4 || bytes.len() < offset + 4 {
                        return status("malformed");
                    }
                    protocol = u16_at(bytes, offset + 2);
                    offset += 4;
                    count += 1;
                }
                (offset, protocol)
            }
            113 => {
                if bytes.len() < 16 {
                    return status("malformed");
                }
                (16, u16_at(bytes, 14))
            }
            276 => {
                if bytes.len() < 20 {
                    return status("malformed");
                }
                (20, u16_at(bytes, 0))
            }
            101 | 12 => {
                if bytes.is_empty() {
                    return status("malformed");
                }
                (0, if bytes[0] >> 4 == 6 { 0x86dd } else { 0x0800 })
            }
            0 | 108 => {
                if bytes.len() < 5 {
                    return status("malformed");
                }
                (4, if bytes[4] >> 4 == 6 { 0x86dd } else { 0x0800 })
            }
            _ => return status("unsupportedLink"),
        };
        let bytes = &bytes[offset..];
        match protocol {
            0x0800 => self.ipv4(bytes, frame.timestamp_us),
            0x86dd => ipv6(bytes, frame.timestamp_us),
            _ => status("ignored"),
        }
    }

    fn ipv4<'a>(&'a mut self, bytes: &'a [u8], timestamp: i64) -> NetworkResult<'a> {
        if bytes.len() < 20 || bytes[0] >> 4 != 4 {
            return status("malformed");
        }
        let header = usize::from(bytes[0] & 15) * 4;
        let length = usize::from(u16_at(bytes, 2));
        if header < 20 || length < header || length > bytes.len() {
            return status("malformed");
        }
        if bytes[9] != 17 {
            return status("ignored");
        }
        let source = Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]);
        let destination = Ipv4Addr::new(bytes[16], bytes[17], bytes[18], bytes[19]);
        let flags = u16_at(bytes, 6);
        let payload = &bytes[header..length];
        if flags & 0x3fff == 0 {
            return udp(payload, timestamp, source.into(), destination.into());
        }
        let key = (source, destination, u16_at(bytes, 4));
        let offset = usize::from(flags & 0x1fff) * 8;
        let more = flags & 0x2000 != 0;
        if payload.is_empty() || offset + payload.len() > 65_515 || (more && !payload.len().is_multiple_of(8)) {
            self.drop_set(&key);
            return status("malformed");
        }
        if !self.fragments.contains_key(&key) {
            if self.fragments.len() >= 128 {
                let oldest = self.fragments.iter().min_by_key(|(_, set)| set.timestamp).map(|(key, _)| *key).unwrap();
                self.drop_set(&oldest);
                self.evicted_fragments += 1;
            }
            let latest = self.latest_timestamp;
            let set = match self.pool.pop() {
                Some(mut set) => {
                    set.received.fill(false);
                    set.timestamp = latest;
                    set.count = 0;
                    set.highest = 0;
                    set.length = None;
                    set
                }
                None => FragmentSet {
                    timestamp: latest,
                    bytes: vec![0; 65_535].into_boxed_slice(),
                    received: vec![false; 65_535].into_boxed_slice(),
                    count: 0,
                    highest: 0,
                    length: None,
                },
            };
            self.fragments.insert(key, set);
        }
        let set = self.fragments.get_mut(&key).unwrap();
        let end = offset + payload.len();
        if set.length.is_some_and(|known| end > known || (!more && end != known)) || (!more && set.highest > end) {
            self.drop_set(&key);
            return status("fragmentConflict");
        }
        for (i, &byte) in payload.iter().enumerate() {
            let position = offset + i;
            if set.received[position] {
                if set.bytes[position] != byte {
                    self.drop_set(&key);
                    return status("fragmentConflict");
                }
            } else {
                set.received[position] = true;
                set.bytes[position] = byte;
                set.count += 1;
            }
        }
        set.highest = set.highest.max(end);
        if !more {
            set.length = Some(end);
        }
        match set.length {
            Some(total) if set.count == total => {
                let set = self.fragments.remove(&key).unwrap();
                let assembled = self.assembled.insert(set);
                udp(&assembled.bytes[..total], timestamp, source.into(), destination.into())
            }
            _ => status("fragmentPending"),
        }
    }
}

fn ipv6(bytes: &[u8], timestamp: i64) -> NetworkResult<'_> {
    if bytes.len() < 40 || bytes[0] >> 4 != 6 {
        return status("malformed");
    }
    let length = 40 + usize::from(u16_at(bytes, 4));
    if length > bytes.len() {
        return status("malformed");
    }
    let (mut next, mut offset) = (bytes[6], 40);
    let mut count = 0;
    while matches!(next, 0 | 43 | 60 | 51) {
        if count >= 8 || offset + 2 > length {
            return status("malformed");
        }
        let extension = if next == 51 { (usize::from(bytes[offset + 1]) + 2) * 4 } else { (usize::from(bytes[offset + 1]) + 1) * 8 };
        next = bytes[offset];
        offset += extension;
        if offset > length {
            return status("malformed");
        }
        count += 1;
    }
    if next == 44 {
        return status("unsupportedIpv6Fragment");
    }
    if next != 17 {
        return status("ignored");
    }
    let address = |start: usize| IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[start..start + 16]).unwrap()));
    udp(&bytes[offset..length], timestamp, address(8), address(24))
}

fn udp(bytes: &[u8], timestamp: i64, source: IpAddr, destination: IpAddr) -> NetworkResult<'_> {
    if bytes.len() < 8 {
        return status("malformed");
    }
    let length = usize::from(u16_at(bytes, 4));
    if length < 8 || length > bytes.len() {
        return status("malformed");
    }
    let (source_port, destination_port) = (u16_at(bytes, 0), u16_at(bytes, 2));
    if !is_game_port(source_port) && !is_game_port(destination_port) {
        return status("ignored");
    }
    NetworkResult {
        status: "udp",
        datagram: Some(UdpDatagram { timestamp_us: timestamp, source, destination, source_port, destination_port, payload: &bytes[8..length] }),
    }
}

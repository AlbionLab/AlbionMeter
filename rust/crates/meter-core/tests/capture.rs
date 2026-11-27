//! tests/AlbionMeter.Tests/CaptureChecks.cs: network, Photon and capture-file decoding.

mod common;

use meter_core::capture::network::NetworkDecoder;
use meter_core::capture::pcap::{PcapFile, write_pcap};
use meter_core::capture::photon::PhotonTransport;
use meter_core::capture::network::UdpDatagram;
use meter_core::testkit::{fragment, frame, ip, photon, put16, put32, udp};
use meter_core::{CaptureAnalysis, CaptureFrame};
use std::net::IpAddr;

fn datagram(bytes: &[u8]) -> UdpDatagram<'_> {
    UdpDatagram {
        timestamp_us: 1_000_000,
        source: "192.0.2.1".parse().unwrap(),
        destination: "192.0.2.2".parse().unwrap(),
        source_port: 5056,
        destination_port: 12345,
        payload: bytes,
    }
}

/// A tiny xorshift so the random checks are deterministic without a dependency.
struct Random(u64);

impl Random {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below as u64) as usize
    }
    fn bytes(&mut self, count: usize) -> Vec<u8> {
        (0..count).map(|_| self.next(256) as u8).collect()
    }
}

#[test]
fn ipv4_udp_payloads_preserve_endpoints_and_timestamps() {
    let frame = frame(ip(&udp(&[1, 2, 3]), 0, 0));
    let mut decoder = NetworkDecoder::new();
    let result = decoder.decode(&frame);
    let datagram = result.datagram.unwrap();
    assert!(result.status == "udp" && datagram.source_port == 5056 && datagram.timestamp_us == 1_000_000);
    assert_eq!(datagram.payload, [1, 2, 3]);
}

#[test]
fn linux_any_interface_frames_decode_with_cooked_headers() {
    let packet = ip(&udp(&[7, 8, 9]), 0, 0);
    let mut sll = vec![0u8; 16 + packet.len()];
    put16(&mut sll, 14, 0x0800);
    sll[16..].copy_from_slice(&packet);
    let mut sll2 = vec![0u8; 20 + packet.len()];
    put16(&mut sll2, 0, 0x0800);
    sll2[20..].copy_from_slice(&packet);
    for (link_type, bytes) in [(113, sll), (276, sll2)] {
        let frame = CaptureFrame { timestamp_us: 1_000_000, original_length: bytes.len() as u32, link_type, data: bytes };
        let mut decoder = NetworkDecoder::new();
        let result = decoder.decode(&frame);
        assert!(result.status == "udp" && result.datagram.unwrap().source_port == 5056 && result.datagram.unwrap().payload == [7, 8, 9]);
    }
}

#[test]
fn truncated_and_malformed_frames_are_counted_separately() {
    let mut decoder = NetworkDecoder::new();
    let mut frame = frame(ip(&udp(&[1]), 0, 0));
    assert_eq!(decoder.decode(&CaptureFrame { original_length: 100, ..frame.clone() }).status, "truncated");
    frame.data[2] = 255;
    assert_eq!(decoder.decode(&frame).status, "malformed");
    assert_eq!(decoder.decode(&common_frame(vec![])).status, "malformed");
    assert_eq!(decoder.decode(&CaptureFrame { link_type: 9999, ..common_frame(vec![0]) }).status, "unsupportedLink");
}

fn common_frame(bytes: Vec<u8>) -> CaptureFrame {
    frame(bytes)
}

#[test]
fn ethernet_vlan_headers_resolve_the_underlying_ipv4_packet() {
    let ip = ip(&udp(&[42]), 0, 0);
    let mut ethernet = vec![0u8; 18 + ip.len()];
    put16(&mut ethernet, 12, 0x8100);
    put16(&mut ethernet, 16, 0x0800);
    ethernet[18..].copy_from_slice(&ip);
    let frame = CaptureFrame { link_type: 1, ..frame(ethernet) };
    assert_eq!(NetworkDecoder::new().decode(&frame).datagram.unwrap().payload[0], 42);
}

#[test]
fn ipv6_udp_and_extension_header_lengths_are_validated() {
    let udp = udp(&[42]);
    let mut ip = vec![0u8; 48 + udp.len()];
    ip[0] = 0x60;
    ip[6] = 0;
    let length = (ip.len() - 40) as u16;
    put16(&mut ip, 4, length);
    ip[40] = 17;
    ip[48..].copy_from_slice(&udp);
    assert_eq!(NetworkDecoder::new().decode(&frame(ip.clone())).datagram.unwrap().payload[0], 42);
    ip[41] = 200;
    assert_eq!(NetworkDecoder::new().decode(&frame(ip)).status, "malformed");
}

#[test]
fn ipv4_fragments_reassemble_out_of_order_without_double_counting() {
    let udp = udp(&[1, 2, 3, 4]);
    let mut decoder = NetworkDecoder::new();
    let tail = frame(ip(&udp[8..], 1, 0));
    let head = frame(ip(&udp[..8], 0x2000, 0));
    assert_eq!(decoder.decode(&tail).status, "fragmentPending");
    assert_eq!(decoder.decode(&tail).status, "fragmentPending");
    assert_eq!(decoder.decode(&head).datagram.unwrap().payload, [1, 2, 3, 4]);
    assert_eq!(decoder.pending_fragments(), 0);
}

#[test]
fn conflicting_ipv4_overlaps_are_rejected_and_incomplete_assemblies_expire() {
    let mut decoder = NetworkDecoder::new();
    let mut head = frame(ip(&udp(&[1, 2, 3, 4])[..8], 0x2000, 0));
    decoder.decode(&head);
    *head.data.last_mut().unwrap() ^= 1;
    assert_eq!(decoder.decode(&head).status, "fragmentConflict");
    decoder.decode(&head);
    decoder.decode(&CaptureFrame { timestamp_us: 17_000_001, ..frame(ip(&udp(&[9]), 0, 0)) });
    assert!(decoder.expired_fragments == 1 && decoder.pending_fragments() == 0);
}

#[test]
fn ip_fragment_storage_is_bounded_under_many_incomplete_datagrams() {
    let mut decoder = NetworkDecoder::new();
    for id in 0..200u16 {
        decoder.decode(&frame(ip(&[0u8; 8], 0x2000, id)));
    }
    assert!(decoder.pending_fragments() == 128 && decoder.evicted_fragments == 72);
}

#[test]
fn retransmissions_are_deduplicated_by_connection_channel_and_sequence() {
    let mut decoder = PhotonTransport::new();
    let bytes = photon(&[0xf3, 4, 1], 10);
    decoder.decode(&datagram(&bytes));
    assert_eq!(decoder.messages().len(), 1);
    decoder.decode(&datagram(&bytes));
    assert_eq!(decoder.messages().len(), 0);
    assert_eq!(decoder.duplicates, 1);
    decoder.decode(&datagram(&photon(&[0xf3, 4, 1], 11)));
    assert_eq!(decoder.messages().len(), 1);
}

#[test]
fn a_route_changer_moving_the_connection_to_other_addresses_keeps_it_one_connection() {
    // ExitLag, NoPing and similar tools switch routes (other IP or port) or send copies over several at once.
    let mut decoder = PhotonTransport::new();
    let bytes = photon(&[0xf3, 4, 1], 10);
    decoder.decode(&datagram(&bytes));
    let first = decoder.messages()[0].connection.clone();
    let moved_source: IpAddr = "198.51.100.7".parse().unwrap();
    decoder.decode(&UdpDatagram { source: moved_source, destination_port: 40000, ..datagram(&bytes) });
    assert!(decoder.messages().is_empty());
    let next = photon(&[0xf3, 4, 1], 12);
    decoder.decode(&UdpDatagram { source: moved_source, destination_port: 40000, ..datagram(&next) });
    assert_eq!(decoder.messages()[0].connection, first);
    decoder.decode(&UdpDatagram {
        source: "192.0.2.2".parse().unwrap(),
        source_port: 12345,
        destination: "192.0.2.1".parse().unwrap(),
        destination_port: 5056,
        ..datagram(&next)
    });
    assert_eq!(decoder.messages().len(), 1);
    assert_ne!(decoder.messages()[0].connection, first);
}

#[test]
fn malformed_trailing_data_cannot_consume_a_valid_command_identity() {
    let mut decoder = PhotonTransport::new();
    let payload = photon(&[0xf3, 4, 1, 0], 5);
    let mut trailing = payload.clone();
    trailing.push(1);
    assert_eq!(decoder.decode(&datagram(&trailing)), "malformedPhoton");
    decoder.decode(&datagram(&payload));
    assert_eq!(decoder.messages().len(), 1);
}

#[test]
fn photon_challenges_isolate_new_connections_sharing_endpoints() {
    let mut decoder = PhotonTransport::new();
    let mut bytes = photon(&[0xf3, 4, 1], 1);
    decoder.decode(&datagram(&bytes));
    bytes[11] = 2;
    decoder.decode(&datagram(&bytes));
    assert_eq!(decoder.messages().len(), 1);
}

#[test]
fn photon_fragments_reassemble_by_connection_and_reject_conflicting_data() {
    let mut decoder = PhotonTransport::new();
    decoder.decode(&datagram(&fragment(&[4, 5], 1, 3, 5)));
    assert!(decoder.messages().is_empty());
    decoder.decode(&datagram(&fragment(&[1, 2, 3], 0, 0, 5)));
    assert_eq!(decoder.messages().len(), 1);
    assert_eq!(decoder.payload(&decoder.messages()[0]), [1, 2, 3, 4, 5]);
    decoder.decode(&datagram(&fragment(&[1, 2, 3], 0, 0, 5)));
    assert!(decoder.messages().is_empty());
    let mut decoder = PhotonTransport::new();
    decoder.decode(&datagram(&fragment(&[1, 2, 3], 0, 0, 5)));
    assert_eq!(decoder.decode(&datagram(&fragment(&[9, 2, 3], 0, 0, 5))), "photonFragmentConflict");
}

#[test]
fn invalid_photon_lengths_and_oversized_fragment_allocation_are_rejected() {
    let mut decoder = PhotonTransport::new();
    let mut bytes = photon(&[1], 1);
    put32(&mut bytes, 16, u32::MAX);
    assert_eq!(decoder.decode(&datagram(&bytes)), "malformedPhoton");
    assert_eq!(decoder.decode(&datagram(&fragment(&[1], 0, 0, 2_000_000))), "malformedPhotonFragment");
    assert_eq!(decoder.pending_assemblies(), 0);
}

#[test]
fn replay_diagnostics_are_deterministic_and_never_claim_combat_decoding() {
    let frames = [frame(ip(&udp(&photon(&[0xf3, 4, 1], 1)), 0, 0)), frame(vec![1, 2])];
    let (mut first, mut second) = (CaptureAnalysis::new(None, false), CaptureAnalysis::new(None, false));
    for frame in &frames {
        first.add(frame);
        second.add(frame);
    }
    let (a, b) = (first.snapshot(), second.snapshot());
    assert!(a.frames == 2 && a.counters.get("transportMessages") == Some(&1) && a.counters.get("malformed") == Some(&1));
    assert!(!a.combat_decoded && a.counters.iter().eq(b.counters.iter()));
}

#[test]
fn random_short_packet_inputs_never_escape_decoder_validation() {
    let mut random = Random(19);
    let (mut network, mut transport) = (NetworkDecoder::new(), PhotonTransport::new());
    for _ in 0..10_000 {
        let count = random.next(300);
        let bytes = random.bytes(count);
        network.decode(&frame(bytes.clone()));
        transport.decode(&datagram(&bytes));
    }
}

#[test]
fn the_built_in_capture_file_reader_reads_pcap_and_pcapng_frames_exactly() {
    let first = frame(ip(&udp(&photon(&[0xf3, 4, 1], 1)), 0, 0));
    let second = CaptureFrame { timestamp_us: 2_500_001, ..first.clone() };
    let mut reader = PcapFile::from_bytes(write_pcap(&[first.clone(), second.clone()], 101)).unwrap();
    assert_eq!(reader.link_type(), 101);
    let one = reader.read().unwrap().unwrap();
    let two = reader.read().unwrap().unwrap();
    assert!(reader.read().unwrap().is_none());
    assert!(one.data == first.data && one.timestamp_us == 1_000_000 && two.timestamp_us == 2_500_001 && two.original_length == second.original_length);

    let packet = first.data.clone();
    let padded = packet.len().div_ceil(4) * 4;
    let mut out = Vec::new();
    let w32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    w32(&mut out, 0x0a0d0d0a);
    w32(&mut out, 28);
    w32(&mut out, 0x1a2b3c4d);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(-1i64).to_le_bytes());
    w32(&mut out, 28);
    w32(&mut out, 1);
    w32(&mut out, 28);
    out.extend_from_slice(&101u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    w32(&mut out, 262_144);
    out.extend_from_slice(&9u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&[9, 0, 0, 0]);
    w32(&mut out, 28);
    let nanoseconds = 3_000_000_500_000i64;
    let length = (32 + padded) as u32;
    w32(&mut out, 6);
    w32(&mut out, length);
    w32(&mut out, 0);
    w32(&mut out, (nanoseconds >> 32) as u32);
    w32(&mut out, nanoseconds as u32);
    w32(&mut out, packet.len() as u32);
    w32(&mut out, packet.len() as u32);
    out.extend_from_slice(&packet);
    out.extend(std::iter::repeat_n(0, padded - packet.len()));
    w32(&mut out, length);
    let mut reader = PcapFile::from_bytes(out).unwrap();
    assert_eq!(reader.link_type(), 101);
    let read = reader.read().unwrap().unwrap();
    assert!(reader.read().unwrap().is_none());
    assert!(read.data == packet && read.timestamp_us == 3_000_000_500 && read.link_type == 101);
    assert!(PcapFile::from_bytes(vec![1, 2, 3, 4, 5]).is_err());
}

#[test]
fn the_reader_reads_a_file_on_disk_and_reuses_its_frame_buffer() {
    let path = std::env::temp_dir().join(format!("albion-reader-{}.pcap", meter_core::cs::Guid::new_v4().simple()));
    let first = frame(ip(&udp(&photon(&[0xf3, 4, 1], 1)), 0, 0));
    std::fs::write(&path, write_pcap(&[first.clone(), first.clone()], 101)).unwrap();
    let mut reader = PcapFile::open(&path).unwrap();
    let mut buffer = CaptureFrame { timestamp_us: 0, original_length: 0, link_type: 0, data: Vec::with_capacity(4096) };
    let pointer = buffer.data.as_ptr();
    assert!(reader.read_into(&mut buffer).unwrap() && reader.read_into(&mut buffer).unwrap() && !reader.read_into(&mut buffer).unwrap());
    assert!(buffer.data == first.data && buffer.data.as_ptr() == pointer);
    std::fs::remove_file(path).unwrap();
}

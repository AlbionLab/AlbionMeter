/// The pcap filter: Albion's UDP ports over IPv4 and IPv6, plus every IPv4 fragment (the ports are only in the first).
pub const DEFAULT_FILTER: &str = "(ip and (udp port 5055 or udp port 5056 or udp port 5058 or (ip[6:2] & 0x3fff != 0))) or (ip6 and (udp port 5055 or udp port 5056 or udp port 5058))";

/// Classic BPF over the IPv4 header (the cooked socket starts at the network layer): IPv4 fragments, and UDP to or
/// from the game ports 5055, 5056 and 5058. Anything else never reaches the socket queue. `(code, jt, jf, k)`.
pub const LINUX_FILTER: [(u16, u8, u8, u32); 18] = [
    (0x30, 0, 0, 0),       // 0  A = ip[0]
    (0x54, 0, 0, 0xf0),    // 1  A &= 0xf0
    (0x15, 0, 13, 0x40),   // 2  version 4, or drop
    (0x28, 0, 0, 6),       // 3  A = ip[6:2]
    (0x45, 12, 0, 0x3fff), // 4  a fragment: accept
    (0x30, 0, 0, 9),       // 5  A = ip[9]
    (0x15, 0, 9, 17),      // 6  UDP, or drop
    (0xb1, 0, 0, 0),       // 7  X = IP header length
    (0x48, 0, 0, 0),       // 8  A = source port
    (0x15, 7, 0, 5055),    // 9
    (0x15, 6, 0, 5056),    // 10
    (0x15, 5, 0, 5058),    // 11
    (0x48, 0, 0, 2),       // 12 A = destination port
    (0x15, 3, 0, 5055),    // 13
    (0x15, 2, 0, 5056),    // 14
    (0x15, 1, 0, 5058),    // 15
    (0x06, 0, 0, 0),       // 16 drop
    (0x06, 0, 0, 0x40000), // 17 accept
];

/// `PacketCapture.IsAlbionPacket`: an IPv4 packet that is a fragment or UDP to/from a game port.
pub fn is_albion_packet(packet: &[u8]) -> bool {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return false;
    }
    let header = usize::from(packet[0] & 0x0f) * 4;
    if header < 20 || packet.len() < header {
        return false;
    }
    if u16::from_be_bytes([packet[6], packet[7]]) & 0x3fff != 0 {
        return true;
    }
    if packet[9] != 17 || packet.len() < header + 4 {
        return false;
    }
    let source = u16::from_be_bytes([packet[header], packet[header + 1]]);
    let destination = u16::from_be_bytes([packet[header + 2], packet[header + 3]]);
    matches!(source, 5055 | 5056 | 5058) || matches!(destination, 5055 | 5056 | 5058)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn ipv4(
        protocol: u8,
        fragment: u16,
        source: u16,
        destination: u16,
        options: usize,
    ) -> Vec<u8> {
        let header = 20 + options * 4;
        let mut packet = vec![0u8; header + 8 + 12];
        packet[0] = 0x40 | (5 + options as u8);
        let total = packet.len() as u16;
        packet[2..4].copy_from_slice(&total.to_be_bytes());
        packet[6..8].copy_from_slice(&fragment.to_be_bytes());
        packet[8] = 64;
        packet[9] = protocol;
        packet[12..16].copy_from_slice(&[10, 0, 0, 2]);
        packet[16..20].copy_from_slice(&[5, 188, 125, 10]);
        packet[header..header + 2].copy_from_slice(&source.to_be_bytes());
        packet[header + 2..header + 4].copy_from_slice(&destination.to_be_bytes());
        packet
    }

    /// The packets both filters must agree on, with the verdict `IsAlbionPacket` gives.
    pub(crate) fn cases() -> Vec<(&'static str, Vec<u8>, bool)> {
        let mut ipv6 = vec![0u8; 48];
        ipv6[0] = 0x60;
        ipv6[6] = 17;
        ipv6[40..42].copy_from_slice(&5056u16.to_be_bytes());
        vec![
            ("udp from 5056", ipv4(17, 0, 5056, 50000, 0), true),
            ("udp to 5055", ipv4(17, 0, 50000, 5055, 0), true),
            (
                "udp to 5058 with options",
                ipv4(17, 0, 50000, 5058, 2),
                true,
            ),
            ("udp to 5057", ipv4(17, 0, 50000, 5057, 0), false),
            ("udp dns", ipv4(17, 0, 53, 40000, 0), false),
            ("tcp on 5056", ipv4(6, 0, 5056, 5056, 0), false),
            ("more fragments flag", ipv4(17, 0x2000, 1, 2, 0), true),
            ("fragment offset", ipv4(6, 0x0010, 1, 2, 0), true),
            ("don't fragment only", ipv4(17, 0x4000, 1, 2, 0), false),
            ("don't fragment game", ipv4(17, 0x4000, 5056, 2, 0), true),
            ("ipv6 game port", ipv6, false),
        ]
    }

    #[test]
    fn classifies_like_packet_capture() {
        for (name, packet, expected) in cases() {
            assert_eq!(is_albion_packet(&packet), expected, "{name}");
        }
    }

    #[test]
    fn rejects_malformed_headers() {
        assert!(!is_albion_packet(&[]));
        assert!(!is_albion_packet(&[0x45; 19]));
        let mut short_ihl = ipv4(17, 0, 5056, 1, 0);
        short_ihl[0] = 0x44;
        assert!(!is_albion_packet(&short_ihl));
        let mut long_ihl = ipv4(17, 0, 5056, 1, 0);
        long_ihl[0] = 0x4f;
        assert!(!is_albion_packet(&long_ihl[..40]));
        // UDP header cut before the ports.
        assert!(!is_albion_packet(&ipv4(17, 0, 5056, 1, 0)[..22]));
    }

    /// The opcodes `LINUX_FILTER` uses, run as the kernel would; an out-of-range load drops the packet.
    fn run_bpf(packet: &[u8]) -> u32 {
        let (mut a, mut x, mut pc) = (0u32, 0u32, 0usize);
        let byte = |at: usize| packet.get(at).map(|&b| u32::from(b));
        let half = |at: usize| {
            Some(u32::from(u16::from_be_bytes([
                *packet.get(at)?,
                *packet.get(at + 1)?,
            ])))
        };
        loop {
            let (code, jt, jf, k) = LINUX_FILTER[pc];
            pc += 1;
            match code {
                0x30 => match byte(k as usize) {
                    Some(v) => a = v,
                    None => return 0,
                },
                0x28 => match half(k as usize) {
                    Some(v) => a = v,
                    None => return 0,
                },
                0x48 => match half((x + k) as usize) {
                    Some(v) => a = v,
                    None => return 0,
                },
                0xb1 => match byte(k as usize) {
                    Some(v) => x = (v & 0x0f) * 4,
                    None => return 0,
                },
                0x54 => a &= k,
                0x15 => pc += usize::from(if a == k { jt } else { jf }),
                0x45 => pc += usize::from(if a & k != 0 { jt } else { jf }),
                0x06 => return k,
                _ => panic!("opcode {code:#x}"),
            }
        }
    }

    #[test]
    fn kernel_filter_matches_user_space_check() {
        for (name, packet, expected) in cases() {
            assert_eq!(run_bpf(&packet) != 0, expected, "{name}");
        }
        assert_eq!(run_bpf(&ipv4(17, 0, 5056, 1, 0)), 0x40000);
    }

    #[cfg(feature = "pcap")]
    #[test]
    fn pcap_filter_matches_user_space_check() {
        // libpcap takes the DLT: DLT_RAW is 12 for the file format's LINKTYPE_RAW (101).
        let capture = pcap::Capture::dead(pcap::Linktype(12)).unwrap();
        let program = capture.compile(DEFAULT_FILTER, true).unwrap();
        for (name, packet, expected) in cases() {
            // The pcap filter also takes IPv6 on the game ports; user space then drops it.
            let ipv6 = packet[0] >> 4 == 6;
            assert_eq!(program.filter(&packet), expected || ipv6, "{name}");
        }
    }
}

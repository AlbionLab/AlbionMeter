use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::CaptureFrame;

const MAXIMUM_PACKET: usize = 262_144;

/// `PcapWriter`: a classic little-endian microsecond pcap, one link type per file.
pub struct PcapWriter<W: Write = BufWriter<File>> {
    out: W,
    link_type: i32,
    bytes_written: u64,
}

impl PcapWriter {
    /// Creates `path`; an existing file is an error, never overwritten.
    pub fn create(path: impl AsRef<Path>, link_type: i32) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        Self::new(BufWriter::with_capacity(64 * 1024, file), link_type)
    }
}

impl<W: Write> PcapWriter<W> {
    pub fn new(mut out: W, link_type: i32) -> io::Result<Self> {
        let mut header = [0u8; 24];
        header[0..4].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        header[4..6].copy_from_slice(&2u16.to_le_bytes());
        header[6..8].copy_from_slice(&4u16.to_le_bytes());
        header[16..20].copy_from_slice(&(MAXIMUM_PACKET as u32).to_le_bytes());
        header[20..24].copy_from_slice(&link_type.to_le_bytes());
        out.write_all(&header)?;
        Ok(Self {
            out,
            link_type,
            bytes_written: 24,
        })
    }

    pub fn write(&mut self, frame: &CaptureFrame) -> io::Result<()> {
        let seconds = frame.timestamp_us.div_euclid(1_000_000);
        if frame.link_type != self.link_type
            || frame.data.len() > MAXIMUM_PACKET
            || frame.data.len() as u64 > u64::from(frame.original_length)
            || frame.timestamp_us < 0
            || seconds > i64::from(u32::MAX)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The packet cannot be represented in this capture file.",
            ));
        }
        let mut record = [0u8; 16];
        record[0..4].copy_from_slice(&(seconds as u32).to_le_bytes());
        record[4..8].copy_from_slice(&((frame.timestamp_us % 1_000_000) as u32).to_le_bytes());
        record[8..12].copy_from_slice(&(frame.data.len() as u32).to_le_bytes());
        record[12..16].copy_from_slice(&frame.original_length.to_le_bytes());
        self.out.write_all(&record)?;
        self.out.write_all(&frame.data)?;
        self.bytes_written += 16 + frame.data.len() as u64;
        Ok(())
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames() -> Vec<CaptureFrame> {
        vec![
            CaptureFrame {
                timestamp_us: 1_732_000_000_123_456,
                original_length: 4,
                link_type: 101,
                data: vec![0x45, 0, 0, 4],
            },
            CaptureFrame {
                timestamp_us: 1_732_000_001_000_000,
                original_length: 1500,
                link_type: 101,
                data: vec![7; 64],
            },
            CaptureFrame {
                timestamp_us: 0,
                original_length: 0,
                link_type: 101,
                data: vec![],
            },
        ]
    }

    /// The reader side lives in meter-core; this one only checks the bytes this writer lays out.
    fn parse(bytes: &[u8]) -> (i32, Vec<CaptureFrame>) {
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(0), 0xa1b2_c3d4);
        assert_eq!(&bytes[4..8], &[2, 0, 4, 0]);
        assert_eq!(u32_at(16), 262_144);
        let link_type = u32_at(20) as i32;
        let (mut at, mut frames) = (24, vec![]);
        while at < bytes.len() {
            let captured = u32_at(at + 8) as usize;
            frames.push(CaptureFrame {
                timestamp_us: i64::from(u32_at(at)) * 1_000_000 + i64::from(u32_at(at + 4)),
                original_length: u32_at(at + 12),
                link_type,
                data: bytes[at + 16..at + 16 + captured].to_vec(),
            });
            at += 16 + captured;
        }
        (link_type, frames)
    }

    #[test]
    fn round_trips_frames() {
        let mut writer = PcapWriter::new(Vec::new(), 101).unwrap();
        for frame in frames() {
            writer.write(&frame).unwrap();
        }
        assert_eq!(writer.bytes_written(), 24 + 16 * 3 + 4 + 64);
        let bytes = writer.into_inner();
        assert_eq!(bytes.len(), 24 + 16 * 3 + 4 + 64);
        assert_eq!(parse(&bytes), (101, frames()));
    }

    #[test]
    fn refuses_what_the_file_cannot_hold() {
        let mut writer = PcapWriter::new(Vec::new(), 101).unwrap();
        let good = frames().remove(0);
        for bad in [
            CaptureFrame {
                link_type: 1,
                ..good.clone()
            },
            CaptureFrame {
                original_length: 3,
                ..good.clone()
            },
            CaptureFrame {
                timestamp_us: -1,
                ..good.clone()
            },
            CaptureFrame {
                timestamp_us: (i64::from(u32::MAX) + 1) * 1_000_000,
                ..good.clone()
            },
            CaptureFrame {
                data: vec![0; 262_145],
                original_length: 262_145,
                ..good.clone()
            },
        ] {
            assert_eq!(
                writer.write(&bad).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(writer.bytes_written(), 24);
        assert_eq!(writer.into_inner().len(), 24);
    }

    #[test]
    fn create_never_overwrites() {
        let directory = crate::storage::tests::scratch("writer");
        let path = directory.join("capture.pcap");
        let mut writer = PcapWriter::create(&path, 101).unwrap();
        for frame in frames() {
            writer.write(&frame).unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(
            PcapWriter::create(&path, 101)
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::AlreadyExists)
        );
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(parse(&bytes).1, frames());

        // libpcap reads it back the same (on Windows only where Npcap is installed: a CI runner has none).
        #[cfg(feature = "pcap")]
        if crate::native_pcap::available().is_ok() {
            let mut file = pcap::Capture::from_file(&path).unwrap();
            // LINKTYPE_RAW in the file is DLT_RAW (12) inside libpcap.
            assert_eq!(file.get_datalink(), pcap::Linktype(12));
            for expected in frames() {
                let packet = file.next_packet().unwrap();
                #[allow(clippy::unnecessary_cast)]
                let stamp =
                    packet.header.ts.tv_sec as i64 * 1_000_000 + packet.header.ts.tv_usec as i64;
                assert_eq!(
                    (stamp, packet.header.len, packet.data),
                    (
                        expected.timestamp_us,
                        expected.original_length,
                        &expected.data[..]
                    )
                );
            }
            assert!(matches!(
                file.next_packet(),
                Err(pcap::Error::NoMorePackets)
            ));
        }
        std::fs::remove_dir_all(directory).unwrap();
    }}

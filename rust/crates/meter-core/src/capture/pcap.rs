//! The built-in PCAP/PCAPNG reader (`PcapFile` in PacketCapture.cs), used to replay recorded captures.

use meter_capture::CaptureFrame;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

const MAXIMUM_PACKET: u32 = 262_144;

pub struct PcapFile<R = BufReader<File>> {
    reader: R,
    position: u64,
    length: u64,
    next: bool,
    swapped: bool,
    divisor: i64,
    interfaces: Vec<(i32, i64)>,
    body: Vec<u8>,
    link_type: i32,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn truncated() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "The capture file is truncated.")
}

impl PcapFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Self::new(BufReader::with_capacity(65_536, file), length)
    }
}

impl PcapFile<io::Cursor<Vec<u8>>> {
    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        let length = bytes.len() as u64;
        Self::new(io::Cursor::new(bytes), length)
    }
}

impl<R: Read> PcapFile<R> {
    pub fn new(reader: R, length: u64) -> io::Result<Self> {
        let mut file = Self {
            reader,
            position: 0,
            length,
            next: false,
            swapped: false,
            divisor: 1,
            interfaces: Vec::new(),
            body: Vec::new(),
            link_type: 0,
        };
        match file.raw_u32()? {
            magic @ (0xa1b2c3d4 | 0xd4c3b2a1) => {
                file.swapped = magic == 0xd4c3b2a1;
                file.classic_header()?;
            }
            magic @ (0xa1b23c4d | 0x4d3cb2a1) => {
                file.swapped = magic == 0x4d3cb2a1;
                file.divisor = 1000;
                file.classic_header()?;
            }
            0x0a0d0d0a => {
                file.next = true;
                file.section_header()?;
                while file.interfaces.is_empty() && file.block(None)? {}
                if file.interfaces.is_empty() {
                    return Err(invalid("The PCAPNG file has no interface."));
                }
                file.link_type = file.interfaces[0].0;
            }
            _ => return Err(invalid("The file is not a PCAP or PCAPNG capture.")),
        }
        Ok(file)
    }

    pub fn link_type(&self) -> i32 {
        self.link_type
    }

    /// Reads the next frame into `frame`, reusing its buffer. `Ok(false)` at the end of the file.
    pub fn read_into(&mut self, frame: &mut CaptureFrame) -> io::Result<bool> {
        if self.next {
            loop {
                if self.position >= self.length {
                    return Ok(false);
                }
                let mut produced = false;
                if !self.block(Some((frame, &mut produced)))? {
                    return Ok(false);
                }
                if produced {
                    return Ok(true);
                }
            }
        }
        if self.position >= self.length {
            return Ok(false);
        }
        let (seconds, fraction, captured, original) = (self.u32()?, self.u32()?, self.u32()?, self.u32()?);
        if captured > MAXIMUM_PACKET || captured > original || i64::from(fraction) >= 1_000_000 * self.divisor {
            return Err(invalid("The capture contains an invalid packet header."));
        }
        frame.data.resize(captured as usize, 0);
        self.exact_into_frame(frame)?;
        frame.timestamp_us = i64::from(seconds) * 1_000_000 + i64::from(fraction) / self.divisor;
        frame.original_length = original;
        frame.link_type = self.link_type;
        Ok(true)
    }

    /// Reads the next frame into a new buffer.
    pub fn read(&mut self) -> io::Result<Option<CaptureFrame>> {
        let mut frame = CaptureFrame { timestamp_us: 0, original_length: 0, link_type: 0, data: Vec::new() };
        Ok(self.read_into(&mut frame)?.then_some(frame))
    }

    fn exact_into_frame(&mut self, frame: &mut CaptureFrame) -> io::Result<()> {
        self.reader.read_exact(&mut frame.data).map_err(|_| truncated())?;
        self.position += frame.data.len() as u64;
        Ok(())
    }

    fn skip(&mut self, count: u64) -> io::Result<()> {
        let copied = io::copy(&mut (&mut self.reader).take(count), &mut io::sink())?;
        self.position += copied;
        if copied < count { Err(truncated()) } else { Ok(()) }
    }

    fn raw_u32(&mut self) -> io::Result<u32> {
        let mut bytes = [0u8; 4];
        self.reader.read_exact(&mut bytes).map_err(|_| truncated())?;
        self.position += 4;
        Ok(u32::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> io::Result<u32> {
        let value = self.raw_u32()?;
        Ok(if self.swapped { value.swap_bytes() } else { value })
    }

    fn classic_header(&mut self) -> io::Result<()> {
        self.skip(16)?;
        self.link_type = (self.u32()? & 0x0fff_ffff) as i32;
        Ok(())
    }

    fn section_header(&mut self) -> io::Result<()> {
        let mut length = self.raw_u32()?;
        let order = self.raw_u32()?;
        if order != 0x1a2b3c4d && order != 0x4d3c2b1a {
            return Err(invalid("The PCAPNG section header is invalid."));
        }
        self.swapped = order == 0x4d3c2b1a;
        if self.swapped {
            length = length.swap_bytes();
        }
        if !(28..=1 << 24).contains(&length) {
            return Err(invalid("The PCAPNG section header is invalid."));
        }
        self.skip(u64::from(length) - 12)?;
        self.interfaces.clear();
        Ok(())
    }

    fn field(&self, offset: usize) -> u32 {
        let bytes: [u8; 4] = self.body[offset..offset + 4].try_into().unwrap();
        if self.swapped { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) }
    }

    fn short(&self, offset: usize) -> u16 {
        let bytes: [u8; 2] = self.body[offset..offset + 2].try_into().unwrap();
        if self.swapped { u16::from_be_bytes(bytes) } else { u16::from_le_bytes(bytes) }
    }

    fn block(&mut self, mut output: Option<(&mut CaptureFrame, &mut bool)>) -> io::Result<bool> {
        if self.position >= self.length {
            return Ok(false);
        }
        let mut kind = self.raw_u32()?;
        if kind == 0x0a0d0d0a {
            self.section_header()?;
            return Ok(true);
        }
        if self.swapped {
            kind = kind.swap_bytes();
        }
        let length = self.u32()?;
        if length < 12 || length % 4 != 0 || length > MAXIMUM_PACKET + 64 && (kind == 6 || kind == 3) || length > 1 << 24 {
            return Err(invalid("The capture contains an invalid block."));
        }
        let mut body = std::mem::take(&mut self.body);
        body.resize(length as usize - 12, 0);
        let read = self.reader.read_exact(&mut body);
        self.body = body;
        read.map_err(|_| truncated())?;
        self.position += self.body.len() as u64;
        self.raw_u32()?;
        let size = self.body.len();
        match kind {
            1 if size >= 8 => {
                let mut divisor = 1i64;
                let mut offset = 8;
                while offset + 4 <= size {
                    let (code, length) = (self.short(offset), self.short(offset + 2) as usize);
                    if code == 0 || offset + 4 + length > size {
                        break;
                    }
                    if code == 9 && length >= 1 {
                        let resolution = self.body[offset + 4];
                        let units = if resolution & 0x80 == 0 { 10f64.powi(i32::from(resolution)) } else { 2f64.powi(i32::from(resolution & 0x7f)) };
                        divisor = (units / 1_000_000.0).max(1.0) as i64;
                    }
                    offset += 4 + length.div_ceil(4) * 4;
                }
                if self.interfaces.len() < 256 {
                    self.interfaces.push((i32::from(self.short(0)), divisor));
                }
            }
            6 if size >= 20 => {
                let index = self.field(0) as usize;
                if index >= self.interfaces.len() {
                    return Err(invalid("The capture refers to an unknown interface."));
                }
                let (link, divisor) = self.interfaces[index];
                let stamp = ((i64::from(self.field(4)) << 32) | i64::from(self.field(8))) / divisor;
                let (captured, original) = (self.field(12), self.field(16));
                if captured > MAXIMUM_PACKET || captured > original || 20 + captured as usize > size {
                    return Err(invalid("The capture contains an invalid packet header."));
                }
                if let Some((frame, produced)) = output.as_mut() {
                    frame.data.clear();
                    frame.data.extend_from_slice(&self.body[20..20 + captured as usize]);
                    frame.timestamp_us = stamp;
                    frame.original_length = original;
                    frame.link_type = link;
                    **produced = true;
                }
            }
            3 if size >= 4 && !self.interfaces.is_empty() => {
                let original = self.field(0);
                let captured = (original as usize).min(size - 4);
                if let Some((frame, produced)) = output.as_mut() {
                    frame.data.clear();
                    frame.data.extend_from_slice(&self.body[4..4 + captured]);
                    frame.timestamp_us = 0;
                    frame.original_length = original;
                    frame.link_type = self.interfaces[0].0;
                    **produced = true;
                }
            }
            _ => {}
        }
        Ok(true)
    }
}

/// Writes a classic microsecond PCAP file, as `PcapWriter.cs`; used for fixtures and the differential harness.
pub fn write_pcap(frames: &[CaptureFrame], link_type: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + frames.iter().map(|frame| 16 + frame.data.len()).sum::<usize>());
    out.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&262_144i32.to_le_bytes());
    out.extend_from_slice(&link_type.to_le_bytes());
    for frame in frames {
        out.extend_from_slice(&((frame.timestamp_us / 1_000_000) as u32).to_le_bytes());
        out.extend_from_slice(&((frame.timestamp_us % 1_000_000) as u32).to_le_bytes());
        out.extend_from_slice(&(frame.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&frame.original_length.to_le_bytes());
        out.extend_from_slice(&frame.data);
    }
    out
}

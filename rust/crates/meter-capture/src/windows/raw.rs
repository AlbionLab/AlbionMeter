//! `RawSocketCapture` and `AllAdaptersCapture`: SIO_RCVALL raw sockets, which need no driver but an administrator.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io::{self, Read as _};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::windows::io::AsRawSocket;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{
    AF_INET, POLLRDNORM, SIO_RCVALL, SOCKADDR, SOCKADDR_IN, SOCKET, WSAEACCES, WSAEMSGSIZE,
    WSAGetLastError, WSAIoctl, WSAPOLLFD, WSAPoll,
};

use crate::source::{now_us, os_text};
use crate::{
    AdapterStatistics, CaptureDevice, CaptureError, CaptureFrame, DEFAULT_ROUTE_FLAG, FrameSource,
    LINK_TYPE_RAW, RAW_SOCKET_PREFIX, Read, is_albion_packet,
};

pub const ALL: &str = "raw:all";
const RESCAN_EVERY: Duration = Duration::from_secs(5);
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

struct Adapter {
    name: String,
    routed: bool,
    addresses: Vec<Ipv4Addr>,
}

fn ipv4(address: *const SOCKADDR) -> Option<Ipv4Addr> {
    // SAFETY: the adapter list owns the address; an AF_INET sockaddr is a SOCKADDR_IN.
    unsafe {
        if address.is_null() || (*address).sa_family != AF_INET {
            return None;
        }
        Some(Ipv4Addr::from(u32::from_be(
            (*address.cast::<SOCKADDR_IN>()).sin_addr.S_un.S_addr,
        )))
    }
}

/// Adapters that are up and not loopback, with their IPv4 addresses and whether an IPv4 gateway sits behind them.
fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buffer: Vec<u64>;
    loop {
        buffer = vec![0; (size as usize).div_ceil(8)];
        // SAFETY: `buffer` holds `size` bytes, 8-aligned.
        let result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET.0),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast()),
                &mut size,
            )
        };
        if result == ERROR_SUCCESS.0 {
            break;
        }
        if result != ERROR_BUFFER_OVERFLOW.0 {
            return vec![];
        }
    }
    let mut adapters = vec![];
    let mut next = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    // SAFETY: a linked list inside `buffer`, which outlives the walk.
    unsafe {
        while let Some(adapter) = next.as_ref() {
            next = adapter.Next;
            if adapter.OperStatus != IfOperStatusUp || adapter.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
                continue;
            }
            let mut routed = false;
            let mut gateway = adapter.FirstGatewayAddress;
            while let Some(entry) = gateway.as_ref() {
                routed |=
                    ipv4(entry.Address.lpSockaddr).is_some_and(|address| !address.is_unspecified());
                gateway = entry.Next;
            }
            let mut addresses = vec![];
            let mut unicast = adapter.FirstUnicastAddress;
            while let Some(entry) = unicast.as_ref() {
                addresses.extend(ipv4(entry.Address.lpSockaddr));
                unicast = entry.Next;
            }
            adapters.push(Adapter {
                name: adapter.FriendlyName.to_string().unwrap_or_default(),
                routed,
                addresses,
            });
        }
    }
    adapters
}

/// One socket per IPv4 address, the routed ones first.
pub fn devices() -> Vec<CaptureDevice> {
    let mut devices: Vec<_> = adapters()
        .into_iter()
        .flat_map(|adapter| {
            let flags = if adapter.routed {
                DEFAULT_ROUTE_FLAG
            } else {
                0
            };
            adapter.addresses.into_iter().map(move |address| {
                CaptureDevice::new(
                    format!("{RAW_SOCKET_PREFIX}{address}"),
                    format!("{} · {address} (Windows raw socket)", adapter.name),
                    flags,
                )
            })
        })
        .collect();
    devices.sort_by_key(|device| std::cmp::Reverse(device.flags & DEFAULT_ROUTE_FLAG));
    devices
}

/// `raw:all` ahead of the per-address sockets.
pub fn devices_with_all() -> Vec<CaptureDevice> {
    let mut devices = devices();
    devices.insert(
        0,
        CaptureDevice::new(ALL, "All adapters (Windows raw socket)", DEFAULT_ROUTE_FLAG),
    );
    devices
}

pub fn open(device: &str) -> Result<Box<dyn FrameSource>, CaptureError> {
    if device == ALL {
        Ok(Box::new(AllAdaptersCapture::open()?))
    } else {
        Ok(Box::new(RawSocketCapture::open(device)?))
    }
}

fn raw(socket: &Socket) -> SOCKET {
    SOCKET(socket.as_raw_socket() as usize)
}

fn last_error() -> io::Error {
    // SAFETY: plain getter.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0)
}

/// Sockets ready to read within `timeout_ms`; every `revents` is updated.
fn poll(sockets: &mut [WSAPOLLFD], timeout_ms: i32) -> io::Result<bool> {
    // SAFETY: `sockets` is a live slice of the length given.
    match unsafe { WSAPoll(sockets.as_mut_ptr(), sockets.len() as u32, timeout_ms) } {
        result if result < 0 => Err(last_error()),
        result => Ok(result > 0),
    }
}

pub struct RawSocketCapture {
    socket: Socket,
    buffer: Box<[u8]>,
    received: u32,
}

impl RawSocketCapture {
    pub fn open(device: &str) -> Result<Self, CaptureError> {
        let invalid =
            || CaptureError::InvalidDevice("Select an available capture interface.".into());
        let address: Ipv4Addr = device
            .strip_prefix(RAW_SOCKET_PREFIX)
            .and_then(|value| value.parse().ok())
            .ok_or_else(invalid)?;
        if !devices().iter().any(|item| item.name == device) {
            return Err(invalid());
        }
        let failed = |error: io::Error| {
            if error.raw_os_error() == Some(WSAEACCES.0) {
                CaptureError::Io(
                    "Packet capture permission denied: run Albion Meter as administrator.".into(),
                )
            } else {
                CaptureError::os(
                    &format!("Packet capture could not start on {address}"),
                    &error,
                )
            }
        };
        let socket =
            Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::from(0))).map_err(failed)?;
        socket
            .bind(&SocketAddrV4::new(address, 0).into())
            .map_err(failed)?;
        socket.set_header_included_v4(true).map_err(failed)?;
        socket
            .set_recv_buffer_size(16 * 1024 * 1024)
            .map_err(failed)?;
        let on = 1u32;
        let mut returned = 0u32;
        // SAFETY: `on` is the 4-byte input SIO_RCVALL takes; no output buffer, no overlapped I/O.
        let result = unsafe {
            WSAIoctl(
                raw(&socket),
                SIO_RCVALL,
                Some((&raw const on).cast::<c_void>()),
                4,
                None,
                0,
                &mut returned,
                None,
                None,
            )
        };
        if result != 0 {
            return Err(failed(last_error()));
        }
        socket.set_nonblocking(true).map_err(failed)?;
        Ok(Self {
            socket,
            buffer: vec![0; 65536].into_boxed_slice(),
            received: 0,
        })
    }

    /// Takes the next game packet; with `wait`, first waits up to 50 ms for the socket to have one.
    fn receive(&mut self, frame: &mut CaptureFrame, mut wait: bool) -> Result<Read, CaptureError> {
        for _ in 0..256 {
            match (&self.socket).read(&mut self.buffer) {
                Ok(0) => return Ok(Read::Idle),
                Ok(length) => {
                    let packet = &self.buffer[..length];
                    if !is_albion_packet(packet) {
                        continue;
                    }
                    self.received = self.received.wrapping_add(1);
                    frame.timestamp_us = now_us();
                    frame.original_length = length as u32;
                    frame.link_type = LINK_TYPE_RAW;
                    frame.data.clear();
                    frame.data.extend_from_slice(packet);
                    return Ok(Read::Frame);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut descriptor = [WSAPOLLFD {
                        fd: raw(&self.socket),
                        events: POLLRDNORM,
                        revents: Default::default(),
                    }];
                    if !std::mem::take(&mut wait)
                        || !poll(&mut descriptor, 50)
                            .map_err(|error| CaptureError::Io(os_text(&error)))?
                    {
                        return Ok(Read::Idle);
                    }
                }
                Err(error) if error.raw_os_error() == Some(WSAEMSGSIZE.0) => {}
                Err(error) => return Err(CaptureError::Io(os_text(&error))),
            }
        }
        Ok(Read::Idle)
    }
}

impl FrameSource for RawSocketCapture {
    fn link_type(&self) -> i32 {
        LINK_TYPE_RAW
    }

    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
        self.receive(frame, true)
    }

    fn statistics(&mut self) -> Option<AdapterStatistics> {
        Some(AdapterStatistics {
            received: self.received,
            dropped: 0,
            interface_dropped: 0,
        })
    }

    /// SIO_RCVALL has no filter: every packet of the adapter is checked here.
    fn kernel_filtered(&self) -> bool {
        false
    }
}

/// A raw socket on every IPv4 adapter at once, rescanned every few seconds: ExitLag, NoPing and VPNs carry the game on
/// an adapter of their own, often one that appears only once they are turned on. A packet seen on two adapters is
/// dropped later as a Photon duplicate.
pub struct AllAdaptersCapture {
    sockets: BTreeMap<String, RawSocketCapture>,
    scanned: Instant,
    polls: Vec<WSAPOLLFD>,
}

impl AllAdaptersCapture {
    pub fn open() -> Result<Self, CaptureError> {
        let mut capture = Self {
            sockets: BTreeMap::new(),
            scanned: Instant::now(),
            polls: vec![],
        };
        let failure = capture.scan();
        match failure {
            Some(failure) if capture.sockets.is_empty() => Err(failure),
            _ if capture.sockets.is_empty() => Err(CaptureError::Io(
                "No network adapter with an IPv4 address is up.".into(),
            )),
            _ => Ok(capture),
        }
    }

    fn scan(&mut self) -> Option<CaptureError> {
        self.scanned = Instant::now();
        let present: Vec<String> = devices().into_iter().map(|device| device.name).collect();
        self.sockets.retain(|name, _| present.contains(name));
        let mut failure = None;
        for name in present {
            if self.sockets.contains_key(&name) {
                continue;
            }
            match RawSocketCapture::open(&name) {
                Ok(capture) => _ = self.sockets.insert(name, capture),
                Err(error) => {
                    _ = failure.get_or_insert(CaptureError::Io(error.message().to_owned()))
                }
            }
        }
        failure
    }
}

impl FrameSource for AllAdaptersCapture {
    fn link_type(&self) -> i32 {
        LINK_TYPE_RAW
    }

    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
        if self.scanned.elapsed() > RESCAN_EVERY {
            self.scan();
        }
        if self.sockets.is_empty() {
            std::thread::sleep(Duration::from_millis(50));
            return Ok(Read::Idle);
        }
        self.polls.clear();
        self.polls
            .extend(self.sockets.values().map(|capture| WSAPOLLFD {
                fd: raw(&capture.socket),
                events: POLLRDNORM,
                revents: Default::default(),
            }));
        match poll(&mut self.polls, 50) {
            Ok(true) => {}
            Ok(false) => return Ok(Read::Idle),
            Err(_) => {
                self.scan();
                return Ok(Read::Idle);
            }
        }
        for (capture, polled) in self.sockets.values_mut().zip(&self.polls) {
            if polled.revents.0 != 0 && capture.receive(frame, false)? == Read::Frame {
                return Ok(Read::Frame);
            }
        }
        Ok(Read::Idle)
    }

    fn statistics(&mut self) -> Option<AdapterStatistics> {
        let received = self
            .sockets
            .values()
            .fold(0u32, |sum, capture| sum.wrapping_add(capture.received));
        Some(AdapterStatistics {
            received,
            dropped: 0,
            interface_dropped: 0,
        })
    }

    fn kernel_filtered(&self) -> bool {
        false
    }
}

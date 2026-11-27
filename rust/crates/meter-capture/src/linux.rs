//! `LinuxSocketCapture`: an AF_PACKET cooked socket with the game filter in the kernel.

use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::source::{now_us, os_text};
use crate::{
    AdapterStatistics, CaptureDevice, CaptureError, CaptureFrame, DEFAULT_ROUTE_FLAG, FrameSource,
    LINK_TYPE_RAW, LINUX_FILTER, RAW_SOCKET_PREFIX, Read, is_albion_packet,
};

const ETH_P_ALL: u16 = 3;

pub struct LinuxSocketCapture {
    socket: OwnedFd,
    buffer: Box<[u8]>,
    received: u32,
    dropped: u32,
    kernel_filtered: bool,
}

/// `raw:any` first, then every interface that is up and not loopback, the routed one (IPv4 gateway) ahead.
pub fn devices() -> Vec<CaptureDevice> {
    let routes = fs::read_to_string("/proc/net/route").unwrap_or_default();
    let mut devices: Vec<_> = interfaces()
        .into_iter()
        .filter(|name| is_up(name) && !is_loopback(name))
        .map(|name| {
            let flags = if routed(&routes, &name) {
                DEFAULT_ROUTE_FLAG
            } else {
                0
            };
            CaptureDevice::new(
                format!("{RAW_SOCKET_PREFIX}{name}"),
                format!("{name} (Linux socket)"),
                flags,
            )
        })
        .collect();
    devices.sort_by_key(|device| std::cmp::Reverse(device.flags));
    devices.insert(
        0,
        CaptureDevice::new(
            "raw:any",
            "All interfaces (Linux socket)",
            DEFAULT_ROUTE_FLAG,
        ),
    );
    devices
}

fn interfaces() -> Vec<String> {
    // SAFETY: if_nameindex returns an array ended by a zero entry, freed below.
    unsafe {
        let list = libc::if_nameindex();
        if list.is_null() {
            return vec![];
        }
        let mut names = vec![];
        let mut entry = list;
        while (*entry).if_index != 0 && !(*entry).if_name.is_null() {
            names.push(
                CStr::from_ptr((*entry).if_name)
                    .to_string_lossy()
                    .into_owned(),
            );
            entry = entry.add(1);
        }
        libc::if_freenameindex(list);
        names
    }
}

/// .NET's `OperationalStatus.Up`: the kernel's operstate says "up" (not "unknown" or "dormant").
fn is_up(name: &str) -> bool {
    fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
        .is_ok_and(|state| state.trim() == "up")
}

fn is_loopback(name: &str) -> bool {
    fs::read_to_string(format!("/sys/class/net/{name}/type")).is_ok_and(|kind| kind.trim() == "772")
}

/// The main card: the one with an IPv4 gateway, where the internet traffic goes.
fn routed(routes: &str, name: &str) -> bool {
    routes.lines().skip(1).any(|line| {
        let mut columns = line.split_whitespace();
        columns.next() == Some(name)
            && columns.nth(1).is_some_and(|gateway| {
                u32::from_str_radix(gateway, 16).is_ok_and(|address| address != 0)
            })
    })
}

pub fn open(device: &str) -> Result<LinuxSocketCapture, CaptureError> {
    let name = device.strip_prefix(RAW_SOCKET_PREFIX).unwrap_or(device);
    if !devices().iter().any(|value| value.name == device) {
        return Err(CaptureError::InvalidDevice(
            "Select an available socket interface.".into(),
        ));
    }
    let index = if name == "any" {
        0
    } else {
        let c_name = CString::new(name).map_err(|_| {
            CaptureError::InvalidDevice("Select an available socket interface.".into())
        })?;
        // SAFETY: a valid NUL-terminated string.
        match unsafe { libc::if_nametoindex(c_name.as_ptr()) } {
            0 => {
                return Err(CaptureError::os(
                    "Interface lookup failed",
                    &io::Error::last_os_error(),
                ));
            }
            index => index,
        }
    };
    // SAFETY: plain syscall; the descriptor is owned right away.
    let descriptor = unsafe {
        libc::socket(
            libc::AF_PACKET,
            libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            i32::from(ETH_P_ALL.to_be()),
        )
    };
    if descriptor < 0 {
        return Err(CaptureError::os(
            "Socket capture permission denied; CAP_NET_RAW may be required",
            &io::Error::last_os_error(),
        ));
    }
    // SAFETY: `descriptor` is a fresh socket nobody else owns.
    let socket = unsafe { OwnedFd::from_raw_fd(descriptor) };
    // SAFETY: sockaddr_ll is plain data; zeroed is a valid start.
    let mut address: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
    address.sll_family = libc::AF_PACKET as u16;
    address.sll_protocol = ETH_P_ALL.to_be();
    address.sll_ifindex = index as i32;
    // SAFETY: `address` is a sockaddr_ll of the size given.
    let bound = unsafe {
        libc::bind(
            descriptor,
            (&raw const address).cast(),
            size_of::<libc::sockaddr_ll>() as libc::socklen_t,
        )
    };
    if bound != 0 {
        return Err(CaptureError::os(
            "Socket interface binding failed",
            &io::Error::last_os_error(),
        ));
    }
    // The socket sees every packet of the interface: without a kernel filter, a download or a stream fills the
    // queue and the kernel drops game packets with the rest. The filter keeps only what is_albion_packet accepts.
    let kernel_filtered = attach_filter(descriptor);
    // SO_RCVBUFFORCE needs CAP_NET_ADMIN; without it SO_RCVBUF asks for the same and the kernel caps it at rmem_max.
    let receive_buffer: libc::c_int = 8 * 1024 * 1024;
    if set_option(
        descriptor,
        libc::SOL_SOCKET,
        libc::SO_RCVBUFFORCE,
        &receive_buffer,
    ) != 0
    {
        set_option(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            &receive_buffer,
        );
    }
    Ok(LinuxSocketCapture {
        socket,
        buffer: vec![0; 65536].into_boxed_slice(),
        received: 0,
        dropped: 0,
        kernel_filtered,
    })
}

fn set_option<T>(descriptor: i32, level: i32, option: i32, value: &T) -> i32 {
    // SAFETY: `value` is a live T of the size given.
    unsafe {
        libc::setsockopt(
            descriptor,
            level,
            option,
            (value as *const T).cast(),
            size_of::<T>() as libc::socklen_t,
        )
    }
}

/// A kernel that refuses the filter still captures; the packets are then checked one by one as before.
fn attach_filter(descriptor: i32) -> bool {
    let mut instructions =
        LINUX_FILTER.map(|(code, jt, jf, k)| libc::sock_filter { code, jt, jf, k });
    let program = libc::sock_fprog {
        len: instructions.len() as u16,
        filter: instructions.as_mut_ptr(),
    };
    set_option(
        descriptor,
        libc::SOL_SOCKET,
        libc::SO_ATTACH_FILTER,
        &program,
    ) == 0
}

impl FrameSource for LinuxSocketCapture {
    fn link_type(&self) -> i32 {
        LINK_TYPE_RAW
    }

    fn read(&mut self, frame: &mut CaptureFrame) -> Result<Read, CaptureError> {
        let descriptor = self.socket.as_raw_fd();
        let mut poll = libc::pollfd {
            fd: descriptor,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, 50) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                return Ok(Read::Idle);
            }
            return Err(CaptureError::os("Socket polling failed", &error));
        }
        if ready == 0 {
            return Ok(Read::Idle);
        }
        if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(CaptureError::Io(
                "The capture socket became unavailable.".into(),
            ));
        }
        for _ in 0..256 {
            // SAFETY: `buffer` is writable for its whole length.
            let length = unsafe {
                libc::recv(
                    descriptor,
                    self.buffer.as_mut_ptr().cast(),
                    self.buffer.len(),
                    0,
                )
            };
            if length < 0 {
                let error = io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::EINTR)) {
                    return Ok(Read::Idle);
                }
                return Err(CaptureError::Io(format!(
                    "Socket receive failed: {}",
                    os_text(&error)
                )));
            }
            if length == 0 {
                return Ok(Read::Idle);
            }
            let packet = &self.buffer[..length as usize];
            if !is_albion_packet(packet) {
                continue;
            }
            self.received = self.received.wrapping_add(1);
            frame.timestamp_us = now_us();
            frame.original_length = packet.len() as u32;
            frame.link_type = LINK_TYPE_RAW;
            frame.data.clear();
            frame.data.extend_from_slice(packet);
            return Ok(Read::Frame);
        }
        Ok(Read::Idle)
    }

    /// The kernel resets its counters on every read, so the drops add up here.
    fn statistics(&mut self) -> Option<AdapterStatistics> {
        let mut counters = libc::tpacket_stats {
            tp_packets: 0,
            tp_drops: 0,
        };
        let mut size = size_of::<libc::tpacket_stats>() as libc::socklen_t;
        // SAFETY: `counters` is writable for `size` bytes.
        let read = unsafe {
            libc::getsockopt(
                self.socket.as_raw_fd(),
                libc::SOL_PACKET,
                libc::PACKET_STATISTICS,
                (&raw mut counters).cast(),
                &mut size,
            )
        };
        if read == 0 {
            self.dropped = self.dropped.wrapping_add(counters.tp_drops);
        }
        Some(AdapterStatistics {
            received: self.received,
            dropped: self.dropped,
            interface_dropped: 0,
        })
    }

    fn kernel_filtered(&self) -> bool {
        self.kernel_filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table_marks_the_gateway_interface() {
        let routes = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                      wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                      wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n\
                      docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";
        assert!(routed(routes, "wlan0"));
        assert!(!routed(routes, "docker0"));
        assert!(!routed(routes, "wlan"));
    }

    #[test]
    fn devices_start_with_raw_any_and_skip_loopback() {
        let devices = devices();
        assert_eq!(
            devices[0],
            CaptureDevice::new(
                "raw:any",
                "All interfaces (Linux socket)",
                DEFAULT_ROUTE_FLAG
            )
        );
        assert!(
            devices
                .iter()
                .all(|device| device.name != "raw:lo" && device.name.starts_with("raw:"))
        );
        assert!(
            devices[1..]
                .windows(2)
                .all(|pair| pair[0].flags >= pair[1].flags)
        );
    }

    #[test]
    fn unknown_devices_are_refused() {
        assert_eq!(
            open("raw:not-an-interface").err(),
            Some(CaptureError::InvalidDevice(
                "Select an available socket interface.".into()
            ))
        );
    }

    /// Needs CAP_NET_RAW: `sudo setcap cap_net_raw+ep <test binary>` or run as root.
    #[test]
    #[ignore]
    fn live_socket_opens_filtered_and_polls() {
        let mut capture = open("raw:any").unwrap();
        assert!(capture.kernel_filtered());
        let mut frame = CaptureFrame::default();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(2) {
            if capture.read(&mut frame).unwrap() == Read::Frame {
                assert!(is_albion_packet(&frame.data));
            }
        }
        assert!(capture.statistics().is_some());
    }

    #[test]
    fn without_cap_net_raw_the_error_says_so() {
        // SAFETY: plain getter.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        match open("raw:any") {
            Err(error) => {
                assert!(
                    error.message().starts_with(
                        "Socket capture permission denied; CAP_NET_RAW may be required: "
                    ),
                    "{error}"
                );
                assert!(error.is_permission());
            }
            // The test binary was given CAP_NET_RAW.
            Ok(capture) => assert!(capture.kernel_filtered()),
        }
    }
}

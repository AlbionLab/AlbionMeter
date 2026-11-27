# meter-capture port report

## C# -> Rust
| C# | Rust |
|---|---|
| PacketCapture.IsAlbionPacket, NativePcap.DefaultFilter, LinuxSocketCapture.Filter | src/filter.rs (is_albion_packet, DEFAULT_FILTER, LINUX_FILTER) |
| ICaptureSource, AdapterStatistics, CaptureFrame/CaptureDevice, exception messages | src/source.rs (FrameSource, Read, AdapterStatistics, CaptureError incl. is_permission() = CaptureService's permission check), src/lib.rs |
| PacketCapture.Devices/DefaultDevice/Open/Library, CaptureEndpoints /devices + /start checks | src/devices.rs (devices, default_device, open, library, device_list -> DeviceList serde camelCase, validate_start) |
| LinuxSocketCapture.cs | src/linux.rs (AF_PACKET SOCK_DGRAM, classic BPF via SO_ATTACH_FILTER, kernel_filtered flag, SO_RCVBUFFORCE->SO_RCVBUF, PACKET_STATISTICS drops, "Socket capture permission denied; CAP_NET_RAW may be required: <strerror>") |
| NativePcap.cs (live part) | src/native_pcap.rs (pcap crate: snaplen 262144, no promisc, 100 ms timeout, immediate mode, DEFAULT_FILTER optimized, stats received/dropped/ifdropped, header validation, pcap_lib_version) |
| RawSocketCapture + AllAdaptersCapture (PacketCapture.cs, Windows) | src/windows/raw.rs (socket2 + windows crate: SIO_RCVALL, GetAdaptersAddresses, WSAPoll, rescan every 5 s) |
| NativePcap resolver (System32\Npcap\wpcap.dll + pcap_init UTF-8) | src/windows/npcap.rs |
| PcapWriter.cs | src/writer.rs (PcapWriter<W: Write>, create_new, same header/validation/BytesWritten) |
| CaptureStorage.cs | src/storage.rs (remove_legacy_recordings) — note: CaptureStorage only deletes legacy GUID .pcap/.json + record-controls.jsonl; it has no writer side, the writer is PcapWriter |
| WindowsElevation.cs | src/elevation.rs (command_line quoting, cross-platform + tested; is_administrator / restart_elevated cfg(windows)) |
| WindowsFirewall.cs | src/windows/firewall.rs (allow_capture -> FirewallRule with the C# console messages) |
| (new) capture thread | src/thread.rs CaptureThread: bounded crossbeam channel, drops+counts when full (never grows), buffer recycling via recycler(), stats refreshed every 200 ms into atomics, cancel flag (<=100 ms), ExitReason Stopped/SourceEnded/Failed |

## Not ported / differences
- PcapFile reader (pcap/pcapng) is meter-core's per the README; not here.
- C# pcap used non-blocking mode (busy loop); Rust uses blocking + 100 ms timeout + immediate mode (idle CPU ~0).
- Windows raw socket: socket() failing with WSAEACCES now also yields the "permission denied: run as administrator" message (C# threw the raw SocketException there). Per-read loop capped at 256 packets so cancel stays prompt under heavy non-game traffic.
- pcap frames carry the DLT from pcap_datalink (e.g. 12 for raw), exactly as C# did.
- Linux interface order: if_nameindex order (C# used getifaddrs order); up = operstate "up", loopback = type 772, routed = /proc/net/route non-zero gateway, as .NET does.

## Cross-crate requests
- meter-app (Windows): link with `/DELAYLOAD:wpcap.dll` + `delayimp.lib` (MSVC; e.g. `cargo:rustc-link-arg-bins` in meter-app build.rs) and Npcap SDK's wpcap.lib (LIBPCAP_LIBDIR) so the exe starts without Npcap; meter_capture calls windows::npcap::load() before any pcap call. For the gnu target, delay-load needs a dlltool --output-delaylib import lib, or build with `--no-default-features` (raw sockets only).
- meter-app: call windows::firewall::allow_capture() before opening a raw: device, elevation::is_administrator()/restart_elevated() at startup, storage::remove_legacy_recordings() when debug is off (same as CaptureService).
- Linux: the binary links libpcap.so.1 at load time (C# loaded it lazily); packagers must depend on libpcap or build without the `pcap` feature.
- Root Cargo.lock changed (crossbeam-channel, pcap, socket2, windows, libc added). Workspace Cargo.toml untouched; deps declared in the crate.

## Verification (from rust/, CARGO_TARGET_DIR=target/rust-capture)
- cargo clippy -p meter-capture --all-targets -- -D warnings -> exit 0
- cargo test -p meter-capture -> exit 0: 28 passed, 0 failed, 1 ignored (live AF_PACKET smoke test, needs CAP_NET_RAW; no sudo here)
- cargo clippy -p meter-capture --all-targets --target x86_64-pc-windows-msvc -- -D warnings -> exit 0 (type-check only, no link: no Npcap SDK/MSVC linker)
- cargo clippy -p meter-capture --all-targets --target x86_64-pc-windows-gnu -- -D warnings -> exit 0 (type-check only, no mingw linker)
- --no-default-features clippy on linux + windows-msvc -> exit 0
Tests cover: IsAlbionPacket cases + malformed headers; LINUX_FILTER run in a cBPF interpreter agrees with IsAlbionPacket; DEFAULT_FILTER compiled by libpcap (pcap_offline_filter) agrees; writer round-trip (bytes + read back by libpcap), refusal cases, create_new; channel back-pressure (100 frames, cap 4 -> 4 sent, 96 dropped), lossless when consumer keeps up, buffer recycling, cancel stops thread < 200 ms, source error ends thread; device list JSON shape, default device per platform, start validation; Linux devices on this machine (raw:any first), CAP_NET_RAW error text; legacy recording cleanup; elevation quoting.

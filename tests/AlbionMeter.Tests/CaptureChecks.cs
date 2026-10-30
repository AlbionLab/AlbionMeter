using Microsoft.Extensions.Configuration;
using System.Buffers.Binary;
using AlbionMeter.Core.Capture;

internal static class CaptureChecks
{
    public static void Run(Action<string, Action> test, Action<bool> assert, bool native)
    {
        test("Capture recovery replays the last day of local recordings and excludes imported replays", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), "albion-recovery-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(directory);
            try
            {
                for (var index = 0; index < 6; index++)
                {
                    var path = Path.Combine(directory, $"{index}.pcap");
                    File.WriteAllBytes(path, []);
                    File.SetLastWriteTimeUtc(path, index == 0 ? DateTime.UtcNow.AddHours(-25) : DateTime.UtcNow.AddMinutes(-60 + index));
                    File.WriteAllText(path + ".json", System.Text.Json.JsonSerializer.Serialize(new { session = new { source = index == 5 ? "replay" : "live-capture" } }));
                }
                var files = AlbionMeter.Server.CaptureHistory.LocalFiles(directory);
                assert(files.Length == 4 && Path.GetFileName(files[0]) == "1.pcap" && Path.GetFileName(files[^1]) == "4.pcap");
            }
            finally { Directory.Delete(directory, true); }
        });
        test("The Linux kernel filter keeps exactly the packets the meter reads", () =>
        {
            if (!OperatingSystem.IsLinux()) return;
            var random = new Random(7);
            var packets = new List<byte[]>();
            foreach (var protocol in new byte[] { 17, 6, 1 })
                foreach (var header in new[] { 20, 24, 60 })
                    foreach (var (source, destination) in new[] { (5056, 40000), (40000, 5055), (5058, 1), (443, 50000), (5057, 5054), (53, 53) })
                        foreach (var fragment in new ushort[] { 0, 0x4000, 0x2000, 0x0010 })
                        {
                            var packet = new byte[header + 16];
                            packet[0] = (byte)(0x40 | header / 4); packet[9] = protocol;
                            BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(6), fragment);
                            BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(header), (ushort)source);
                            BinaryPrimitives.WriteUInt16BigEndian(packet.AsSpan(header + 2), (ushort)destination);
                            packets.Add(packet);
                        }
            packets.Add([0x60, 0, 0, 0, 0, 8, 17, 64, .. new byte[40]]);
            packets.Add([0x00, 1, 8, 0, 6, 4, 0, 1, .. new byte[20]]);
            for (var index = 0; index < 2000; index++)
            {
                // Well formed as the kernel delivers them: never shorter than their own IP header.
                var header = random.Next(5, 16);
                var packet = new byte[header * 4 + random.Next(0, 40)];
                random.NextBytes(packet);
                packet[0] = (byte)(0x40 | header);
                if (index % 2 == 0) packet[9] = 17;
                packets.Add(packet);
            }
            foreach (var packet in packets)
                assert(Accepts(LinuxSocketCapture.Filter, packet) == PacketCapture.IsAlbionPacket(packet));
        });
        test("IPv4 UDP payloads preserve endpoints and timestamps", () =>
        {
            var frame = Frame(Ip(Udp([1, 2, 3])));
            var result = new NetworkDecoder().Decode(frame);
            assert(result.Status == "udp" && result.Datagram!.SourcePort == 5056 && result.Datagram.TimestampMicroseconds == 1_000_000);
            assert(result.Datagram!.Payload.SequenceEqual(new byte[] { 1, 2, 3 }));
        });
        test("Linux any-interface frames decode with cooked headers", () =>
        {
            var packet = Ip(Udp([7, 8, 9]));
            var sll = new byte[16 + packet.Length]; Put16(sll, 14, 0x0800); packet.CopyTo(sll, 16);
            var sll2 = new byte[20 + packet.Length]; Put16(sll2, 0, 0x0800); packet.CopyTo(sll2, 20);
            foreach (var (linkType, bytes) in new[] { (113, sll), (276, sll2) })
            {
                var result = new NetworkDecoder().Decode(new(1_000_000, (uint)bytes.Length, linkType, bytes));
                assert(result.Status == "udp" && result.Datagram!.SourcePort == 5056 && result.Datagram.Payload.SequenceEqual(new byte[] { 7, 8, 9 }));
            }
        });
        test("Truncated and malformed frames are counted separately", () =>
        {
            var decoder = new NetworkDecoder(); var frame = Frame(Ip(Udp([1])));
            assert(decoder.Decode(frame with { OriginalLength = 100 }).Status == "truncated");
            frame.Data[2] = 255;
            assert(decoder.Decode(frame).Status == "malformed");
            assert(decoder.Decode(Frame([])).Status == "malformed");
            assert(decoder.Decode(Frame([0]) with { LinkType = 9999 }).Status == "unsupportedLink");
        });
        test("Ethernet VLAN headers resolve the underlying IPv4 packet", () =>
        {
            var ip = Ip(Udp([42])); var ethernet = new byte[18 + ip.Length];
            Put16(ethernet, 12, 0x8100); Put16(ethernet, 16, 0x0800); ip.CopyTo(ethernet, 18);
            assert(new NetworkDecoder().Decode(Frame(ethernet) with { LinkType = 1 }).Datagram!.Payload[0] == 42);
        });
        test("IPv6 UDP and extension header lengths are validated", () =>
        {
            var udp = Udp([42]); var ip = new byte[48 + udp.Length]; ip[0] = 0x60; ip[6] = 0;
            Put16(ip, 4, (ushort)(ip.Length - 40)); ip[40] = 17; udp.CopyTo(ip, 48);
            assert(new NetworkDecoder().Decode(Frame(ip)).Datagram!.Payload[0] == 42);
            ip[41] = 200;
            assert(new NetworkDecoder().Decode(Frame(ip)).Status == "malformed");
        });
        test("IPv4 fragments reassemble out of order without double counting", () =>
        {
            var udp = Udp([1, 2, 3, 4]); var decoder = new NetworkDecoder();
            var tail = Frame(Ip(udp[8..], 1)); var head = Frame(Ip(udp[..8], 0x2000));
            assert(decoder.Decode(tail).Status == "fragmentPending");
            assert(decoder.Decode(tail).Status == "fragmentPending");
            assert(decoder.Decode(head).Datagram!.Payload.SequenceEqual(new byte[] { 1, 2, 3, 4 }));
            assert(decoder.PendingFragments == 0);
        });
        test("Conflicting IPv4 overlaps are rejected and incomplete assemblies expire", () =>
        {
            var decoder = new NetworkDecoder(); var head = Frame(Ip(Udp([1, 2, 3, 4])[..8], 0x2000));
            decoder.Decode(head); head.Data[^1] ^= 1;
            assert(decoder.Decode(head).Status == "fragmentConflict");
            decoder.Decode(head);
            decoder.Decode(Frame(Ip(Udp([9]))) with { TimestampMicroseconds = 17_000_001 });
            assert(decoder.ExpiredFragments == 1 && decoder.PendingFragments == 0);
        });
        test("IP fragment storage is bounded under many incomplete datagrams", () =>
        {
            var decoder = new NetworkDecoder();
            for (ushort id = 0; id < 200; id++) decoder.Decode(Frame(Ip(new byte[8], 0x2000, id)));
            assert(decoder.PendingFragments == 128 && decoder.EvictedFragments == 72);
        });
        test("Retransmissions are deduplicated by connection channel and sequence", () =>
        {
            var decoder = new PhotonTransport(); var udp = Datagram(Photon([0xf3, 4, 1], 10));
            assert(decoder.Decode(udp).Messages.Length == 1);
            assert(decoder.Decode(udp).Messages.Length == 0);
            assert(decoder.Duplicates == 1);
            assert(decoder.Decode(Datagram(Photon([0xf3, 4, 1], 11))).Messages.Length == 1);
        });
        test("A route changer moving the connection to other addresses keeps it one connection", () =>
        {
            // ExitLag, NoPing and similar tools switch routes (other IP or port) or send copies over several at once.
            var decoder = new PhotonTransport(); var udp = Datagram(Photon([0xf3, 4, 1], 10));
            var first = decoder.Decode(udp).Messages.Single();
            assert(decoder.Decode(udp with { Source = "198.51.100.7", DestinationPort = 40000 }).Messages.Length == 0);
            var moved = decoder.Decode(Datagram(Photon([0xf3, 4, 1], 12)) with { Source = "198.51.100.7", DestinationPort = 40000 }).Messages.Single();
            assert(moved.Connection == first.Connection);
            var sent = decoder.Decode(Datagram(Photon([0xf3, 4, 1], 12)) with { Source = "192.0.2.2", SourcePort = 12345, Destination = "192.0.2.1", DestinationPort = 5056 }).Messages.Single();
            assert(sent.Connection != first.Connection);
        });
        test("Malformed trailing data cannot consume a valid command identity", () =>
        {
            var decoder = new PhotonTransport(); var payload = Photon([0xf3, 4, 1, 0], 5);
            assert(decoder.Decode(Datagram([.. payload, 1])).Status == "malformedPhoton");
            assert(decoder.Decode(Datagram(payload)).Messages.Length == 1);
        });
        test("Photon challenges isolate new connections sharing endpoints", () =>
        {
            var decoder = new PhotonTransport(); var bytes = Photon([0xf3, 4, 1], 1);
            decoder.Decode(Datagram(bytes)); bytes[11] = 2;
            assert(decoder.Decode(Datagram(bytes)).Messages.Length == 1);
        });
        test("Photon fragments reassemble by connection and reject conflicting data", () =>
        {
            var decoder = new PhotonTransport();
            assert(decoder.Decode(Datagram(Fragment([4, 5], 1, 3, 5))).Messages.Length == 0);
            var result = decoder.Decode(Datagram(Fragment([1, 2, 3], 0, 0, 5)));
            assert(result.Messages.Single().Payload.SequenceEqual(new byte[] { 1, 2, 3, 4, 5 }));
            assert(decoder.Decode(Datagram(Fragment([1, 2, 3], 0, 0, 5))).Messages.Length == 0);
            decoder = new(); decoder.Decode(Datagram(Fragment([1, 2, 3], 0, 0, 5)));
            assert(decoder.Decode(Datagram(Fragment([9, 2, 3], 0, 0, 5))).Status == "photonFragmentConflict");
        });
        test("Invalid Photon lengths and oversized fragment allocation are rejected", () =>
        {
            var decoder = new PhotonTransport(); var bytes = Photon([1], 1); Put32(bytes, 16, uint.MaxValue);
            assert(decoder.Decode(Datagram(bytes)).Status == "malformedPhoton");
            assert(decoder.Decode(Datagram(Fragment([1], 0, 0, 2_000_000))).Status == "malformedPhotonFragment");
            assert(decoder.PendingAssemblies == 0);
        });
        test("Replay diagnostics are deterministic and never claim combat decoding", () =>
        {
            var frames = new[] { Frame(Ip(Udp(Photon([0xf3, 4, 1], 1)))), Frame([1, 2]) };
            var first = new CaptureAnalysis(); var second = new CaptureAnalysis();
            foreach (var frame in frames) { first.Add(frame); second.Add(frame); }
            var a = first.Snapshot(); var b = second.Snapshot();
            assert(a.Frames == 2 && a.Counters["transportMessages"] == 1 && a.Counters["malformed"] == 1);
            assert(!a.CombatDecoded && a.Counters.SequenceEqual(b.Counters));
        });
        test("Random short packet inputs never escape decoder validation", () =>
        {
            var random = new Random(19); var network = new NetworkDecoder(); var photon = new PhotonTransport();
            for (var i = 0; i < 10000; i++)
            {
                var bytes = new byte[random.Next(0, 300)]; random.NextBytes(bytes);
                network.Decode(Frame(bytes)); photon.Decode(Datagram(bytes));
            }
        });
        test("The built-in capture file reader reads PCAP and PCAPNG frames exactly", () =>
        {
            var path = Path.Combine(Path.GetTempPath(), $"albion-reader-{Guid.NewGuid():N}.pcap");
            try
            {
                var first = Frame(Ip(Udp(Photon([0xf3, 4, 1], 1)))); var second = first with { TimestampMicroseconds = 2_500_001 };
                using (var writer = new PcapWriter(path, 101)) { writer.Write(first); writer.Write(second); }
                using (var reader = PcapFile.Open(path))
                {
                    assert(reader.LinkType == 101); assert(reader.Read(out var one)); assert(reader.Read(out var two)); assert(!reader.Read(out _));
                    assert(one!.Data.SequenceEqual(first.Data) && one.TimestampMicroseconds == 1_000_000 && two!.TimestampMicroseconds == 2_500_001 && two.OriginalLength == second.OriginalLength);
                }
                var packet = first.Data; var padded = (packet.Length + 3) / 4 * 4;
                using var stream = new MemoryStream(); using var output = new BinaryWriter(stream);
                output.Write(0x0a0d0d0au); output.Write(28u); output.Write(0x1a2b3c4du); output.Write((ushort)1); output.Write((ushort)0); output.Write(-1L); output.Write(28u);
                output.Write(1u); output.Write(28u); output.Write((ushort)101); output.Write((ushort)0); output.Write(262144u); output.Write((ushort)9); output.Write((ushort)1); output.Write(new byte[] { 9, 0, 0, 0 }); output.Write(28u);
                var nanoseconds = 3_000_000_500_000L; var length = (uint)(32 + padded);
                output.Write(6u); output.Write(length); output.Write(0u); output.Write((uint)(nanoseconds >> 32)); output.Write((uint)nanoseconds); output.Write((uint)packet.Length); output.Write((uint)packet.Length);
                output.Write(packet); output.Write(new byte[padded - packet.Length]); output.Write(length);
                File.WriteAllBytes(path, stream.ToArray());
                using (var reader = PcapFile.Open(path))
                {
                    assert(reader.LinkType == 101); assert(reader.Read(out var frame)); assert(!reader.Read(out _));
                    assert(frame!.Data.SequenceEqual(packet) && frame.TimestampMicroseconds == 3_000_000_500 && frame.LinkType == 101);
                }
                File.WriteAllBytes(path, [1, 2, 3, 4, 5]); var rejected = false;
                try { using var reader = PcapFile.Open(path); } catch (IOException) { rejected = true; }
                assert(rejected);
            }
            finally { File.Delete(path); }
        });
        test("The raw socket keeps only Albion UDP and IP fragments, like the libpcap filter", () =>
        {
            assert(PacketCapture.IsAlbionPacket(Ip(Udp(Photon([0xf3, 4, 1], 1)))));
            var other = Ip(Udp([1, 2, 3])); Put16(other, 20, 443); Put16(other, 22, 50000);
            assert(!PacketCapture.IsAlbionPacket(other));
            assert(PacketCapture.IsAlbionPacket(Ip([1, 2, 3, 4, 5, 6, 7, 8], flags: 0x2000)));
            var tcp = Ip(Udp(Photon([0xf3, 4, 1], 1))); tcp[9] = 6;
            assert(!PacketCapture.IsAlbionPacket(tcp) && !PacketCapture.IsAlbionPacket([0x45, 0, 0]) && !PacketCapture.IsAlbionPacket(new byte[40]));
        });
        if (!native) return;
        test("Native libpcap reads a recorded file and preserves packet data", () =>
        {
            var path = Path.Combine(Path.GetTempPath(), $"albion-native-{Guid.NewGuid():N}.pcap");
            try
            {
                var frame = Frame(Ip(Udp(Photon([0xf3, 4, 1], 1))));
                using (var writer = new PcapWriter(path, 101)) writer.Write(frame);
                using var reader = NativePcap.OpenFile(path);
                assert(reader.Read(out var read) && read is not null);
                assert(read!.Data.SequenceEqual(frame.Data) && read.TimestampMicroseconds == frame.TimestampMicroseconds);
                assert(!reader.Read(out _)); assert(reader.Statistics() is null);
                assert(!string.IsNullOrWhiteSpace(NativePcap.Version));
            }
            finally { File.Delete(path); }
        });
        test("The built-in reader and libpcap read recorded captures identically", () =>
        {
            var directory = Path.Combine(AppContext.BaseDirectory, "../../../../../.dev/data/captures");
            var files = Directory.Exists(directory) ? Directory.EnumerateFiles(directory, "*.pcap").Select(file => new FileInfo(file)).Where(file => file.Length is > 1000 and < 30_000_000).OrderBy(file => file.Length).Take(3).ToArray() : [];
            foreach (var file in files)
            {
                using var native = NativePcap.OpenFile(file.FullName); using var managed = PcapFile.Open(file.FullName);
                while (true)
                {
                    bool nativeRead, managedRead; CaptureFrame? expected, actual;
                    try { nativeRead = native.Read(out expected); } catch (IOException) { break; }
                    try { managedRead = managed.Read(out actual); } catch (IOException) { assert(false); break; }
                    assert(nativeRead == managedRead);
                    if (!nativeRead) break;
                    assert(expected!.TimestampMicroseconds == actual!.TimestampMicroseconds && expected.LinkType == actual.LinkType && expected.Data.SequenceEqual(actual.Data));
                }
            }
        });
        test("Capture service replays packets without retaining raw files or reports", () =>
        {
            var directory = Path.Combine(Path.GetTempPath(), $"albion-service-{Guid.NewGuid():N}");
            Directory.CreateDirectory(directory);
            try
            {
                var input = Path.Combine(directory, "fixture.pcap");
                using (var writer = new PcapWriter(input, 101)) writer.Write(Frame(Ip(Udp(Photon([0xf3, 4, 1, 0], 1)))));
                var settings = new Microsoft.Extensions.Configuration.ConfigurationBuilder()
                    .AddInMemoryCollection(new Dictionary<string, string?> { ["Storage:Directory"] = directory }).Build();
                var service = new AlbionMeter.Server.CaptureService(settings);
                using var stream = File.OpenRead(input);
                var result = service.ReplayAsync(stream, CancellationToken.None).GetAwaiter().GetResult();
                assert(result.State == "completed" && result.Source == "replay" && result.Report!.Frames == 1);
                assert(result.Report!.Counters["decodedMessage"] == 1 && !result.Report.CombatDecoded);
                assert(result.File is null && !Directory.EnumerateFiles(Path.Combine(directory, "captures"), "*.pcap*").Any());
                assert(service.StopAsync().GetAwaiter().GetResult().State == "completed");
                using var invalid = new MemoryStream([1, 2, 3]);
                result = service.ReplayAsync(invalid, CancellationToken.None).GetAwaiter().GetResult();
                assert(result.State == "failed" && !string.IsNullOrWhiteSpace(result.Error));
            }
            finally { Directory.Delete(directory, true); }
        });
        test("Native libpcap reports invalid savefiles without producing frames", () =>
        {
            var path = Path.Combine(Path.GetTempPath(), $"albion-native-{Guid.NewGuid():N}.pcap");
            try
            {
                File.WriteAllBytes(path, [1, 2, 3]); var rejected = false;
                try { using var reader = NativePcap.OpenFile(path); } catch (IOException) { rejected = true; }
                assert(rejected);
            }
            finally { File.Delete(path); }
        });
    }
    private static CaptureFrame Frame(byte[] bytes) => new(1_000_000, (uint)bytes.Length, 101, bytes);
    private static UdpDatagram Datagram(byte[] bytes) => new(1_000_000, "192.0.2.1", "192.0.2.2", 5056, 12345, bytes);
    private static byte[] Udp(byte[] payload)
    {
        var bytes = new byte[8 + payload.Length]; Put16(bytes, 0, 5056); Put16(bytes, 2, 12345);
        Put16(bytes, 4, (ushort)bytes.Length); payload.CopyTo(bytes, 8); return bytes;
    }
    private static byte[] Ip(byte[] payload, ushort flags = 0, ushort id = 0)
    {
        var bytes = new byte[20 + payload.Length]; bytes[0] = 0x45; bytes[9] = 17;
        Put16(bytes, 2, (ushort)bytes.Length); Put16(bytes, 4, id); Put16(bytes, 6, flags);
        bytes[12] = 192; bytes[14] = 2; bytes[15] = 1; bytes[16] = 192; bytes[18] = 2; bytes[19] = 2;
        payload.CopyTo(bytes, 20); return bytes;
    }
    private static byte[] Photon(byte[] payload, uint sequence)
    {
        var bytes = new byte[24 + payload.Length]; bytes[3] = 1; bytes[11] = 1; bytes[12] = 6;
        Put32(bytes, 16, (uint)(12 + payload.Length)); Put32(bytes, 20, sequence); payload.CopyTo(bytes, 24); return bytes;
    }
    private static byte[] Fragment(byte[] payload, uint number, uint offset, uint total)
    {
        var bytes = new byte[44 + payload.Length]; bytes[3] = 1; bytes[11] = 1; bytes[12] = 8;
        Put32(bytes, 16, (uint)(32 + payload.Length)); Put32(bytes, 20, number + 10); Put32(bytes, 24, 10);
        Put32(bytes, 28, 2); Put32(bytes, 32, number); Put32(bytes, 36, total); Put32(bytes, 40, offset);
        payload.CopyTo(bytes, 44); return bytes;
    }
    private static void Put16(byte[] bytes, int offset, ushort value) => BinaryPrimitives.WriteUInt16BigEndian(bytes.AsSpan(offset), value);
    private static void Put32(byte[] bytes, int offset, uint value) => BinaryPrimitives.WriteUInt32BigEndian(bytes.AsSpan(offset), value);

    /// <summary>A classic BPF interpreter for the instructions the Linux filter uses; a read past the packet drops it, as in the kernel.</summary>
    static bool Accepts((ushort Code, byte True, byte False, uint Value)[] program, byte[] packet)
    {
        uint a = 0, x = 0;
        for (var pc = 0; pc < program.Length; pc++)
        {
            var (code, jumpTrue, jumpFalse, value) = program[pc];
            switch (code)
            {
                case 0x30: if (value >= packet.Length) return false; a = packet[value]; break;
                case 0x28: if (value + 2 > packet.Length) return false; a = BinaryPrimitives.ReadUInt16BigEndian(packet.AsSpan((int)value)); break;
                case 0x48: if (x + value + 2 > packet.Length) return false; a = BinaryPrimitives.ReadUInt16BigEndian(packet.AsSpan((int)(x + value))); break;
                case 0xb1: if (value >= packet.Length) return false; x = (uint)(packet[value] & 0x0f) * 4; break;
                case 0x54: a &= value; break;
                case 0x15: pc += a == value ? jumpTrue : jumpFalse; break;
                case 0x45: pc += (a & value) != 0 ? jumpTrue : jumpFalse; break;
                case 0x06: return value != 0;
                default: throw new InvalidOperationException($"Unknown BPF code {code:x}");
            }
        }
        return false;
    }
}

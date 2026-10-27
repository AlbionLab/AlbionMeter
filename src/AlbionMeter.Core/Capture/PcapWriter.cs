namespace AlbionMeter.Core.Capture;

public sealed class PcapWriter : IDisposable
{
    private readonly BinaryWriter _writer;
    private readonly int _linkType;
    public long BytesWritten { get; private set; } = 24;
    public PcapWriter(string path, int linkType)
    {
        _linkType = linkType;
        _writer = new BinaryWriter(new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read));
        _writer.Write(0xa1b2c3d4u);
        _writer.Write((ushort)2); _writer.Write((ushort)4);
        _writer.Write(0); _writer.Write(0); _writer.Write(262144); _writer.Write(linkType);
    }
    public void Write(CaptureFrame frame)
    {
        if (frame.LinkType != _linkType || frame.Data.Length > 262144 || frame.Data.Length > frame.OriginalLength
            || frame.TimestampMicroseconds < 0 || frame.TimestampMicroseconds / 1_000_000 > uint.MaxValue)
            throw new InvalidDataException("The packet cannot be represented in this capture file.");
        _writer.Write((uint)(frame.TimestampMicroseconds / 1_000_000));
        _writer.Write((uint)(frame.TimestampMicroseconds % 1_000_000));
        _writer.Write(frame.Data.Length); _writer.Write(frame.OriginalLength); _writer.Write(frame.Data);
        BytesWritten += 16 + frame.Data.Length;
    }
    public void Flush() => _writer.Flush();
    public void Dispose() => _writer.Dispose();
}

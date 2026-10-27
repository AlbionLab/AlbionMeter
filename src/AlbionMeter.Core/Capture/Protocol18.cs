using System.Buffers.Binary;
using System.Text;

namespace AlbionMeter.Core.Capture;

public sealed record WireMessage(byte Kind, byte Code, short? ReturnCode, Dictionary<byte, object?> Parameters);
public sealed record OpaqueValue(byte Type, byte[] Bytes);
public sealed record WireDecodeResult(string Status, WireMessage? Message = null, string? Detail = null);

public static class Protocol18
{
    public static WireDecodeResult Decode(ReadOnlySpan<byte> payload)
    {
        if (payload.Length < 2) return new("malformedMessage");
        if (payload[0] != 0xf3) return new("unsupportedMessageMagic");
        if ((payload[1] & 0x80) != 0) return new("encryptedMessage");
        if (payload[1] is not (2 or 3 or 4)) return new("unsupportedMessageKind");
        try
        {
            var reader = new Reader(payload[2..]);
            var message = reader.Message(payload[1], 0);
            if (reader.Remaining != 0) return new("malformedMessage", Detail: "Trailing bytes.");
            return new("decodedMessage", message);
        }
        catch (NotSupportedException exception) { return new("unsupportedValueType", Detail: exception.Message); }
        catch (Exception exception) when (exception is InvalidDataException or OverflowException or DecoderFallbackException)
        { return new("malformedMessage", Detail: exception.Message); }
    }
    private ref struct Reader(ReadOnlySpan<byte> input)
    {
        private ReadOnlySpan<byte> _remaining = input;
        private int _budget = 65536;
        private static readonly UTF8Encoding Utf8 = new(false, true);
        public int Remaining => _remaining.Length;
        private ReadOnlySpan<byte> Take(int count)
        {
            if (count < 0 || count > _remaining.Length) throw new InvalidDataException("Incomplete value.");
            var value = _remaining[..count]; _remaining = _remaining[count..]; return value;
        }
        private byte Byte() => Take(1)[0];
        private ushort Short() => BinaryPrimitives.ReadUInt16LittleEndian(Take(2));
        private ulong Unsigned(int bits)
        {
            ulong result = 0;
            for (var shift = 0; shift < bits; shift += 7)
            {
                var value = Byte();
                if (bits - shift < 7 && (value & 0x7f) >= (1 << (bits - shift))) throw new InvalidDataException("Variable integer overflow.");
                result |= (ulong)(value & 0x7f) << shift;
                if ((value & 0x80) == 0) return result;
            }
            throw new InvalidDataException("Unterminated variable integer.");
        }
        private long Signed(int bits)
        {
            var value = Unsigned(bits);
            return unchecked((long)(value >> 1)) ^ -((long)value & 1);
        }
        private int Length(int maximum = 65536)
        {
            var count = Unsigned(32);
            if (count > (uint)maximum) throw new InvalidDataException("Value exceeds the supported limit.");
            return (int)count;
        }
        public WireMessage Message(byte kind, int depth)
        {
            if (depth > 16) throw new InvalidDataException("Nested message limit exceeded.");
            var code = Byte();
            short? result = null;
            if (kind == 3)
            {
                result = unchecked((short)Short());
                var debug = Value(0, depth + 1);
                if (debug is not null and not string) throw new InvalidDataException("Invalid response debug value.");
            }
            var count = Byte();
            Dictionary<byte, object?> parameters = [];
            for (var i = 0; i < count; i++)
            {
                var key = Byte(); var value = Value(0, depth + 1);
                if (!parameters.TryAdd(key, value)) throw new InvalidDataException("Duplicate parameter key.");
            }
            return new(kind, code, result, parameters);
        }
        private object? Value(byte type, int depth)
        {
            if (depth > 16 || --_budget < 0) throw new InvalidDataException("Value budget exceeded.");
            if (type == 0) type = Byte();
            if (type is >= 128 and <= 228) return new OpaqueValue((byte)(type - 128), Take(Length(1_048_576)).ToArray());
            switch (type)
            {
                case 2: return Byte() != 0;
                case 3: return Byte();
                case 4: return unchecked((short)Short());
                case 5: return BinaryPrimitives.ReadSingleLittleEndian(Take(4));
                case 6: return BinaryPrimitives.ReadDoubleLittleEndian(Take(8));
                case 7: return Utf8.GetString(Take(Length(1_048_576)));
                case 8: return null;
                case 9: return (int)Signed(32);
                case 10: return Signed(64);
                case 11: return (int)Byte();
                case 12: return -(int)Byte();
                case 13: return (int)Short();
                case 14: return -(int)Short();
                case 15: return (long)Byte();
                case 16: return -(long)Byte();
                case 17: return (long)Short();
                case 18: return -(long)Short();
                case 19:
                    var custom = Byte(); return new OpaqueValue(custom, Take(Length(1_048_576)).ToArray());
                case 20:
                    var keyType = Byte(); var valueType = Byte();
                    if (valueType is 20 or 64) throw new NotSupportedException("Nested typed dictionaries require an explicit schema.");
                    return Map(keyType, valueType, depth);
                case 21: return Map(0, 0, depth);
                case 23: return Array(0, depth);
                case 24: return Message(2, depth + 1);
                case 25: return Message(3, depth + 1);
                case 26: return Message(4, depth + 1);
                case 27: return false;
                case 28: return true;
                case 29: return (short)0;
                case 30: return 0;
                case 31: return 0L;
                case 32: return 0f;
                case 33: return 0d;
                case 34: return (byte)0;
                case 64: return Array(0, depth);
                case 66:
                    var count = Length(); var packed = Take((count + 7) / 8); var booleans = new bool[count];
                    for (var i = 0; i < count; i++) booleans[i] = (packed[i / 8] & (1 << (i % 8))) != 0;
                    return booleans;
                case 67: return Take(Length(1_048_576)).ToArray();
                case 68: return Array(4, depth);
                case 69: return Array(5, depth);
                case 70: return Array(6, depth);
                case 71: return Array(7, depth);
                case 73: return Array(9, depth);
                case 74: return Array(10, depth);
                case 83:
                    var total = Length(); var customType = Byte(); var values = new OpaqueValue[total];
                    for (var i = 0; i < total; i++) values[i] = new(customType, Take(Length(1_048_576)).ToArray());
                    return values;
                case 85: return Array(21, depth);
                default: throw new NotSupportedException($"Value type {type}.");
            }
        }
        private object?[] Array(byte type, int depth)
        {
            var count = Length();
            if (count > _budget) throw new InvalidDataException("Array exceeds the value budget.");
            var values = new object?[count];
            for (var i = 0; i < count; i++) values[i] = Value(type, depth + 1);
            return values;
        }
        private KeyValuePair<object?, object?>[] Map(byte keyType, byte valueType, int depth)
        {
            var count = Length();
            if (count > _budget / 2) throw new InvalidDataException("Map exceeds the value budget.");
            var values = new KeyValuePair<object?, object?>[count];
            for (var i = 0; i < count; i++) values[i] = new(Value(keyType, depth + 1), Value(valueType, depth + 1));
            return values;
        }
    }
}

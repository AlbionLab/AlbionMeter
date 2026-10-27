using System.Globalization;
using AlbionMeter.Core.Capture;

namespace AlbionMeter.Core.Observation;

public sealed record LocalCharacterSnapshot(Guid CharacterId, long? ObjectId, string Name, string? Guild,
    string? Alliance, string? MapId, string? DestinationMapId, string LocationState,
    long ObservedAtMicroseconds, string Connection);

public sealed class LocalCharacterObservation
{
    private LocalCharacterSnapshot? _current;
    public LocalCharacterSnapshot? Snapshot => _current;
    public void Restore(LocalCharacterSnapshot character)
    {
        if (character.CharacterId == Guid.Empty || string.IsNullOrWhiteSpace(character.Name)) return;
        _current = character with { ObjectId = null, Connection = "", LocationState = "last-observed" };
    }
    public void Observe(WireMessage message, string connection, long timestamp)
    {
        if (_current is { } current && timestamp < current.ObservedAtMicroseconds) return;
        var parameters = message.Parameters;
        object? Field(byte key) => parameters.GetValueOrDefault(key);
        if (message.Kind == 3 && message.ReturnCode == 0 && Integer(Field(253)) is { } operation)
        {
            if (operation == 2)
            {
                var id = Identity(Field(1));
                var name = Text(Field(2), 100);
                if (id is null || string.IsNullOrWhiteSpace(name)) return;
                _current = new(id.Value, Integer(Field(0)), name,
                    Text(Field(58), 200), Text(Field(79), 100),
                    Map(Field(8)), null, "observed", timestamp, connection);
            }
            else if (operation == 41 && _current is { } known && known.Connection == connection)
            {
                var destination = Map(Field(0));
                if (destination is null) return;
                _current = known with { MapId = null, DestinationMapId = destination, LocationState = "changing", ObservedAtMicroseconds = timestamp };
            }
        }
        else if (message.Kind == 4 && Integer(Field(252)) == 29
            && _current is { } local && local.Connection == connection
            && Identity(Field(7)) == local.CharacterId)
        {
            _current = local with
            {
                ObjectId = Integer(Field(0)) ?? local.ObjectId,
                Guild = Text(Field(8), 200),
                Alliance = Text(Field(51), 100),
                ObservedAtMicroseconds = timestamp
            };
        }
    }
    private static Guid? Identity(object? value) => value is byte[] { Length: 16 } bytes && new Guid(bytes) is var id && id != Guid.Empty ? id : null;
    private static long? Integer(object? value) => value switch { byte b => b, short s => s, int i => i, long l => l, _ => null };
    private static string? Text(object? value, int maximum) => value is string text && text.Length <= maximum && !text.Any(char.IsControl) ? text.Trim() : null;
    private static string? Map(object? value)
    {
        var text = Text(value, 512) ?? Integer(value)?.ToString(CultureInfo.InvariantCulture);
        return string.IsNullOrWhiteSpace(text) ? null : text;
    }
}

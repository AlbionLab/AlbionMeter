using AlbionMeter.Core.Capture;

namespace AlbionMeter.Core.Observation;

public sealed record PartyRosterMember(Guid Id, string Name);
public sealed record PartyRosterSnapshot(PartyRosterMember[] Members, long ObservedAtMicroseconds, bool Complete, string Source = "capture", long? ListedAtMicroseconds = null, long? PartyId = null)
{
    public long Listed => ListedAtMicroseconds ?? ObservedAtMicroseconds;
    public bool IsConfirmedSince(long? captureStarted) => Source == "user-confirmed"
        || Source == "capture" && captureStarted is { } started && Listed >= started;
}

public sealed class PartyRosterObservation
{
    private PartyRosterSnapshot? _current;
    private Guid? _localId;
    public PartyRosterSnapshot? Snapshot => _current;
    public bool Restore(PartyRosterSnapshot value)
    {
        if (_current is not null && value.ObservedAtMicroseconds <= _current.ObservedAtMicroseconds) return false;
        if (value.Members.Length > 100 || value.Members.Any(member => member.Id == Guid.Empty || string.IsNullOrWhiteSpace(member.Name) || member.Name.Length > 100 || member.Name.Any(char.IsControl))
            || value.Members.Select(member => member.Id).Distinct().Count() != value.Members.Length) return false;
        if (_localId is { } id && value.Members.Length > 0 && !value.Members.Any(member => member.Id == id)) return false;
        _current = value;
        return true;
    }
    private PartyRosterSnapshot Without(Guid id, long timestamp) => id == _localId
        ? new([], timestamp, true, ListedAtMicroseconds: timestamp)
        : new((_current?.Members ?? []).Where(member => member.Id != id).ToArray(), timestamp, _current?.Complete ?? false, ListedAtMicroseconds: _current?.Listed ?? 0, PartyId: _current?.PartyId);
    public void Observe(WireMessage message, LocalCharacterSnapshot? local, long timestamp)
    {
        if (local is not null && _localId != local.CharacterId)
        {
            if (_localId is not null) _current = null;
            _localId = local.CharacterId;
        }
        if (message.Kind != 4 || (_current is not null && timestamp < _current.ObservedAtMicroseconds)) return;
        var fields = message.Parameters;
        if (!fields.TryGetValue(252, out var value) || value is not (byte or short or int or long)) return;
        var code = Convert.ToInt64(value);
        if (code == 230)
        {
            if (fields.GetValueOrDefault((byte)8) is not byte[] bytes || bytes.Length == 0 || bytes.Length % 16 != 0 || bytes.Length > 1600
                || fields.GetValueOrDefault((byte)9) is not Array names || names.Length != bytes.Length / 16) return;
            var members = new List<PartyRosterMember>();
            for (var index = 0; index < names.Length; index++)
            {
                var id = new Guid(bytes.AsSpan(index * 16, 16));
                if (id == Guid.Empty || names.GetValue(index) is not string name || string.IsNullOrWhiteSpace(name) || name.Length > 100 || name.Any(char.IsControl)) return;
                members.Add(new(id, name));
            }
            if (members.Select(member => member.Id).Distinct().Count() != members.Count) return;
            if (_localId is { } localId && !members.Any(member => member.Id == localId)) return;
            _current = new(members.ToArray(), timestamp, true, ListedAtMicroseconds: timestamp, PartyId: fields.GetValueOrDefault((byte)0) is byte or short or int or long ? Convert.ToInt64(fields[0]) : null);
        }
        else if (code == 231)
        {
            _current = new([], timestamp, true, ListedAtMicroseconds: timestamp);
        }
        else if (code == 235)
        {
            if (fields.GetValueOrDefault((byte)1) is not byte[] { Length: 16 } removed || new Guid(removed) == Guid.Empty) return;
            var id = new Guid(removed);
            _current = Without(id, timestamp);
        }
        else if (code is 232 or 234)
        {
            if (fields.GetValueOrDefault((byte)1) is not byte[] { Length: 16 } identity) return;
            var id = new Guid(identity);
            if (id == Guid.Empty) return;
            if (code == 234)
            {
                _current = Without(id, timestamp);
                return;
            }
            if (fields.GetValueOrDefault((byte)2) is not string name || string.IsNullOrWhiteSpace(name) || name.Length > 100 || name.Any(char.IsControl)) return;
            var members = (_current?.Members ?? []).Where(member => member.Id != id).Append(new(id, name)).ToArray();
            if (members.Length <= 100) _current = new(members, timestamp, _current?.Complete ?? false, ListedAtMicroseconds: _current?.Listed ?? 0, PartyId: _current?.PartyId);
        }
    }
}

using System.Text.Json;
using AlbionMeter.Core.Observation;

namespace AlbionMeter.Server;

public sealed class CharacterContextStore
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly object _gate = new();
    private readonly string _path;
    private LocalCharacterSnapshot? _character;
    private PartyRosterSnapshot? _party;
    private readonly string _partyPath;
    public CharacterContextStore(IConfiguration configuration)
    {
        var directory = configuration["Storage:Directory"] ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "AlbionMeter");
        _path = Path.Combine(directory, "captures", "local-character.json");
        _partyPath = Path.Combine(directory, "captures", "local-party.json");
        try
        {
            if (File.Exists(_path))
            {
                var value = JsonSerializer.Deserialize<LocalCharacterSnapshot>(File.ReadAllText(_path), JsonOptions);
                if (value is { CharacterId: var id, Name.Length: > 0 and <= 100 } && id != Guid.Empty) _character = value;
            }
        }
        catch (Exception exception) when (exception is IOException or JsonException or UnauthorizedAccessException) { }
        try
        {
            if (File.Exists(_partyPath))
            {
                var value = JsonSerializer.Deserialize<PartyRosterSnapshot>(File.ReadAllText(_partyPath), JsonOptions);
                if (value?.Members is { Length: <= 100 } members && members.All(member => member.Id != Guid.Empty && !string.IsNullOrWhiteSpace(member.Name))) _party = value;
            }
        }
        catch (Exception exception) when (exception is IOException or JsonException or UnauthorizedAccessException) { }
    }
    public LocalCharacterSnapshot? Snapshot { get { lock (_gate) return _character; } }
    public string? StorageError { get; private set; }
    public PartyRosterSnapshot? Party { get { lock (_gate) return _party; } }
    public void ObserveParty(PartyRosterSnapshot value)
    {
        lock (_gate)
        {
            if (_party is not null && value.ObservedAtMicroseconds <= _party.ObservedAtMicroseconds) return;
            _party = value;
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(_partyPath)!);
                File.WriteAllText(_partyPath + ".tmp", JsonSerializer.Serialize(value, JsonOptions));
                File.Move(_partyPath + ".tmp", _partyPath, true);
            }
            catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { StorageError = exception.Message; }
        }
    }
    public void Observe(LocalCharacterSnapshot value)
    {
        lock (_gate)
        {
            if (_character == value || (_character is not null && value.ObservedAtMicroseconds < _character.ObservedAtMicroseconds)) return;
            var changedCharacter = _character is not null && _character.CharacterId != value.CharacterId;
            _character = value;
            if (changedCharacter) _party = null;
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
                if (changedCharacter && File.Exists(_partyPath)) File.Delete(_partyPath);
                File.WriteAllText(_path + ".tmp", JsonSerializer.Serialize(value, JsonOptions));
                File.Move(_path + ".tmp", _path, true);
                StorageError = null;
            }
            catch (Exception exception) when (exception is IOException or UnauthorizedAccessException) { StorageError = exception.Message; }
        }
    }
}

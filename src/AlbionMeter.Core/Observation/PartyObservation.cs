namespace AlbionMeter.Core.Observation;

public sealed record ObservationContext(Guid Connection, long ZoneGeneration);
public sealed record ObservedCharacter(Guid Id, string Name, string? WeaponId);
public sealed record ResolvedCharacter(ObservedCharacter Character, bool? IsPartyMember);
public sealed record PartyObservationSnapshot(ObservationContext Context, bool RosterComplete,
    ObservedCharacter[] Members, int BoundEntities, long StaleUpdates, long BindingConflicts);

public sealed class PartyObservation
{
    private readonly object _gate = new();
    private readonly Dictionary<Guid, ObservedCharacter> _characters = [];
    private readonly Dictionary<long, (Guid Id, long Revision)> _bindings = [];
    private readonly Dictionary<Guid, (long ObjectId, long Revision)> _objects = [];
    private readonly HashSet<Guid> _party = [];
    private readonly Dictionary<Guid, long> _identityRevisions = [];
    private ObservationContext _context = new(Guid.NewGuid(), 0);
    private long _partyRevision = -1;
    private long _staleUpdates, _bindingConflicts;
    private bool _rosterComplete;
    public ObservationContext Context { get { lock (_gate) return _context; } }
    public ObservationContext BeginConnection()
    {
        lock (_gate)
        {
            _characters.Clear(); _bindings.Clear(); _objects.Clear(); _party.Clear(); _identityRevisions.Clear();
            _partyRevision = -1; _rosterComplete = false;
            return _context = new(Guid.NewGuid(), 0);
        }
    }
    public ObservationContext EnterZone()
    {
        lock (_gate)
        {
            _bindings.Clear(); _objects.Clear(); _identityRevisions.Clear();
            return _context = _context with { ZoneGeneration = checked(_context.ZoneGeneration + 1) };
        }
    }
    public bool ReplaceParty(ObservationContext context, long revision, IEnumerable<ObservedCharacter> members)
    {
        var values = members.Take(101).ToArray();
        if (values.Length > 100 || values.Any(value => value.Id == Guid.Empty || string.IsNullOrWhiteSpace(value.Name))
            || values.Select(value => value.Id).Distinct().Count() != values.Length)
            throw new ArgumentException("A roster must contain at most 100 distinct characters with valid names.", nameof(members));
        lock (_gate)
        {
            if (!Accept(context) || revision <= _partyRevision) { _staleUpdates++; return false; }
            if (_characters.Count + values.Count(value => !_characters.ContainsKey(value.Id)) > 4096)
                throw new InvalidOperationException("The observation character limit was reached.");
            foreach (var value in values)
            {
                _characters[value.Id] = _characters.TryGetValue(value.Id, out var existing)
                    ? existing with { Name = value.Name } : value;
            }
            _party.Clear(); _party.UnionWith(values.Select(value => value.Id));
            _partyRevision = revision; _rosterComplete = true; return true;
        }
    }
    public bool Observe(ObservationContext context, long revision, long objectId, ObservedCharacter character)
    {
        if (character.Id == Guid.Empty || string.IsNullOrWhiteSpace(character.Name) || revision < 0)
            throw new ArgumentException("Character observations require identity, name and a nonnegative revision.");
        lock (_gate)
        {
            if (!Accept(context)) { _staleUpdates++; return false; }
            var hasBinding = _bindings.TryGetValue(objectId, out var binding);
            var hasObject = _objects.TryGetValue(character.Id, out var previous);
            if ((hasBinding && revision <= binding.Revision) || (_identityRevisions.TryGetValue(character.Id, out var lastRevision) && revision <= lastRevision))
            { _staleUpdates++; return false; }
            if (!_characters.ContainsKey(character.Id) && _characters.Count >= 4096)
                throw new InvalidOperationException("The observation character limit was reached.");
            if (hasBinding && binding.Id != character.Id)
            { _objects.Remove(binding.Id); _bindingConflicts++; }
            if (hasObject && previous.ObjectId != objectId) _bindings.Remove(previous.ObjectId);
            _characters[character.Id] = character; _identityRevisions[character.Id] = revision;
            _bindings[objectId] = (character.Id, revision); _objects[character.Id] = (objectId, revision);
            return true;
        }
    }
    public ResolvedCharacter? Resolve(ObservationContext context, long objectId)
    {
        lock (_gate)
        {
            if (!Accept(context) || !_bindings.TryGetValue(objectId, out var binding)) return null;
            return new(_characters[binding.Id], _rosterComplete ? _party.Contains(binding.Id) : null);
        }
    }
    public PartyObservationSnapshot Snapshot()
    {
        lock (_gate) return new(_context, _rosterComplete, _party.Select(id => _characters[id]).ToArray(),
            _bindings.Count, _staleUpdates, _bindingConflicts);
    }
    private bool Accept(ObservationContext context) => context == _context;
}

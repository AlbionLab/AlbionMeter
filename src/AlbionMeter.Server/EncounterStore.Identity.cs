using System.Text.Json;
using System.Text.Json.Nodes;
using AlbionMeter.Core.Observation;
using Microsoft.Data.Sqlite;

namespace AlbionMeter.Server;

public sealed partial class EncounterStore
{
    public sealed record IdentityResolution(string MemberId, Guid CharacterId, string Name, string Method);
    private const int FingerprintsPerCharacter = 8;
    private readonly Dictionary<Guid, string> _writtenCharacters = [];
    private readonly HashSet<string> _writtenEvidence = [];
    private bool _identitySchema;

    private void EnsureIdentitySchema(SqliteConnection connection)
    {
        if (_identitySchema) return;
        using var command = connection.CreateCommand();
        command.CommandText = """
            CREATE TABLE IF NOT EXISTS characters (id TEXT PRIMARY KEY, name TEXT NOT NULL, fingerprints TEXT NOT NULL, seen_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS unidentified_players (member_id TEXT PRIMARY KEY, record_id TEXT, object_id INTEGER NOT NULL, equipment TEXT,
                candidates TEXT NOT NULL, observed_at TEXT NOT NULL, resolved_id TEXT, resolved_name TEXT, method TEXT);
            CREATE TABLE IF NOT EXISTS identity_sightings (character_id TEXT NOT NULL, object_id INTEGER NOT NULL, visit TEXT NOT NULL, seen_at TEXT NOT NULL, method TEXT NOT NULL,
                PRIMARY KEY (character_id, object_id, visit));
            CREATE INDEX IF NOT EXISTS identity_sightings_object ON identity_sightings (visit, object_id);
            CREATE TABLE IF NOT EXISTS skill_weapons (skill INTEGER PRIMARY KEY, weapons TEXT NOT NULL);
            DROP TABLE IF EXISTS effect_items;
            CREATE TABLE IF NOT EXISTS identity_timeline (character_id TEXT NOT NULL, at TEXT NOT NULL, kind TEXT NOT NULL, visit TEXT NOT NULL, name TEXT, object_id INTEGER, equipment TEXT,
                PRIMARY KEY (character_id, at, kind));
            CREATE INDEX IF NOT EXISTS identity_timeline_at ON identity_timeline (at);
            """;
        command.ExecuteNonQuery();
        foreach (var (table, column) in new[] { ("characters", "skills"), ("unidentified_players", "skills"), ("unidentified_players", "visit") })
        {
            using var alter = connection.CreateCommand();
            alter.CommandText = $"ALTER TABLE {table} ADD COLUMN {column} TEXT";
            try { alter.ExecuteNonQuery(); } catch (SqliteException) { }
        }
        _identitySchema = true;
    }

    public Dictionary<int, Dictionary<int, int>> KnownSkillWeapons()
    {
        using var connection = Open();
        EnsureIdentitySchema(connection);
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT skill, weapons FROM skill_weapons";
        using var reader = command.ExecuteReader();
        var result = new Dictionary<int, Dictionary<int, int>>();
        while (reader.Read()) result[reader.GetInt32(0)] = JsonSerializer.Deserialize<Dictionary<int, int>>(reader.GetString(1)) ?? [];
        return result;
    }

    private readonly Dictionary<int, string> _writtenSkills = [];
    public void ObserveSkillWeapons(IReadOnlyDictionary<int, Dictionary<int, int>> skills)
    {
        using var connection = Open();
        EnsureIdentitySchema(connection);
        using var transaction = connection.BeginTransaction();
        foreach (var (skill, weapons) in skills)
        {
            var json = JsonSerializer.Serialize(weapons.OrderBy(pair => pair.Key).ToDictionary());
            if (_writtenSkills.GetValueOrDefault(skill) == json) continue;
            using var write = connection.CreateCommand();
            write.Transaction = transaction;
            write.CommandText = "INSERT OR REPLACE INTO skill_weapons VALUES ($skill, $weapons)";
            write.Parameters.AddWithValue("$skill", skill);
            write.Parameters.AddWithValue("$weapons", json);
            write.ExecuteNonQuery();
            _writtenSkills[skill] = json;
        }
        transaction.Commit();
    }

    public void ObserveTimeline(IEnumerable<IdentityTimelineEntry> timeline)
    {
        using var connection = Open();
        EnsureIdentitySchema(connection);
        using var transaction = connection.BeginTransaction();
        using var write = connection.CreateCommand();
        write.Transaction = transaction;
        write.CommandText = "INSERT OR IGNORE INTO identity_timeline VALUES ($character, $at, $kind, $visit, $name, $object, $equipment)";
        foreach (var entry in timeline)
        {
            write.Parameters.Clear();
            write.Parameters.AddWithValue("$character", entry.CharacterId.ToString());
            write.Parameters.AddWithValue("$at", DateTimeOffset.FromUnixTimeMilliseconds(entry.Timestamp / 1000).ToString("O"));
            write.Parameters.AddWithValue("$kind", entry.Kind);
            write.Parameters.AddWithValue("$visit", entry.Visit);
            write.Parameters.AddWithValue("$name", (object?)entry.Name ?? DBNull.Value);
            write.Parameters.AddWithValue("$object", (object?)entry.ObjectId ?? DBNull.Value);
            write.Parameters.AddWithValue("$equipment", entry.Equipment is null ? DBNull.Value : JsonSerializer.Serialize(entry.Equipment));
            write.ExecuteNonQuery();
        }
        transaction.Commit();
    }

    public Dictionary<Guid, KnownCharacter> KnownCharacters()
    {
        using var connection = Open();
        EnsureIdentitySchema(connection);
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT id, name, fingerprints, skills FROM characters";
        using var reader = command.ExecuteReader();
        var result = new Dictionary<Guid, KnownCharacter>();
        while (reader.Read())
            if (Guid.TryParse(reader.GetString(0), out var id))
                result[id] = new(reader.GetString(1), JsonSerializer.Deserialize<int?[][]>(reader.GetString(2))?.FirstOrDefault(), reader.IsDBNull(3) ? null : JsonSerializer.Deserialize<int[]>(reader.GetString(3)));
        return result;
    }

    public IReadOnlyList<IdentityResolution> ObserveIdentities(IReadOnlyDictionary<Guid, KnownCharacter> characters, IEnumerable<UnidentifiedPlayer> unidentified, IEnumerable<IdentitySighting>? sightings = null)
    {
        using var connection = Open();
        EnsureIdentitySchema(connection);
        var changed = false;
        using (var transaction = connection.BeginTransaction())
        {
            foreach (var (id, character) in characters)
            {
                var key = $"{character.Name}|{(character.Equipment is null ? "" : string.Join(',', character.Equipment))}|{(character.Skills is null ? "" : string.Join(',', character.Skills))}";
                if (_writtenCharacters.GetValueOrDefault(id) == key) continue;
                using var read = connection.CreateCommand();
                read.Transaction = transaction;
                read.CommandText = "SELECT fingerprints, skills FROM characters WHERE id = $id";
                read.Parameters.AddWithValue("$id", id.ToString());
                List<int?[]> fingerprints = []; var skills = new SortedSet<int>();
                using (var stored = read.ExecuteReader())
                    if (stored.Read())
                    {
                        fingerprints = JsonSerializer.Deserialize<List<int?[]>>(stored.GetString(0)) ?? [];
                        if (!stored.IsDBNull(1)) skills.UnionWith(JsonSerializer.Deserialize<int[]>(stored.GetString(1)) ?? []);
                    }
                if (character.Skills is not null) skills.UnionWith(character.Skills);
                if (character.Equipment is { Length: >= 7 } equipment && !fingerprints.Any(known => known.SequenceEqual(equipment)))
                {
                    fingerprints.Insert(0, equipment);
                    if (fingerprints.Count > FingerprintsPerCharacter) fingerprints.RemoveRange(FingerprintsPerCharacter, fingerprints.Count - FingerprintsPerCharacter);
                }
                using var write = connection.CreateCommand();
                write.Transaction = transaction;
                write.CommandText = "INSERT OR REPLACE INTO characters (id, name, fingerprints, seen_at, skills) VALUES ($id, $name, $fingerprints, $seen, $skills)";
                write.Parameters.AddWithValue("$skills", skills.Count == 0 ? DBNull.Value : JsonSerializer.Serialize(skills.Take(64)));
                write.Parameters.AddWithValue("$id", id.ToString());
                write.Parameters.AddWithValue("$name", character.Name);
                write.Parameters.AddWithValue("$fingerprints", JsonSerializer.Serialize(fingerprints));
                write.Parameters.AddWithValue("$seen", DateTimeOffset.UtcNow.ToString("O"));
                write.ExecuteNonQuery();
                _writtenCharacters[id] = key;
                changed = true;
            }
            foreach (var player in unidentified)
            {
                if (!_writtenEvidence.Add(player.MemberId)) continue;
                using var write = connection.CreateCommand();
                write.Transaction = transaction;
                write.CommandText = "INSERT OR IGNORE INTO unidentified_players (member_id, record_id, object_id, equipment, candidates, observed_at, skills, visit) VALUES ($member, $record, $object, $equipment, $candidates, $observed, $skills, $visit)";
                write.Parameters.AddWithValue("$visit", (object?)player.Visit ?? DBNull.Value);
                write.Parameters.AddWithValue("$skills", player.Skills is { Length: > 0 } observed ? JsonSerializer.Serialize(observed) : DBNull.Value);
                write.Parameters.AddWithValue("$member", player.MemberId);
                write.Parameters.AddWithValue("$record", (object?)player.RecordId ?? DBNull.Value);
                write.Parameters.AddWithValue("$object", player.ObjectId);
                write.Parameters.AddWithValue("$equipment", player.Equipment is null ? DBNull.Value : JsonSerializer.Serialize(player.Equipment));
                write.Parameters.AddWithValue("$candidates", JsonSerializer.Serialize(player.Candidates));
                write.Parameters.AddWithValue("$observed", DateTimeOffset.FromUnixTimeMilliseconds(player.Timestamp / 1000).ToString("O"));
                changed |= write.ExecuteNonQuery() > 0;
            }
            foreach (var sighting in sightings ?? [])
            {
                if (!_writtenEvidence.Add($"sighting:{sighting.CharacterId}:{sighting.ObjectId}:{sighting.Visit}")) continue;
                using var write = connection.CreateCommand();
                write.Transaction = transaction;
                write.CommandText = "INSERT OR IGNORE INTO identity_sightings VALUES ($character, $object, $visit, $seen, $method)";
                write.Parameters.AddWithValue("$character", sighting.CharacterId.ToString());
                write.Parameters.AddWithValue("$object", sighting.ObjectId);
                write.Parameters.AddWithValue("$visit", sighting.Visit);
                write.Parameters.AddWithValue("$seen", DateTimeOffset.FromUnixTimeMilliseconds(Math.Max(0, sighting.Timestamp) / 1000).ToString("O"));
                write.Parameters.AddWithValue("$method", sighting.Method);
                changed |= write.ExecuteNonQuery() > 0;
            }
            transaction.Commit();
        }
        return changed ? Reconcile(connection) : [];
    }

    private IReadOnlyList<IdentityResolution> Reconcile(SqliteConnection connection)
    {
        var characters = new Dictionary<Guid, (string Name, List<int?[]> Fingerprints, HashSet<int> Skills)>();
        using (var command = connection.CreateCommand())
        {
            command.CommandText = "SELECT id, name, fingerprints, skills FROM characters";
            using var reader = command.ExecuteReader();
            while (reader.Read())
                if (Guid.TryParse(reader.GetString(0), out var id))
                    characters[id] = (reader.GetString(1), JsonSerializer.Deserialize<List<int?[]>>(reader.GetString(2)) ?? [], reader.IsDBNull(3) ? [] : [.. JsonSerializer.Deserialize<int[]>(reader.GetString(3)) ?? []]);
        }
        var pending = new List<(string Member, string? Record, int?[]? Equipment, Guid[] Candidates, HashSet<int> Skills, string? Visit, long Object)>();
        using (var command = connection.CreateCommand())
        {
            command.CommandText = "SELECT member_id, record_id, equipment, candidates, skills, visit, object_id FROM unidentified_players WHERE resolved_id IS NULL";
            using var reader = command.ExecuteReader();
            while (reader.Read())
                pending.Add((reader.GetString(0), reader.IsDBNull(1) ? null : reader.GetString(1),
                    reader.IsDBNull(2) ? null : JsonSerializer.Deserialize<int?[]>(reader.GetString(2)), JsonSerializer.Deserialize<Guid[]>(reader.GetString(3)) ?? [],
                    reader.IsDBNull(4) ? [] : [.. JsonSerializer.Deserialize<int[]>(reader.GetString(4)) ?? []], reader.IsDBNull(5) ? null : reader.GetString(5), reader.GetInt64(6)));
        }
        var resolutions = new List<IdentityResolution>();
        var labels = new Dictionary<string, string>();
        var recordMembers = pending.Where(player => player.Record is not null).Select(player => player.Record!).Distinct().ToDictionary(record => record, record => RecordMembers(connection, record));
        using (var command = connection.CreateCommand())
        {
            command.CommandText = "SELECT record_id, resolved_id FROM unidentified_players WHERE resolved_id IS NOT NULL AND record_id IS NOT NULL";
            using var reader = command.ExecuteReader();
            while (reader.Read())
                if (recordMembers.TryGetValue(reader.GetString(0), out var members) && Guid.TryParse(reader.GetString(1), out var resolved)) members.Add(resolved);
        }
        HashSet<Guid> Named(string? record) => record is null ? [] : [.. recordMembers[record], .. resolutions.Where(resolution => pending.Any(player => player.Member == resolution.MemberId && player.Record == record)).Select(resolution => resolution.CharacterId)];
        foreach (var player in pending.Where(player => player.Visit is not null))
        {
            using var command = connection.CreateCommand();
            command.CommandText = "SELECT DISTINCT character_id FROM identity_sightings WHERE visit = $visit AND object_id = $object";
            command.Parameters.AddWithValue("$visit", player.Visit);
            command.Parameters.AddWithValue("$object", player.Object);
            using var reader = command.ExecuteReader();
            var seen = new List<Guid>();
            while (reader.Read()) if (Guid.TryParse(reader.GetString(0), out var id)) seen.Add(id);
            if (seen.Count == 1 && characters.TryGetValue(seen[0], out var character)) resolutions.Add(new(player.Member, seen[0], character.Name, "same-zone"));
        }
        for (var changed = true; changed;)
        {
            changed = false;
            foreach (var player in pending.Where(player => resolutions.All(resolution => resolution.MemberId != player.Member)))
            {
                var named = Named(player.Record);
                var open = player.Candidates.Where(candidate => !named.Contains(candidate) && characters.ContainsKey(candidate)).ToArray();
                if (player.Record is not null && open.Length == 1) { resolutions.Add(new(player.Member, open[0], characters[open[0]].Name, "elimination")); changed = true; }
                else if (open.Length is >= 2 and <= 4)
                {
                    var names = open.Select(candidate => characters[candidate].Name).Order().ToArray();
                    labels[player.Member] = $"{string.Join(", ", names.SkipLast(1))} or {names[^1]} (obj {player.Object})";
                }
            }
        }
        foreach (var resolution in resolutions) Apply(connection, resolution);
        foreach (var (member, label) in labels) if (resolutions.All(resolution => resolution.MemberId != member)) Label(connection, member, label);
        return resolutions;
    }

    private static void Label(SqliteConnection connection, string memberId, string label)
    {
        foreach (var table in new[] { "encounters", "dungeon_runs" })
        {
            var updates = new List<(string Id, string Snapshot)>();
            using (var select = connection.CreateCommand())
            {
                select.CommandText = $"SELECT id, snapshot FROM {table} WHERE instr(snapshot, $member) > 0";
                select.Parameters.AddWithValue("$member", memberId);
                using var reader = select.ExecuteReader();
                while (reader.Read())
                    if (JsonNode.Parse(reader.GetString(1)) is { } node && Rename(node, memberId, label)) updates.Add((reader.GetString(0), node.ToJsonString()));
            }
            foreach (var (id, snapshot) in updates)
            {
                using var update = connection.CreateCommand();
                update.CommandText = $"UPDATE {table} SET snapshot = $snapshot WHERE id = $id";
                update.Parameters.AddWithValue("$snapshot", snapshot);
                update.Parameters.AddWithValue("$id", id);
                update.ExecuteNonQuery();
            }
        }
    }

    private static HashSet<Guid> RecordMembers(SqliteConnection connection, string recordId)
    {
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT snapshot FROM encounters WHERE id = $id";
        command.Parameters.AddWithValue("$id", recordId);
        if (command.ExecuteScalar() is not string json || JsonNode.Parse(json) is not JsonObject snapshot) return [];
        return new[] { "members", "formerMembers" }.SelectMany(key => snapshot[key]?.AsArray() ?? [])
            .Where(member => (member?["damage"]?.GetValue<double>() ?? 0) + (member?["healing"]?.GetValue<double>() ?? 0) > 0)
            .Select(member => member?["id"]?.GetValue<string>()).Where(id => Guid.TryParse(id, out _)).Select(id => Guid.Parse(id!)).ToHashSet();
    }

    private void ReapplyIdentities(SqliteConnection connection, SqliteTransaction transaction, string recordId)
    {
        var resolved = new List<IdentityResolution>();
        using (var command = connection.CreateCommand())
        {
            command.Transaction = transaction;
            command.CommandText = "SELECT member_id, resolved_id, resolved_name, method FROM unidentified_players WHERE record_id = $record AND resolved_id IS NOT NULL";
            command.Parameters.AddWithValue("$record", recordId);
            using var reader = command.ExecuteReader();
            while (reader.Read()) if (Guid.TryParse(reader.GetString(1), out var id)) resolved.Add(new(reader.GetString(0), id, reader.GetString(2), reader.GetString(3)));
        }
        if (resolved.Count == 0) return;
        using var select = connection.CreateCommand();
        select.Transaction = transaction;
        select.CommandText = "SELECT snapshot FROM encounters WHERE id = $id";
        select.Parameters.AddWithValue("$id", recordId);
        if (select.ExecuteScalar() is not string json || JsonNode.Parse(json) is not { } node) return;
        var changed = false;
        foreach (var resolution in resolved) changed |= Merge(node, resolution.MemberId, resolution.CharacterId.ToString(), resolution.Name);
        if (!changed) return;
        using var update = connection.CreateCommand();
        update.Transaction = transaction;
        update.CommandText = "UPDATE encounters SET snapshot = $snapshot WHERE id = $id";
        update.Parameters.AddWithValue("$snapshot", node.ToJsonString());
        update.Parameters.AddWithValue("$id", recordId);
        update.ExecuteNonQuery();
    }

    private static void Apply(SqliteConnection connection, IdentityResolution resolution)
    {
        foreach (var table in new[] { "encounters", "dungeon_runs" })
        {
            var updates = new List<(string Id, string Snapshot)>();
            using (var select = connection.CreateCommand())
            {
                select.CommandText = $"SELECT id, snapshot FROM {table} WHERE instr(snapshot, $member) > 0";
                select.Parameters.AddWithValue("$member", resolution.MemberId);
                using var reader = select.ExecuteReader();
                while (reader.Read())
                    if (JsonNode.Parse(reader.GetString(1)) is { } node && Merge(node, resolution.MemberId, resolution.CharacterId.ToString(), resolution.Name)) updates.Add((reader.GetString(0), node.ToJsonString()));
            }
            foreach (var (id, snapshot) in updates)
            {
                using var update = connection.CreateCommand();
                update.CommandText = $"UPDATE {table} SET snapshot = $snapshot WHERE id = $id";
                update.Parameters.AddWithValue("$snapshot", snapshot);
                update.Parameters.AddWithValue("$id", id);
                update.ExecuteNonQuery();
            }
        }
        using (var merge = connection.CreateCommand())
        {
            merge.CommandText = "SELECT (SELECT skills FROM unidentified_players WHERE member_id = $member), (SELECT skills FROM characters WHERE id = $id)";
            merge.Parameters.AddWithValue("$member", resolution.MemberId);
            merge.Parameters.AddWithValue("$id", resolution.CharacterId.ToString());
            using var reader = merge.ExecuteReader();
            if (reader.Read() && !reader.IsDBNull(0))
            {
                var skills = new SortedSet<int>(JsonSerializer.Deserialize<int[]>(reader.GetString(0)) ?? []);
                if (!reader.IsDBNull(1)) skills.UnionWith(JsonSerializer.Deserialize<int[]>(reader.GetString(1)) ?? []);
                reader.Close();
                using var write = connection.CreateCommand();
                write.CommandText = "UPDATE characters SET skills = $skills WHERE id = $id";
                write.Parameters.AddWithValue("$skills", JsonSerializer.Serialize(skills.Take(64)));
                write.Parameters.AddWithValue("$id", resolution.CharacterId.ToString());
                write.ExecuteNonQuery();
            }
        }
        using var mark = connection.CreateCommand();
        mark.CommandText = "UPDATE unidentified_players SET resolved_id = $id, resolved_name = $name, method = $method WHERE member_id = $member";
        mark.Parameters.AddWithValue("$id", resolution.CharacterId.ToString());
        mark.Parameters.AddWithValue("$name", resolution.Name);
        mark.Parameters.AddWithValue("$method", resolution.Method);
        mark.Parameters.AddWithValue("$member", resolution.MemberId);
        mark.ExecuteNonQuery();
    }

    private static bool Merge(JsonNode node, string memberId, string characterId, string name)
    {
        var merged = false;
        foreach (var snapshot in Snapshots(node))
        {
            var lists = new[] { "members", "formerMembers" }.Select(key => snapshot[key] as JsonArray).Where(list => list is not null).ToArray();
            JsonNode? Find(string id) => lists.SelectMany(list => list!).FirstOrDefault(member => member?["id"]?.GetValue<string>() == id);
            if (Find(memberId) is null) continue;
            if (Find(characterId) is { } existing)
            {
                if ((existing["damage"]?.GetValue<double>() ?? 0) + (existing["healing"]?.GetValue<double>() ?? 0) > 0) { merged |= Rename(snapshot, memberId, name); continue; }
                if (Find(memberId) is { } unidentified && unidentified["weapon"]?.GetValue<string>() == "Weapon not observed" && existing["weapon"]?.GetValue<string>() is { } weapon and not "Weapon not observed")
                    unidentified["weapon"] = weapon;
                foreach (var list in lists) if (list!.Contains(existing)) list.Remove(existing);
            }
            Rekey(snapshot, memberId, characterId);
            Rename(snapshot, characterId, name);
            merged = true;
        }
        return merged;
    }

    private static IEnumerable<JsonObject> Snapshots(JsonNode node)
    {
        if (node is JsonObject item)
        {
            if (item.ContainsKey("members") && item.ContainsKey("timeline")) yield return item;
            foreach (var child in item.Select(pair => pair.Value).Where(child => child is not null).ToArray())
                foreach (var nested in Snapshots(child!)) yield return nested;
        }
        else if (node is JsonArray array)
            foreach (var child in array.Where(child => child is not null).ToArray())
                foreach (var nested in Snapshots(child!)) yield return nested;
    }

    private static string Replace(string value, string from, string to) => value == from ? to : value.StartsWith(from + ":", StringComparison.Ordinal) ? to + value[from.Length..] : value;

    private static void Rekey(JsonNode node, string from, string to)
    {
        if (node is JsonObject item)
        {
            foreach (var key in item.Select(pair => pair.Key).ToArray())
            {
                var value = item[key];
                if (value is JsonValue text && text.GetValueKind() == JsonValueKind.String && text.GetValue<string>() is var current && Replace(current, from, to) != current) item[key] = Replace(current, from, to);
                else if (value is not null) Rekey(value, from, to);
                if (Replace(key, from, to) != key) { item.Remove(key); item[Replace(key, from, to)] = value; }
            }
        }
        else if (node is JsonArray array)
            foreach (var child in array.ToArray()) if (child is not null) Rekey(child, from, to);
    }

    private static bool Rename(JsonNode node, string memberId, string name)
    {
        var changed = false;
        if (node is JsonObject item)
        {
            if (item["id"]?.GetValueKind() == JsonValueKind.String && item["id"]!.GetValue<string>() == memberId && item.ContainsKey("name")) { item["name"] = name; changed = true; }
            if (item["playerId"]?.GetValueKind() == JsonValueKind.String && item["playerId"]!.GetValue<string>() == memberId && item.ContainsKey("playerName")) { item["playerName"] = name; changed = true; }
            foreach (var child in item.Select(pair => pair.Value).ToArray()) if (child is not null) changed |= Rename(child, memberId, name);
        }
        else if (node is JsonArray array)
            foreach (var child in array.ToArray()) if (child is not null) changed |= Rename(child, memberId, name);
        return changed;
    }
}

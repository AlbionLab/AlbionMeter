//! .NET semantics the C# reference relies on and the JSON exposes: `Dictionary`/`HashSet` enumeration order, `Guid`
//! byte layout, formatting and hash, and `DateTimeOffset` JSON text.

use serde::{Serialize, Serializer};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

/// A map that enumerates like .NET's `Dictionary<TKey,TValue>`: insertion order, except that a removed entry's slot is
/// reused by the next insertion (last freed first). Totals are summed and members listed in this order, so the Rust
/// numbers and arrays come out identical to the C# ones. Small maps (most timeline buckets) are scanned linearly and
/// only grow a hash index past `LINEAR` entries ([`LINEAR_LIMIT`] by default).
#[derive(Clone, Debug)]
pub struct CsMap<K, V, const LINEAR: usize = LINEAR_LIMIT> {
    slots: Vec<Option<(K, V)>>,
    free: Vec<usize>,
    len: usize,
    index: Option<HashMap<K, usize>>,
}

pub const LINEAR_LIMIT: usize = 16;

impl<K, V, const LINEAR: usize> Default for CsMap<K, V, LINEAR> {
    fn default() -> Self {
        Self { slots: Vec::new(), free: Vec::new(), len: 0, index: None }
    }
}

impl<K: Hash + Eq + Clone, V, const LINEAR: usize> CsMap<K, V, LINEAR> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn find<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
    {
        match &self.index {
            Some(index) => index.get(key).copied(),
            None => self.slots.iter().position(|slot| slot.as_ref().is_some_and(|(k, _)| k.borrow() == key)),
        }
    }
    pub fn contains_key<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.find(key).is_some()
    }
    pub fn get<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.find(key).map(|slot| &self.slots[slot].as_ref().unwrap().1)
    }
    pub fn get_mut<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let slot = self.find(key)?;
        Some(&mut self.slots[slot].as_mut().unwrap().1)
    }
    /// `dictionary[key] = value`: an existing key keeps its position.
    pub fn insert(&mut self, key: K, value: V) {
        if let Some(slot) = self.find(&key) {
            self.slots[slot].as_mut().unwrap().1 = value;
            return;
        }
        let slot = match self.free.pop() {
            Some(slot) => slot,
            None => {
                self.slots.push(None);
                self.slots.len() - 1
            }
        };
        self.len += 1;
        match &mut self.index {
            Some(index) => {
                index.insert(key.clone(), slot);
            }
            None if self.len > LINEAR => {
                let mut index: HashMap<K, usize> =
                    self.slots.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|(k, _)| (k.clone(), i))).collect();
                index.insert(key.clone(), slot);
                self.index = Some(index);
            }
            None => {}
        }
        self.slots[slot] = Some((key, value));
    }
    /// Inserts a key the caller knows is absent, skipping the lookup.
    pub fn insert_new(&mut self, key: K, value: V) {
        debug_assert!(!self.contains_key(&key));
        if self.index.is_none() && self.len < LINEAR {
            let slot = match self.free.pop() {
                Some(slot) => slot,
                None => {
                    self.slots.push(None);
                    self.slots.len() - 1
                }
            };
            self.slots[slot] = Some((key, value));
            self.len += 1;
        } else {
            self.insert(key, value);
        }
    }
    /// `TryAdd`.
    pub fn try_add(&mut self, key: K, value: V) -> bool {
        if self.contains_key(&key) {
            return false;
        }
        self.insert(key, value);
        true
    }
    pub fn get_or_insert_with(&mut self, key: K, value: impl FnOnce() -> V) -> &mut V {
        let slot = match self.find(&key) {
            Some(slot) => slot,
            None => {
                self.insert(key.clone(), value());
                self.find(&key).unwrap()
            }
        };
        &mut self.slots[slot].as_mut().unwrap().1
    }
    pub fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let slot = self.find(key)?;
        if let Some(index) = &mut self.index {
            index.remove(key);
        }
        self.free.push(slot);
        self.len -= 1;
        self.slots[slot].take().map(|(_, value)| value)
    }
    pub fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.len = 0;
        self.index = None;
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + Clone {
        self.slots.iter().filter_map(|slot| slot.as_ref().map(|(key, value)| (key, value)))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.slots.iter_mut().filter_map(|slot| slot.as_mut().map(|(key, value)| (&*key, value)))
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + Clone {
        self.iter().map(|(key, _)| key)
    }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + Clone {
        self.iter().map(|(_, value)| value)
    }
    /// Removes every entry the predicate rejects, in enumeration order (as `foreach … ToArray() Remove`).
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let doomed: Vec<K> = self.iter().filter(|(key, value)| !keep(key, value)).map(|(key, _)| key.clone()).collect();
        for key in doomed {
            self.remove(&key);
        }
    }
    /// `new Dictionary<K,V>(other)`: the copy is compacted, so it enumerates like the source but has no free slots.
    pub fn compacted(&self) -> Self
    where
        V: Clone,
    {
        let mut copy = Self::new();
        for (key, value) in self.iter() {
            copy.insert(key.clone(), value.clone());
        }
        copy
    }
}

impl<K: Hash + Eq + Clone, V, const LINEAR: usize> FromIterator<(K, V)> for CsMap<K, V, LINEAR> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(items: I) -> Self {
        let mut map = Self::new();
        for (key, value) in items {
            map.insert(key, value);
        }
        map
    }
}

impl<K: Hash + Eq + Clone, V: PartialEq, const LINEAR: usize> PartialEq for CsMap<K, V, LINEAR> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(key, value)| other.get(key) == Some(value))
    }
}

impl<K: Serialize + Hash + Eq + Clone, V: Serialize, const LINEAR: usize> Serialize for CsMap<K, V, LINEAR> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter())
    }
}

/// `HashSet<T>` with the same enumeration rules as [`CsMap`].
#[derive(Clone, Debug)]
pub struct CsSet<K>(CsMap<K, ()>);

impl<K> Default for CsSet<K> {
    fn default() -> Self {
        Self(CsMap::default())
    }
}

impl<K: Hash + Eq + Clone> CsSet<K> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn contains<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.0.contains_key(key)
    }
    /// `Add`: false when already present.
    pub fn insert(&mut self, key: K) -> bool {
        self.0.try_add(key, ())
    }
    pub fn remove<Q: Hash + Eq + ?Sized>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.0.remove(key).is_some()
    }
    pub fn clear(&mut self) {
        self.0.clear()
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &K> + Clone {
        self.0.keys()
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.0.retain(|key, _| keep(key))
    }
}

impl<K: Hash + Eq + Clone> FromIterator<K> for CsSet<K> {
    fn from_iter<I: IntoIterator<Item = K>>(items: I) -> Self {
        let mut set = Self::new();
        for key in items {
            set.insert(key);
        }
        set
    }
}

impl<K: Serialize + Hash + Eq + Clone> Serialize for CsSet<K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

/// A .NET `Guid` as `new Guid(byte[16])` reads the game's bytes: the first three groups little-endian.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub const EMPTY: Guid = Guid([0; 16]);
    pub fn from_slice(bytes: &[u8]) -> Option<Guid> {
        bytes.try_into().ok().map(Guid)
    }
    pub fn is_empty(&self) -> bool {
        self.0 == [0; 16]
    }
    /// `ToByteArray()`.
    pub fn to_bytes(self) -> [u8; 16] {
        self.0
    }
    /// `Guid.GetHashCode()`: the four 32-bit words of the layout XORed together.
    pub fn dotnet_hash(&self) -> i32 {
        self.0.as_chunks::<4>().0.iter().fold(0i32, |hash, word| hash ^ i32::from_le_bytes(*word))
    }
    /// A random (version 4) id, as `Guid.NewGuid()`.
    pub fn new_v4() -> Guid {
        let mut bytes = random_bytes();
        bytes[7] = (bytes[7] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Guid(bytes)
    }
    /// `ToString("N")`.
    pub fn simple(&self) -> String {
        self.to_string().replace('-', "")
    }
    /// `Guid.TryParse` for the "D" and "N" formats the meter writes.
    pub fn parse(text: &str) -> Option<Guid> {
        let hex: String = match text.len() {
            36 if [8, 13, 18, 23].iter().all(|&i| text.as_bytes()[i] == b'-') => text.replace('-', ""),
            32 => text.to_owned(),
            _ => return None,
        };
        if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut display = [0u8; 16];
        for (i, byte) in display.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        let mut bytes = display;
        bytes[0..4].reverse();
        bytes[4..6].reverse();
        bytes[6..8].reverse();
        Some(Guid(bytes))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for Guid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Guid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Guid::parse(&text).ok_or_else(|| serde::de::Error::custom("invalid guid"))
    }
}

fn random_bytes() -> [u8; 16] {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut bytes = [0u8; 16];
    for half in bytes.as_chunks_mut::<8>().0 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        half.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes
}

/// A `DateTimeOffset` at offset zero, kept as Unix milliseconds; serialized the way System.Text.Json writes it
/// (`2024-05-01T12:00:00.123+00:00`, trailing fraction zeros dropped).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct DateTimeOffset(pub i64);

impl DateTimeOffset {
    pub fn from_unix_milliseconds(ms: i64) -> Self {
        Self(ms)
    }
    pub fn now() -> Self {
        Self(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
    }
    pub fn unix_milliseconds(self) -> i64 {
        self.0
    }
}

impl fmt::Display for DateTimeOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.0.div_euclid(86_400_000);
        let ms = self.0.rem_euclid(86_400_000);
        // Howard Hinnant's civil-from-days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let (hour, minute, second, milli) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
        write!(f, "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")?;
        if milli != 0 {
            let fraction = format!("{milli:03}");
            write!(f, ".{}", fraction.trim_end_matches('0'))?;
        }
        f.write_str("+00:00")
    }
}

impl Serialize for DateTimeOffset {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The slot and source-kind names as `&'static str`: the known ones are matched, any other (a new slot in the game
/// data) is leaked once, so contributions and events carry them without allocating.
pub fn intern(text: &str) -> &'static str {
    const KNOWN: [&str; 16] = [
        "unknown", "autoAttack", "ability", "passive", "mainhand", "offhand", "head", "chest", "armor", "shoes", "bag", "cape", "mount", "potion", "food", "",
    ];
    if let Some(known) = KNOWN.iter().find(|known| **known == text) {
        return known;
    }
    static OTHERS: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut others = OTHERS.lock().unwrap();
    if let Some(other) = others.iter().find(|other| **other == text) {
        return other;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    others.push(leaked);
    leaked
}

/// `string.Length`: UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `string.IsNullOrWhiteSpace`.
pub fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reuse<const LINEAR: usize>() -> Vec<&'static str> {
        let mut map: CsMap<&str, i32, LINEAR> = CsMap::new();
        for key in ["a", "b", "c", "d"] {
            map.insert(key, 0);
        }
        map.remove("b");
        map.remove("d");
        map.insert("e", 0);
        map.insert_new("f", 0);
        map.insert("g", 0);
        assert!(map.contains_key("f") && !map.contains_key("b") && map.len() == 5);
        map.keys().copied().collect()
    }

    #[test]
    fn removed_slots_are_reused_like_dotnet_with_and_without_an_index() {
        assert_eq!(reuse::<16>(), ["a", "f", "c", "e", "g"]);
        assert_eq!(reuse::<2>(), ["a", "f", "c", "e", "g"]);
    }

    #[test]
    fn guid_matches_dotnet_layout() {
        let guid = Guid([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(guid.to_string(), "04030201-0605-0807-090a-0b0c0d0e0f10");
        assert_eq!(Guid::parse(&guid.to_string()), Some(guid));
        assert_eq!(guid.dotnet_hash(), 0x04030201 ^ 0x08070605 ^ 0x0c0b0a09 ^ 0x100f0e0d);
    }

    #[test]
    fn date_time_offset_matches_system_text_json() {
        assert_eq!(DateTimeOffset(0).to_string(), "1970-01-01T00:00:00+00:00");
        assert_eq!(DateTimeOffset(1_714_564_800_120).to_string(), "2024-05-01T12:00:00.12+00:00");
        assert_eq!(DateTimeOffset(1_000).to_string(), "1970-01-01T00:00:01+00:00");
    }
}

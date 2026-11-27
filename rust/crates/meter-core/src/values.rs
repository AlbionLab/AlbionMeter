//! The values of one timeline bucket, by member or contribution id, in .NET dictionary order.
//!
//! A record's damage and healing buckets all hold every member, in member order, so they share one key list and keep
//! only a dense `f64` per member: 8 bytes an entry instead of an id and a value per bucket. Any other map (weapon and
//! item buckets, dungeon totals) is an ordinary [`CsMap`] while its second is current, and is then frozen into an
//! exact slice of (key id, value) pairs, 16 bytes an entry, with the ids from one process-wide table of keys. All
//! forms behave the same: a change that does not fit (a new key, a removal) turns it into a map first, keeping the
//! order.

use crate::cs::CsMap;
use serde::{Serialize, Serializer};
use std::sync::Arc;

pub type Keys = Arc<Vec<Arc<str>>>;

#[derive(Clone, Debug)]
enum Repr {
    /// `keys[i]` maps to `values[i]`, for `i < values.len()`.
    Members { keys: Keys, values: Vec<f64> },
    /// Boxed: most buckets are past and frozen, so the live map's 64 bytes are kept out of every one of them.
    Map(Box<CsMap<Arc<str>, f64, 128>>),
    /// A past bucket, in enumeration order.
    Packed(Box<[(u32, f64)]>),
}

/// The keys of frozen buckets, by id. Contribution keys (`actor:weapon`, `actor:slot:item:kind`) are few and reused
/// by every bucket, so the table stays small.
struct Table {
    ids: std::collections::HashMap<Arc<str>, u32>,
    keys: Vec<Arc<str>>,
}

static TABLE: std::sync::RwLock<Option<Table>> = std::sync::RwLock::new(None);

fn key_id(key: &Arc<str>) -> u32 {
    if let Some(table) = TABLE.read().unwrap().as_ref()
        && let Some(&id) = table.ids.get(key)
    {
        return id;
    }
    let mut table = TABLE.write().unwrap();
    let table = table.get_or_insert_with(|| Table { ids: Default::default(), keys: Vec::new() });
    *table.ids.entry(key.clone()).or_insert_with(|| {
        table.keys.push(key.clone());
        (table.keys.len() - 1) as u32
    })
}

fn key_of(id: u32) -> Arc<str> {
    TABLE.read().unwrap().as_ref().unwrap().keys[id as usize].clone()
}

#[derive(Clone, Debug)]
pub struct Values(Repr);

impl Default for Values {
    fn default() -> Self {
        Values(Repr::Map(Box::default()))
    }
}

pub enum Iter<'a> {
    Members(std::iter::Zip<std::slice::Iter<'a, Arc<str>>, std::slice::Iter<'a, f64>>),
    Map(Box<dyn Iterator<Item = (&'a Arc<str>, &'a f64)> + 'a>),
    Packed(std::slice::Iter<'a, (u32, f64)>),
}

/// Yields owned pairs (an `Arc` clone), since a frozen bucket's keys live in the shared table.
impl Iterator for Iter<'_> {
    type Item = (Arc<str>, f64);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Iter::Members(iter) => iter.next().map(|(k, v)| (k.clone(), *v)),
            Iter::Map(iter) => iter.next().map(|(k, v)| (k.clone(), *v)),
            Iter::Packed(iter) => iter.next().map(|(id, v)| (key_of(*id), *v)),
        }
    }
}

fn same(key: &Arc<str>, wanted: &str) -> bool {
    std::ptr::eq(key.as_ptr(), wanted.as_ptr()) && key.len() == wanted.len() || **key == *wanted
}

impl Values {
    pub fn new() -> Self {
        Self::default()
    }

    /// One zero per member, sharing the record's key list.
    pub fn members(keys: &Keys) -> Self {
        Values(Repr::Members { keys: keys.clone(), values: vec![0.0; keys.len()] })
    }

    /// Appends a member registered after this bucket was made; `keys` is the record's new key list, ending with it.
    pub fn push_member(&mut self, keys: &Keys, value: f64) {
        match &mut self.0 {
            Repr::Members { keys: own, values } if values.len() + 1 == keys.len() && own[..values.len()] == keys[..values.len()] => {
                *own = keys.clone();
                values.push(value);
            }
            _ => self.insert(keys.last().unwrap().clone(), value),
        }
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Members { values, .. } => values.len(),
            Repr::Map(map) => map.len(),
            Repr::Packed(pairs) => pairs.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn position(keys: &Keys, length: usize, key: &str) -> Option<usize> {
        keys[..length].iter().position(|k| same(k, key))
    }

    pub fn get(&self, key: &str) -> Option<&f64> {
        match &self.0 {
            Repr::Members { keys, values } => Self::position(keys, values.len(), key).map(|i| &values[i]),
            Repr::Map(map) => map.get(key),
            Repr::Packed(pairs) => {
                let table = TABLE.read().unwrap();
                let keys = &table.as_ref()?.keys;
                pairs.iter().find(|(id, _)| *keys[*id as usize] == *key).map(|(_, value)| value)
            }
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut f64> {
        if matches!(self.0, Repr::Packed(_)) {
            self.get(key)?;
            return self.map().get_mut(key);
        }
        match &mut self.0 {
            Repr::Members { keys, values } => Self::position(keys, values.len(), key).map(|i| &mut values[i]),
            Repr::Map(map) => map.get_mut(key),
            Repr::Packed(_) => unreachable!(),
        }
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    fn map(&mut self) -> &mut CsMap<Arc<str>, f64, 128> {
        if !matches!(self.0, Repr::Map(_)) {
            let map = self.iter().collect();
            self.0 = Repr::Map(Box::new(map));
        }
        match &mut self.0 {
            Repr::Map(map) => map,
            _ => unreachable!(),
        }
    }

    /// `dictionary[key] = value`.
    pub fn insert(&mut self, key: Arc<str>, value: f64) {
        if let Some(existing) = self.get_mut(&key) {
            *existing = value;
            return;
        }
        self.map().insert(key, value);
    }

    pub fn remove(&mut self, key: &str) -> Option<f64> {
        self.get(key)?;
        self.map().remove(key)
    }

    pub fn clear(&mut self) {
        self.0 = Repr::Map(Box::default());
    }

    pub fn iter(&self) -> Iter<'_> {
        match &self.0 {
            Repr::Members { keys, values } => Iter::Members(keys.iter().zip(values.iter())),
            Repr::Map(map) => Iter::Map(Box::new(map.iter())),
            Repr::Packed(pairs) => Iter::Packed(pairs.iter()),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = Arc<str>> + '_ {
        self.iter().map(|(key, _)| key)
    }

    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.iter().map(|(_, value)| value)
    }

    /// Freezes a bucket whose second is past: dense values lose their spare capacity, a map becomes an exact slice.
    pub fn shrink(&mut self) {
        match &mut self.0 {
            Repr::Members { values, .. } => values.shrink_to_fit(),
            Repr::Map(map) => {
                let pairs: Box<[(u32, f64)]> = map.iter().map(|(key, value)| (key_id(key), *value)).collect();
                self.0 = Repr::Packed(pairs);
            }
            Repr::Packed(_) => {}
        }
    }
}

impl PartialEq for Values {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(key, value)| other.get(&key) == Some(&value))
    }
}

impl FromIterator<(Arc<str>, f64)> for Values {
    fn from_iter<I: IntoIterator<Item = (Arc<str>, f64)>>(items: I) -> Self {
        Values(Repr::Map(Box::new(items.into_iter().collect())))
    }
}

impl Serialize for Values {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dense form enumerates, changes and serializes like the map it stands for.
    #[test]
    fn dense_member_values_behave_like_the_map() {
        let mut keys: Keys = Arc::new(vec!["a".into(), "b".into()]);
        let mut dense = Values::members(&keys);
        let mut map: Values = keys.iter().map(|k| (k.clone(), 0.0)).collect();
        Arc::make_mut(&mut keys).push("c".into());
        dense.push_member(&keys, 0.0);
        map.insert("c".into(), 0.0);
        *dense.get_mut("b").unwrap() += 5.0;
        *map.get_mut("b").unwrap() += 5.0;
        assert!(dense == map && serde_json::to_string(&dense).unwrap() == serde_json::to_string(&map).unwrap());
        for values in [&mut dense, &mut map] {
            values.insert("d".into(), 1.0);
            values.remove("a");
            values.insert("e".into(), 2.0);
            values.shrink();
        }
        assert_eq!(dense.keys().map(|k| k.to_string()).collect::<Vec<_>>(), ["e", "b", "c", "d"]);
        let mut frozen: Values = dense.iter().collect();
        frozen.shrink();
        assert!(frozen == dense && frozen.get("c") == Some(&0.0) && serde_json::to_string(&frozen).unwrap() == serde_json::to_string(&dense).unwrap());
        *frozen.get_mut("b").unwrap() += 1.0;
        frozen.insert("f".into(), 3.0);
        assert_eq!(frozen.keys().map(|k| k.to_string()).collect::<Vec<_>>(), ["e", "b", "c", "d", "f"]);
        assert_eq!(serde_json::to_string(&dense).unwrap(), serde_json::to_string(&map).unwrap());
    }
}

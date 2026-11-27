//! Zone-scoped object bindings and the party list (`PartyObservation.cs`). The C# lock is gone: the engine thread owns it.

use crate::cs::{CsMap, CsSet, Guid, is_blank};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObservationContext {
    pub connection: Guid,
    pub zone_generation: i64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ObservedCharacter {
    pub id: Guid,
    pub name: String,
    pub weapon_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResolvedCharacter {
    pub character: ObservedCharacter,
    pub is_party_member: Option<bool>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartyObservationSnapshot {
    pub context: ObservationContext,
    pub roster_complete: bool,
    pub members: Vec<ObservedCharacter>,
    pub bound_entities: usize,
    pub stale_updates: i64,
    pub binding_conflicts: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PartyError {
    InvalidRoster,
    InvalidObservation,
    CharacterLimit,
}

pub struct PartyObservation {
    characters: CsMap<Guid, ObservedCharacter>,
    bindings: CsMap<i64, (Guid, i64)>,
    objects: CsMap<Guid, (i64, i64)>,
    party: CsSet<Guid>,
    identity_revisions: CsMap<Guid, i64>,
    context: ObservationContext,
    party_revision: i64,
    stale_updates: i64,
    binding_conflicts: i64,
    roster_complete: bool,
}

impl Default for PartyObservation {
    fn default() -> Self {
        Self {
            characters: CsMap::new(),
            bindings: CsMap::new(),
            objects: CsMap::new(),
            party: CsSet::new(),
            identity_revisions: CsMap::new(),
            context: ObservationContext { connection: Guid::new_v4(), zone_generation: 0 },
            party_revision: -1,
            stale_updates: 0,
            binding_conflicts: 0,
            roster_complete: false,
        }
    }
}

impl PartyObservation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn context(&self) -> ObservationContext {
        self.context
    }
    pub fn begin_connection(&mut self) -> ObservationContext {
        self.characters.clear();
        self.bindings.clear();
        self.objects.clear();
        self.party.clear();
        self.identity_revisions.clear();
        self.party_revision = -1;
        self.roster_complete = false;
        self.context = ObservationContext { connection: Guid::new_v4(), zone_generation: 0 };
        self.context
    }
    pub fn enter_zone(&mut self) -> ObservationContext {
        self.bindings.clear();
        self.objects.clear();
        self.identity_revisions.clear();
        self.context.zone_generation += 1;
        self.context
    }
    pub fn replace_party(&mut self, context: ObservationContext, revision: i64, members: Vec<ObservedCharacter>) -> Result<bool, PartyError> {
        let values: Vec<ObservedCharacter> = members.into_iter().take(101).collect();
        let distinct: HashSet<Guid> = values.iter().map(|v| v.id).collect();
        if values.len() > 100 || values.iter().any(|v| v.id.is_empty() || is_blank(&v.name)) || distinct.len() != values.len() {
            return Err(PartyError::InvalidRoster);
        }
        if context != self.context || revision <= self.party_revision {
            self.stale_updates += 1;
            return Ok(false);
        }
        if self.characters.len() + values.iter().filter(|v| !self.characters.contains_key(&v.id)).count() > 4096 {
            return Err(PartyError::CharacterLimit);
        }
        for value in &values {
            let updated = match self.characters.get(&value.id) {
                Some(existing) => ObservedCharacter { name: value.name.clone(), ..existing.clone() },
                None => value.clone(),
            };
            self.characters.insert(value.id, updated);
        }
        self.party.clear();
        for value in &values {
            self.party.insert(value.id);
        }
        self.party_revision = revision;
        self.roster_complete = true;
        Ok(true)
    }
    pub fn observe(&mut self, context: ObservationContext, revision: i64, object_id: i64, character: ObservedCharacter) -> Result<bool, PartyError> {
        if character.id.is_empty() || is_blank(&character.name) || revision < 0 {
            return Err(PartyError::InvalidObservation);
        }
        if context != self.context {
            self.stale_updates += 1;
            return Ok(false);
        }
        let binding = self.bindings.get(&object_id).copied();
        let previous = self.objects.get(&character.id).copied();
        if binding.is_some_and(|b| revision <= b.1) || self.identity_revisions.get(&character.id).is_some_and(|&last| revision <= last) {
            self.stale_updates += 1;
            return Ok(false);
        }
        if !self.characters.contains_key(&character.id) && self.characters.len() >= 4096 {
            return Err(PartyError::CharacterLimit);
        }
        if let Some(binding) = binding
            && binding.0 != character.id
        {
            self.objects.remove(&binding.0);
            self.binding_conflicts += 1;
        }
        if let Some(previous) = previous
            && previous.0 != object_id
        {
            self.bindings.remove(&previous.0);
        }
        let id = character.id;
        self.characters.insert(id, character);
        self.identity_revisions.insert(id, revision);
        self.bindings.insert(object_id, (id, revision));
        self.objects.insert(id, (object_id, revision));
        Ok(true)
    }
    pub fn resolve(&self, context: ObservationContext, object_id: i64) -> Option<ResolvedCharacter> {
        if context != self.context {
            return None;
        }
        let binding = self.bindings.get(&object_id)?;
        Some(ResolvedCharacter {
            character: self.characters.get(&binding.0).unwrap().clone(),
            is_party_member: self.roster_complete.then(|| self.party.contains(&binding.0)),
        })
    }
    pub fn snapshot(&self) -> PartyObservationSnapshot {
        PartyObservationSnapshot {
            context: self.context,
            roster_complete: self.roster_complete,
            members: self.party.iter().map(|id| self.characters.get(id).unwrap().clone()).collect(),
            bound_entities: self.bindings.len(),
            stale_updates: self.stale_updates,
            binding_conflicts: self.binding_conflicts,
        }
    }
}

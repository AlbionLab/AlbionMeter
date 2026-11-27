//! Game-state observations built from decoded Photon messages (`Observation/*`).

pub mod combat;
pub mod dungeon;
pub mod local;
pub mod party;
pub mod roster;

use crate::capture::photon::Identity;
use crate::engine::EventId;
use std::fmt;

/// The transport identity of a message, the root of its combat event ids. Captures carry a Photon command identity;
/// tests and callers that build messages by hand name them with text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MessageId {
    Command(Identity),
    Text(String),
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageId::Command(identity) => identity.fmt(f),
            MessageId::Text(text) => f.write_str(text),
        }
    }
}

impl MessageId {
    /// The id of the `index`-th effect of this message (`{identity}:{index}` in C#).
    pub fn event(&self, index: u32) -> EventId {
        match self {
            MessageId::Command(identity) => EventId::Command {
                connection: identity.connection.clone(),
                channel: identity.channel,
                kind: identity.kind,
                number: identity.number,
                extra: identity.extra,
                index,
            },
            MessageId::Text(text) => EventId::Text(format!("{text}:{index}").into()),
        }
    }
}

impl From<&str> for MessageId {
    fn from(text: &str) -> Self {
        MessageId::Text(text.into())
    }
}

impl From<String> for MessageId {
    fn from(text: String) -> Self {
        MessageId::Text(text)
    }
}

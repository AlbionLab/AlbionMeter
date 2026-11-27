//! The meter core: packet decoding, the combat engine, game-state observations and dungeon runs, ported from
//! `src/AlbionMeter.Core` with identical numbers and JSON. See `rust/README.md` for the architecture.
//!
//! Feed frames with [`CaptureAnalysis::add`]; read the C#-shaped report with [`CaptureAnalysis::snapshot`], or the
//! incremental view a UI should poll with [`CaptureAnalysis::combat_delta`] (see [`engine`] for the delta contract).
//!
//! Real time: the decode path (frame → IP → Photon → Protocol18 → observations) allocates nothing per packet once warm
//! (`tests/allocations.rs` proves it with a counting allocator); a fight allocates only what its record keeps (event
//! ids, one timeline bucket per second). `cargo bench -p meter-core` reports packets/s and per-packet percentiles.

pub mod capture;
pub mod catalog;
pub mod cs;
pub mod dungeon;
pub mod engine;
pub mod importer;
pub mod maps;
pub mod observation;
pub mod record_controls;
pub mod values;
#[doc(hidden)]
pub mod testkit;

pub use capture::analysis::{CaptureAnalysis, CaptureReport, Identities, RecordError, RecordState};
pub use catalog::CombatSourceCatalog;
pub use engine::{CombatChanges, CombatDelta, CombatEngine, CombatSnapshot};
pub use meter_capture::CaptureFrame;

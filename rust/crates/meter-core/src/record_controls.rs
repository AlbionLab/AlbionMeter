//! The debug journal of Play/Pause/New-record presses and how a replay applies them (`RecordControls.cs`).

use crate::CaptureAnalysis;
use crate::cs::Guid;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordControl {
    pub timestamp_microseconds: i64,
    pub action: String,
    pub character_id: Option<Guid>,
}

impl RecordControl {
    pub fn new(timestamp: i64, action: &str, character_id: Option<Guid>) -> Self {
        Self { timestamp_microseconds: timestamp, action: action.into(), character_id }
    }
}

pub struct RecordControls {
    directory: PathBuf,
    path: PathBuf,
}

impl RecordControls {
    pub fn new(directory: impl AsRef<Path>) -> Self {
        let directory = directory.as_ref().to_path_buf();
        Self { path: directory.join("record-controls.jsonl"), directory }
    }

    pub fn append(&self, control: &RecordControl) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        if fs::metadata(&self.path).is_ok_and(|m| m.len() > 5_000_000) {
            return Err(io::Error::other("The record control journal is full."));
        }
        let mut file = fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(control)?)
    }

    pub fn read(&self) -> io::Result<Vec<RecordControl>> {
        let Ok(metadata) = fs::metadata(&self.path) else { return Ok(Vec::new()) };
        if metadata.len() > 5_000_000 {
            return Err(io::Error::other("The record control journal is too large."));
        }
        let mut result = Vec::new();
        for line in fs::read_to_string(&self.path)?.lines() {
            if line.len() > 1000 || result.len() >= 10_000 {
                return Err(io::Error::other("The record control journal limit was reached."));
            }
            if let Ok(value) = serde_json::from_str::<RecordControl>(line)
                && value.timestamp_microseconds > 0
                && matches!(value.action.as_str(), "pause" | "resume" | "new-record")
            {
                result.push(value);
            }
        }
        result.sort_by_key(|control| control.timestamp_microseconds);
        Ok(result)
    }

    pub fn apply(analysis: &mut CaptureAnalysis, control: &RecordControl) {
        if let Some(id) = control.character_id
            && analysis.character().map(|c| c.character_id) != Some(id)
        {
            return;
        }
        if control.action == "new-record" {
            analysis.begin_record(Some(control.timestamp_microseconds));
        } else {
            let _ = analysis.set_paused(control.action == "pause", control.timestamp_microseconds);
        }
    }
}

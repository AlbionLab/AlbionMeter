#![allow(dead_code)]

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A directory removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("meter-store-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A combat snapshot shaped like the C# `CombatSnapshot` JSON, with `damage` dealt by one member at second 0.
pub fn snapshot(id: &str, started_at: &str, damage: f64) -> Value {
    json!({
        "id": id, "startedAt": started_at, "elapsedSeconds": 1, "status": "running", "mode": "live", "sequence": 1,
        "eventCount": if damage > 0.0 { 1 } else { 0 }, "totalDamage": damage, "totalHealing": 0,
        "members": [{"id": "a", "name": "Alpha", "weapon": "Sword", "weaponFamily": "sword", "color": "#ffffff", "damage": damage, "healing": 0,
            "encounterDps": damage, "encounterHps": 0, "rollingDps": 0, "rollingHps": 0, "damageShare": 100, "healingShare": 0, "membership": "active"}],
        "timeline": [{"second": 0, "damage": {"a": damage}, "healing": {}}],
        "full": true, "location": null
    })
}

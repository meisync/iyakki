//! Counters of the experimental wake phrase (F-VOICE-46): how often the spotter heard it, how often the second-stage check said no, and what the
//! person marked as a false wake or a miss. Numbers only: no sound, no text and no time of day is kept; the file is `wake-stats.json` in the
//! data folder and belongs to the person's account.
//!
//! This file is also the FIRST real data migration that runs through the two-phase library (F-CORE-31, `twophase`). The first layout (schema 1)
//! counted `wakes` and `false_wakes`; schema 2 renames `wakes` to `heard` and adds `rejected` and `misses`. The new layout is STAGED next to the
//! old file at the first start of the new version, read from there until the start is confirmed and the grace period is over, committed after
//! that, and rolled back when the old version starts again: the old version always finds the file it knows.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::twophase;

pub const FILE: &str = "wake-stats.json";
/// The name of the migration in `migrations/`.
pub const MIGRATION: &str = "wake-stats-2";
/// How long the new layout must have been running well before the old file is put aside (seconds).
pub const GRACE_SECS: i64 = 3 * 24 * 3600;

fn v1_to_v2(v: &mut Value) {
    if let Value::Object(o) = v {
        if let Some(w) = o.remove("wakes") {
            o.insert("heard".into(), w);
        }
        o.entry("rejected").or_insert(json!(0));
        o.entry("misses").or_insert(json!(0));
    }
}

pub const SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec {
    name: "wake phrase counters",
    current: 2,
    migrations: &[iyakki_schema::Migration { to: 1, what: "first layout: wakes and false wakes", run: |_| {} }, iyakki_schema::Migration { to: 2, what: "wakes become heard; rejected and misses are added", run: v1_to_v2 }],
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub schema: u32,
    /// Times the spotter heard the phrase.
    #[serde(default)]
    pub heard: u64,
    /// Times the second-stage check heard no such words and the wake was dropped.
    #[serde(default)]
    pub rejected: u64,
    /// Wakes the person marked as wrong (nobody had said it).
    #[serde(default)]
    pub false_wakes: u64,
    /// Times the person marked that they said it and nothing happened.
    #[serde(default)]
    pub misses: u64,
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    Heard,
    Rejected,
    FalseWake,
    Miss,
}

impl Count {
    pub fn parse(s: &str) -> Option<Count> {
        match s {
            "false_wake" => Some(Count::FalseWake),
            "miss" => Some(Count::Miss),
            _ => None,
        }
    }
}

/// The file a running version reads and writes: the staged new layout while a migration for it is waiting, the file itself otherwise.
fn path(dir: &Path, running: &str) -> PathBuf {
    twophase::path_to_read(dir, MIGRATION, FILE, running)
}

pub fn load(dir: &Path) -> Stats {
    load_as(dir, crate::relinfo::VERSION)
}

pub fn load_as(dir: &Path, running: &str) -> Stats {
    let (_, s, _) = iyakki_schema::load_typed::<Stats>(&SCHEMA, &path(dir, running), iyakki_schema::now_secs());
    s
}

/// Add one to a counter (a file written by a newer release is read but not changed).
pub fn bump(dir: &Path, what: Count) -> Result<Stats, String> {
    bump_as(dir, crate::relinfo::VERSION, what)
}

pub fn bump_as(dir: &Path, running: &str, what: Count) -> Result<Stats, String> {
    let p = path(dir, running);
    let (out, mut s, _) = iyakki_schema::load_typed::<Stats>(&SCHEMA, &p, iyakki_schema::now_secs());
    if let iyakki_schema::Outcome::Newer { found, .. } = &out {
        return Err(iyakki_schema::newer_message(&SCHEMA, *found));
    }
    match what {
        Count::Heard => s.heard += 1,
        Count::Rejected => s.rejected += 1,
        Count::FalseWake => s.false_wakes += 1,
        Count::Miss => s.misses += 1,
    }
    s.schema = SCHEMA.current;
    iyakki_schema::save_typed(&SCHEMA, &p, &s, 0o600)?;
    Ok(s)
}

/// One plain sentence for the tile and the chat tool.
pub fn summary(s: &Stats) -> String {
    if s.heard == 0 && s.false_wakes == 0 && s.misses == 0 && s.rejected == 0 {
        return "No wake phrase counts yet.".into();
    }
    format!("Wake phrase counts: heard {}, dropped by the second check {}, marked as wrong {}, marked as missed {}.", s.heard, s.rejected, s.false_wakes, s.misses)
}

impl crate::services::Services {
    /// The wake phrase counters in one sentence (the tile and Settings show it).
    pub fn wake_stats_line(&self) -> String {
        summary(&load(self.dir()))
    }

    /// The person reports a mistake: `false_wake` or `miss`. Only a number goes up.
    pub fn wake_mark(&self, kind: &str) -> Result<(), String> {
        let k = Count::parse(kind).ok_or_else(|| "the mark is false_wake or miss".to_string())?;
        bump(self.dir(), k).map(|_| ())
    }
}

// ------------------------------------------------------------------------------------------------ the migration (F-CORE-31)

/// Convert the old file's bytes to the schema-2 layout. An empty (missing) old file stays empty.
pub fn convert(_file: &str, old: &[u8]) -> Result<Vec<u8>, String> {
    if old.is_empty() {
        return Ok(vec![]);
    }
    let mut v: Value = serde_json::from_slice(old).map_err(|e| format!("the counters file is not readable: {e}"))?;
    iyakki_schema::migrate(&SCHEMA, &mut v)?;
    serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())
}

/// At a start of version `running`: when a counters file of the old layout is there, stage the new layout next to it (phase 1). The old file is not
/// touched. Returns the migration's state when one was staged now or earlier.
pub fn stage_if_needed(dir: &Path, running: &str, from_version: &str, now: i64) -> Option<twophase::State> {
    if let Some(s) = twophase::load(dir, MIGRATION) {
        return Some(s);
    }
    let old = std::fs::read(dir.join(FILE)).ok()?;
    let v: Value = serde_json::from_slice(&old).ok()?;
    if iyakki_schema::version_of(&v) >= SCHEMA.current {
        return None;
    }
    twophase::stage(dir, MIGRATION, from_version, running, &[FILE], now, &convert).ok()
}

/// Phase 2 for a confirmed migration whose grace period is over; anything else changes nothing. True when the new layout took the old one's place.
pub fn commit_when_due(dir: &Path, now: i64, grace_secs: i64) -> bool {
    twophase::load(dir, MIGRATION).is_some_and(|s| s.phase == twophase::Phase::Confirmed) && twophase::commit(dir, MIGRATION, now, grace_secs).is_ok()
}

/// What the boot does about the counters, in order: stage (phase 1), then commit what is due. `reconcile` runs separately, after the start is confirmed.
pub fn boot_step(dir: &Path, running: &str, from_version: &str, now: i64) {
    let _ = stage_if_needed(dir, running, from_version, now);
    let _ = commit_when_due(dir, now, GRACE_SECS);
}

#[cfg(test)]
#[path = "wakestats_tests.rs"]
mod tests;

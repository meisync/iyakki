//! The dashboards every account starts with. Today one: "Local cluster (k3s)" on a `kube` source, so the cluster of this machine can be
//! watched without any setup. Seeding happens ONCE per account: a marker file (`<data>/dash/seeded-<user>.json`) records what was
//! seeded, and it is written after the files are, so a person who edits the dashboard keeps the edits and a person who deletes it (or
//! the source) does not get it back. An existing source or dashboard with the same id is never overwritten. Only administrators are
//! seeded: the dashboard is private to its owner, who may share it (`dash/share.rs`).
//!
//! To get it back: DASH > Dashboards > "Local cluster (k3s)" under the templates makes a fresh copy, or [`reset_default`] (and
//! deleting the marker file, see `docs/DASHBOARD.md`) seeds it again.

use std::path::{Path, PathBuf};

use super::connector::SourceConfig;
use super::store::{Dashboards, Sources};

pub const SOURCE_ID: &str = "kube-local";
pub const DASHBOARD_ID: &str = "k3s-local";
/// The name in the marker file; a later default dashboard gets a new name and is seeded on its own.
const MARK: &str = "k3s-local";

fn marker(data_dir: &Path, user: &str) -> PathBuf {
    data_dir.join("dash").join(format!("seeded-{user}.json"))
}

fn marked(data_dir: &Path, user: &str) -> Vec<String> {
    std::fs::read_to_string(marker(data_dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.get("seeded").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default()
}

fn write_marks(data_dir: &Path, user: &str, marks: &[String]) -> Result<(), String> {
    let p = marker(data_dir, user);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    iyakki_schema::write_atomic(&p, serde_json::json!({"seeded": marks}).to_string().as_bytes(), 0o600)
}

pub fn default_source() -> SourceConfig {
    let mut c = SourceConfig { id: SOURCE_ID.into(), kind: "kube".into(), name: "Local cluster (k3s)".into(), ..Default::default() };
    c.values.insert("cluster".into(), "local".into());
    c
}

/// Save the default source and dashboard unless this account was seeded before. Returns what was created (empty when nothing was
/// needed). A failure leaves the marker unwritten, so the next call tries again.
pub fn seed_defaults(data_dir: &Path, home: &Path, user: &str, admin: bool) -> Result<Vec<String>, String> {
    // the cluster is the administrators' own: anyone else gets nothing, and is not marked, so an account made an administrator later is seeded then
    if !admin {
        return Ok(vec![]);
    }
    let mut marks = marked(data_dir, user);
    if marks.iter().any(|m| m == MARK) {
        return Ok(vec![]);
    }
    let mut made = vec![];
    let sources = Sources::open(data_dir, user);
    if sources.get(SOURCE_ID).is_none() {
        sources.save(default_source())?;
        made.push(format!("source {SOURCE_ID}"));
    }
    let boards = Dashboards::open(home);
    if boards.find(DASHBOARD_ID).is_none() && !boards.dir().join(format!("{DASHBOARD_ID}.json")).exists() {
        let t = super::templates::find(DASHBOARD_ID).ok_or("the default dashboard is missing from this build")?;
        boards.save(&t.instantiate(SOURCE_ID, DASHBOARD_ID))?;
        made.push(format!("dashboard {DASHBOARD_ID}"));
    }
    marks.push(MARK.into());
    write_marks(data_dir, user, &marks)?;
    Ok(made)
}

/// Put the default back as shipped: the source and the dashboard are rewritten (the person's edits to them are lost), then it is seeded
/// state again. Only an explicit request calls this.
pub fn reset_default(data_dir: &Path, home: &Path, user: &str) -> Result<(), String> {
    let marks: Vec<String> = marked(data_dir, user).into_iter().filter(|m| m != MARK).collect();
    write_marks(data_dir, user, &marks)?;
    let t = super::templates::find(DASHBOARD_ID).ok_or("the default dashboard is missing from this build")?;
    Sources::open(data_dir, user).save(default_source())?;
    Dashboards::open(home).save(&t.instantiate(SOURCE_ID, DASHBOARD_ID))?;
    let mut marks = marks;
    marks.push(MARK.into());
    write_marks(data_dir, user, &marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(tag: &str) -> (PathBuf, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-dash-seed-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::create_dir_all(d.join("home")).unwrap();
        (d.join("data"), d.join("home"))
    }

    #[test]
    fn a_new_account_gets_the_source_and_the_dashboard_once() {
        let (data, home) = dirs("first");
        assert_eq!(seed_defaults(&data, &home, "amy", true).unwrap().len(), 2);
        let d = Dashboards::open(&home).load(DASHBOARD_ID).unwrap();
        assert_eq!(d.title, "Local cluster (k3s)");
        assert_eq!(d.refresh_s, 30);
        assert!(d.panels.iter().all(|p| p.source == SOURCE_ID));
        let s = Sources::open(&data, "amy").get(SOURCE_ID).unwrap();
        assert_eq!(s.kind, "kube");
        assert!(s.secrets.is_empty());
        // the second call does nothing
        assert!(seed_defaults(&data, &home, "amy", true).unwrap().is_empty());
        // another account is seeded on its own
        let bobs_home = home.parent().unwrap().join("home-bob");
        assert_eq!(seed_defaults(&data, &bobs_home, "bob", true).unwrap().len(), 2);
    }

    #[test]
    fn a_dashboard_the_person_edited_is_not_overwritten() {
        let (data, home) = dirs("edited");
        seed_defaults(&data, &home, "amy", true).unwrap();
        let boards = Dashboards::open(&home);
        let mut d = boards.load(DASHBOARD_ID).unwrap();
        d.title = "My cluster".into();
        d.remove_panel("events");
        boards.save(&d).unwrap();
        assert!(seed_defaults(&data, &home, "amy", true).unwrap().is_empty());
        let back = boards.load(DASHBOARD_ID).unwrap();
        assert_eq!(back.title, "My cluster");
        assert!(back.panel("events").is_none());
    }

    #[test]
    fn a_dashboard_the_person_deleted_does_not_come_back() {
        let (data, home) = dirs("deleted");
        seed_defaults(&data, &home, "amy", true).unwrap();
        let boards = Dashboards::open(&home);
        assert!(boards.delete(DASHBOARD_ID).unwrap());
        Sources::open(&data, "amy").remove(SOURCE_ID).unwrap();
        for _ in 0..3 {
            assert!(seed_defaults(&data, &home, "amy", true).unwrap().is_empty());
        }
        assert!(boards.find(DASHBOARD_ID).is_none());
        assert!(Sources::open(&data, "amy").get(SOURCE_ID).is_none());
    }

    #[test]
    fn an_existing_dashboard_or_source_with_that_id_is_kept() {
        let (data, home) = dirs("existing");
        let boards = Dashboards::open(&home);
        let mut mine = super::super::model::Dashboard::new(DASHBOARD_ID, "Mine");
        mine.refresh_s = 5;
        boards.save(&mine).unwrap();
        let mut src = default_source();
        src.name = "My kube".into();
        Sources::open(&data, "amy").save(src).unwrap();
        assert!(seed_defaults(&data, &home, "amy", true).unwrap().is_empty());
        assert_eq!(boards.load(DASHBOARD_ID).unwrap().title, "Mine");
        assert_eq!(Sources::open(&data, "amy").get(SOURCE_ID).unwrap().name, "My kube");
    }

    #[test]
    fn a_failed_save_is_tried_again_and_reset_restores_the_shipped_dashboard() {
        let (data, home) = dirs("reset");
        // the home folder is a file: nothing can be saved, nothing is marked
        let blocked = home.join("blocked");
        std::fs::write(&blocked, "x").unwrap();
        assert!(seed_defaults(&data, &blocked, "amy", true).is_err());
        assert!(marked(&data, "amy").is_empty());
        seed_defaults(&data, &home, "amy", true).unwrap();
        let boards = Dashboards::open(&home);
        let mut d = boards.load(DASHBOARD_ID).unwrap();
        d.panels.clear();
        boards.save(&d).unwrap();
        reset_default(&data, &home, "amy").unwrap();
        assert_eq!(boards.load(DASHBOARD_ID).unwrap().panels.len(), 20);
        assert!(seed_defaults(&data, &home, "amy", true).unwrap().is_empty());
    }

    #[test]
    fn a_broken_marker_file_means_not_seeded_yet_and_never_a_panic() {
        let (data, home) = dirs("broken");
        std::fs::create_dir_all(data.join("dash")).unwrap();
        std::fs::write(marker(&data, "amy"), "{not json").unwrap();
        assert_eq!(seed_defaults(&data, &home, "amy", true).unwrap().len(), 2);
    }
}

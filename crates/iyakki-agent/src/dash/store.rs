//! Saving. Sources (no secrets, only vault references) live in the system data folder, one file per person; dashboards are
//! JSON files in the person's own `Dashboards` folder so they can be copied, shared and exported like any other file.

use std::path::{Path, PathBuf};

use super::connector::{sanitize_imported_source, valid_id, SourceConfig};
use super::model::{Dashboard, MAX_PANELS};

pub const MAX_FILE_BYTES: u64 = 2 << 20;
pub const MAX_SOURCES: usize = 40;

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

// ------------------------------------------------------------------------------------------------------ sources

pub struct Sources {
    file: PathBuf,
}

impl Sources {
    pub fn open(data_dir: &Path, user: &str) -> Sources {
        Sources { file: data_dir.join("dash").join(format!("sources-{user}.json")) }
    }

    pub fn list(&self) -> Vec<SourceConfig> {
        std::fs::read_to_string(&self.file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<SourceConfig> {
        self.list().into_iter().find(|s| s.id == id || s.name.eq_ignore_ascii_case(id))
    }

    /// Add or replace (by id). The config is validated against its kind first.
    pub fn save(&self, cfg: SourceConfig) -> Result<(), String> {
        let info = super::registry::kind(&cfg.kind).ok_or_else(|| format!("unknown kind `{}`", cfg.kind))?;
        let problems = super::connector::validate(&cfg, &info);
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let mut all = self.list();
        match all.iter().position(|s| s.id == cfg.id) {
            Some(i) => all[i] = cfg,
            None if all.len() >= MAX_SOURCES => return Err(format!("at most {MAX_SOURCES} sources")),
            None => all.push(cfg),
        }
        write_atomic(&self.file, serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?.as_bytes())
    }

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut all = self.list();
        let n = all.len();
        all.retain(|s| s.id != id);
        if all.len() == n {
            return Ok(false);
        }
        write_atomic(&self.file, serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?.as_bytes())?;
        Ok(true)
    }

    /// An id for a new source of `kind`: `prometheus`, `prometheus-2`, ...
    pub fn new_id(&self, kind: &str) -> String {
        let all = self.list();
        (1..).map(|i| if i == 1 { kind.to_string() } else { format!("{kind}-{i}") }).find(|id| !all.iter().any(|s| &s.id == id)).unwrap()
    }
}

// ------------------------------------------------------------------------------------------------------ dashboards

pub struct Dashboards {
    dir: PathBuf,
}

/// The folder a person's dashboards are kept in.
pub fn dashboards_dir(home: &Path) -> PathBuf {
    home.join("Dashboards")
}

/// `My CPU board!` becomes `my-cpu-board`.
pub fn slug(title: &str) -> String {
    let mut s: String = title.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let s: String = s.chars().take(40).collect();
    if s.is_empty() { "dashboard".into() } else { s }
}

/// What reading a bundle gives.
#[derive(Clone, Debug)]
pub struct Imported {
    pub dashboard: Dashboard,
    pub sources: Vec<SourceConfig>,
    pub warnings: Vec<String>,
    /// (source id, field label) that still need a vault entry.
    pub needs_entry: Vec<(String, String)>,
}

/// Replace anything that looks like a credential in the texts of a dashboard (queries, targets, legends, notes, expressions).
/// Returns a sentence for each.
pub fn scrub_dashboard(d: &mut Dashboard) -> Vec<String> {
    let mut notes = vec![];
    let mut fix = |t: &mut String, what: &str| {
        if let Some(f) = crate::credguard::detect(t) {
            *t = f.redacted;
            notes.push(format!("{what}: a {} was removed", f.what));
        }
    };
    for p in &mut d.panels {
        let title = p.title.clone();
        fix(&mut p.query.text, &format!("panel “{title}” query A"));
        fix(&mut p.options.text, &format!("panel “{title}” text"));
        fix(&mut p.options.legend_format, &format!("panel “{title}” legend"));
        for (i, q) in p.queries.iter_mut().enumerate() {
            fix(&mut q.query.text, &format!("panel “{title}” query {}", super::model::query_letter(i + 1)));
        }
        for e in &mut p.expressions {
            fix(e, &format!("panel “{title}” expression"));
        }
    }
    for v in &mut d.variables {
        if let super::model::VarKind::Query { query, .. } = &mut v.kind {
            fix(query, &format!("variable {}", v.name));
        }
    }
    notes
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub panels: usize,
}

impl Dashboards {
    pub fn open(home: &Path) -> Dashboards {
        Dashboards { dir: dashboards_dir(home) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(format!("`{id}` is not a valid dashboard id"));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn list(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json") && e.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES))
            .filter_map(|e| {
                let t = std::fs::read_to_string(e.path()).ok()?;
                let d: Dashboard = serde_json::from_str(&t).ok()?;
                Some(Entry { id: e.path().file_stem()?.to_string_lossy().into_owned(), title: d.title, panels: d.panels.len() })
            })
            .collect();
        v.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        v
    }

    pub fn load(&self, id: &str) -> Result<Dashboard, String> {
        let p = self.path(id)?;
        let meta = std::fs::metadata(&p).map_err(|_| format!("no dashboard `{id}`"))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err("that dashboard file is too large".into());
        }
        let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut d: Dashboard = serde_json::from_str(&t).map_err(|e| format!("{}: not a dashboard ({e})", p.display()))?;
        d.id = id.to_string();
        d.sanitize();
        Ok(d)
    }

    /// Find by id or by title (the assistant says "the checkout dashboard").
    pub fn find(&self, name: &str) -> Option<Dashboard> {
        let n = name.trim();
        self.load(n).ok().or_else(|| self.list().into_iter().find(|e| e.title.eq_ignore_ascii_case(n)).and_then(|e| self.load(&e.id).ok()))
    }

    pub fn save(&self, d: &Dashboard) -> Result<(), String> {
        let p = self.path(&d.id)?;
        if d.panels.len() > MAX_PANELS {
            return Err(format!("too many panels (max {MAX_PANELS})"));
        }
        // a credential pasted into a query is never written to a file
        let mut clean = d.clone();
        scrub_dashboard(&mut clean);
        write_atomic(&p, serde_json::to_string_pretty(&clean).map_err(|e| e.to_string())?.as_bytes())
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let p = self.path(id)?;
        if !p.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// A free id for a title.
    pub fn new_id(&self, title: &str) -> String {
        let base = slug(title);
        (1..).map(|i| if i == 1 { base.clone() } else { format!("{base}-{i}") }).find(|id| self.path(id).is_ok_and(|p| !p.exists())).unwrap()
    }

    /// Read a dashboard from JSON text (import): repaired, given a fresh id, never trusted. Secrets cannot be in it: the model
    /// holds only source ids.
    pub fn import(&self, text: &str) -> Result<(Dashboard, Vec<String>), String> {
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err("that file is too large to be a dashboard".into());
        }
        let mut d: Dashboard = serde_json::from_str(text).map_err(|e| format!("this is not a dashboard file: {e}"))?;
        let mut notes = d.sanitize();
        notes.extend(scrub_dashboard(&mut d));
        d.id = self.new_id(&d.title);
        Ok((d, notes))
    }

    /// The dashboard as JSON for a file. It holds source ids and queries only, and a query that looks like it contains a
    /// credential is cleaned first.
    pub fn export_text(d: &Dashboard) -> String {
        let mut clean = d.clone();
        scrub_dashboard(&mut clean);
        serde_json::to_string_pretty(&clean).unwrap_or_default()
    }

    /// The dashboard together with the sources it uses (addresses and settings, and for secrets only the *names* of the vault entries
    /// as placeholders), so another person can set it up. Never a secret value.
    pub fn export_bundle(d: &Dashboard, sources: &[SourceConfig]) -> String {
        let used: Vec<String> = {
            let mut v: Vec<String> = vec![d.default_source.clone()];
            for p in &d.panels {
                v.extend(p.sources(&d.default_source));
            }
            for var in &d.variables {
                if let super::model::VarKind::Query { source, .. } = &var.kind {
                    v.push(source.clone());
                }
                v.extend(var.mapping.keys().cloned());
            }
            v.retain(|s| !s.is_empty());
            v.sort();
            v.dedup();
            v
        };
        let mut srcs: Vec<SourceConfig> = sources.iter().filter(|s| used.contains(&s.id)).cloned().collect();
        for s in &mut srcs {
            if let Some(info) = super::registry::kind(&s.kind) {
                // the export goes through the same cleaning as an import
                *s = sanitize_imported_source(s.clone(), &info).0;
            }
        }
        let mut clean = d.clone();
        scrub_dashboard(&mut clean);
        serde_json::to_string_pretty(&serde_json::json!({"iyakki_dashboard_bundle": 1, "dashboard": clean, "sources": srcs})).unwrap_or_default()
    }

    /// Read a bundle (or a plain dashboard file). Sources come back cleaned; a vault reference is kept only when the importer has an entry of
    /// that name, otherwise it is dropped and listed in `needs_entry` (source id, field label) so the person picks one.
    pub fn import_bundle(&self, text: &str, vault_names: &[String]) -> Result<Imported, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("this is not a dashboard file: {e}"))?;
        if v.get("iyakki_dashboard_bundle").is_none() {
            let (dashboard, warnings) = self.import(text)?;
            return Ok(Imported { dashboard, sources: vec![], warnings, needs_entry: vec![] });
        }
        let dtext = v.get("dashboard").map(|d| d.to_string()).ok_or("the bundle has no dashboard")?;
        let (dashboard, mut warnings) = self.import(&dtext)?;
        let mut sources = vec![];
        let mut needs = vec![];
        for sv in v.get("sources").and_then(|s| s.as_array()).into_iter().flatten().take(MAX_SOURCES) {
            let Ok(cfg) = serde_json::from_value::<SourceConfig>(sv.clone()) else {
                warnings.push("a source in the file could not be read and was skipped".into());
                continue;
            };
            let Some(info) = super::registry::kind(&cfg.kind) else {
                warnings.push(format!("the source `{}` has an unknown kind and was skipped", cfg.name));
                continue;
            };
            let (mut cfg, notes) = sanitize_imported_source(cfg, &info);
            warnings.extend(notes);
            for f in info.fields.iter().filter(|f| f.kind == super::connector::FieldKind::Secret) {
                let Some(r) = cfg.secrets.get(&f.key).cloned() else { continue };
                let name = r.split_once(':').map_or("", |x| x.1).to_string();
                if !(r.starts_with("uvault:") && vault_names.contains(&name)) {
                    cfg.secrets.remove(&f.key);
                    needs.push((cfg.id.clone(), f.label.clone()));
                    warnings.push(format!("{}: the vault entry `{name}` is not in your vault: pick one for the {}", cfg.name, f.label));
                }
            }
            sources.push(cfg);
        }
        Ok(Imported { dashboard, sources, warnings, needs_entry: needs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::model::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-dash-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dashboards_save_list_load_delete_and_import() {
        let home = tmp("dash");
        let s = Dashboards::open(&home);
        assert!(s.list().is_empty());
        let mut d = Dashboard::new(&s.new_id("My CPU board!"), "My CPU board!");
        assert_eq!(d.id, "my-cpu-board");
        d.add_panel(Panel::new("", "CPU", "prom", Visual::Line, "up", Grid::new(0, 0, 12, 6))).unwrap();
        s.save(&d).unwrap();
        assert_eq!(s.new_id("My CPU board!"), "my-cpu-board-2");
        assert_eq!(s.list(), vec![Entry { id: "my-cpu-board".into(), title: "My CPU board!".into(), panels: 1 }]);
        assert_eq!(s.load("my-cpu-board").unwrap(), d);
        assert_eq!(s.find("my cpu BOARD!").unwrap().id, "my-cpu-board");
        assert!(dashboards_dir(&home).join("my-cpu-board.json").is_file());
        assert!(s.load("../x").is_err() && s.load("nope").is_err());
        let (imp, _) = s.import(&Dashboards::export_text(&d)).unwrap();
        assert_eq!(imp.id, "my-cpu-board-2", "an import never overwrites");
        assert!(s.import("{nope").is_err());
        assert!(s.delete("my-cpu-board").unwrap() && !s.delete("my-cpu-board").unwrap());
        // damaged and oversized files are skipped, not fatal
        std::fs::write(s.dir().join("bad.json"), "{{{").unwrap();
        assert!(s.list().is_empty());
    }

    #[test]
    fn sources_hold_references_only_and_validate() {
        let dir = tmp("src");
        let st = Sources::open(&dir, "amy");
        let mut c = SourceConfig { id: "prom".into(), kind: "prometheus".into(), name: "Prod".into(), ..Default::default() };
        c.values.insert("url".into(), "http://p:9090".into());
        c.values.insert("auth".into(), "bearer".into());
        c.secrets.insert("token".into(), "uvault:svc.prom".into());
        st.save(c.clone()).unwrap();
        let raw = std::fs::read_to_string(dir.join("dash/sources-amy.json")).unwrap();
        assert!(raw.contains("uvault:svc.prom") && !raw.contains("password"));
        let mut bad = c.clone();
        bad.id = "p2".into();
        bad.secrets.insert("token".into(), "hunter2".into());
        assert!(st.save(bad).is_err());
        assert_eq!(st.new_id("prometheus"), "prometheus");
        assert_eq!(st.get("prod").unwrap().id, "prom");
        assert!(st.remove("prom").unwrap() && st.list().is_empty());
        assert!(Sources::open(&dir, "bob").list().is_empty(), "per person");
    }
}

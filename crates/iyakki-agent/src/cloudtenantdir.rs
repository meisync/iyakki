//! The writer of the tenant directory `<data_dir>/cloud/tenants.json` (read by `webservice/tenants.rs`): the home tenant and the other
//! tenants a pod's switcher offers. Until now the file was written by hand; `iyakki cloud tenants add|remove|list|home` writes it with the
//! same validation the reader applies (so an entry written here is never skipped when read), atomically, keeping what is already there.
//! Who belongs to a tenant is never written here: that is the tenant's own `roles.json`.

use crate::cloudtenant::host_name;
use crate::webservice::tenants::{file, valid_id, valid_name};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub host: Option<String>,
    pub data_dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dir {
    pub home: Option<(String, String)>,
    pub tenants: Vec<Tenant>,
}

pub fn load(data_dir: &Path) -> Result<Dir, String> {
    let p = file(data_dir);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Dir::default()),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
    let s = |x: &Value, k: &str| x.get(k).and_then(|y| y.as_str()).map(String::from);
    let home = v.get("home").and_then(|h| Some((s(h, "id")?, s(h, "name")?)));
    let tenants = v.get("tenants").and_then(|t| t.as_array()).map(|a| a.iter().filter_map(|t| Some(Tenant { id: s(t, "id")?, name: s(t, "name")?, host: s(t, "host"), data_dir: s(t, "data_dir")? })).collect()).unwrap_or_default();
    Ok(Dir { home, tenants })
}

fn check_tenant(t: &Tenant) -> Result<(), String> {
    if !valid_id(&t.id) {
        return Err("the id is a DNS label: lower case letters, digits and -, at most 63 characters".into());
    }
    if !valid_name(&t.name) {
        return Err("the name is 1 to 80 characters without control characters".into());
    }
    if !t.data_dir.starts_with('/') || t.data_dir.contains("..") || t.data_dir.chars().any(|c| c.is_control()) {
        return Err("the data folder is an absolute path without ..".into());
    }
    if let Some(h) = &t.host {
        if !host_name(h) {
            return Err("the host is not a valid host name".into());
        }
    }
    Ok(())
}

impl Dir {
    pub fn to_json(&self) -> String {
        let mut v = json!({"tenants": self.tenants.iter().map(|t| {
            let mut o = json!({"id": t.id, "name": t.name, "data_dir": t.data_dir});
            if let Some(h) = &t.host { o["host"] = json!(h); }
            o
        }).collect::<Vec<_>>()});
        if let Some((id, name)) = &self.home {
            v["home"] = json!({"id": id, "name": name});
        }
        serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
    }

    /// Add (or replace by id) one tenant. The id may not be the home tenant's.
    pub fn add(&mut self, t: Tenant) -> Result<(), String> {
        check_tenant(&t)?;
        if self.home.as_ref().is_some_and(|h| h.0 == t.id) {
            return Err("that id is the home tenant's".into());
        }
        if self.tenants.iter().any(|o| o.id != t.id && o.data_dir == t.data_dir) {
            return Err("another tenant already uses that data folder".into());
        }
        self.tenants.retain(|o| o.id != t.id);
        if self.tenants.len() >= 200 {
            return Err("at most 200 tenants are listed".into());
        }
        self.tenants.push(t);
        self.tenants.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let n = self.tenants.len();
        self.tenants.retain(|t| t.id != id);
        if self.tenants.len() == n {
            return Err("no such tenant in the list".into());
        }
        Ok(())
    }

    pub fn set_home(&mut self, id: &str, name: &str) -> Result<(), String> {
        if !valid_id(id) || !valid_name(name) {
            return Err("the home tenant needs a DNS-label id and a name of 1 to 80 characters".into());
        }
        if self.tenants.iter().any(|t| t.id == id) {
            return Err("that id is already listed as another tenant".into());
        }
        self.home = Some((id.into(), name.into()));
        Ok(())
    }
}

/// Write the directory atomically (a reader never sees half a file).
pub fn save(data_dir: &Path, d: &Dir) -> Result<(), String> {
    iyakki_schema::write_atomic(&file(data_dir), d.to_json().as_bytes(), 0o644)
}

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

/// `iyakki cloud tenants <add ID --name NAME --data-dir /ABS [--host HOST] | remove ID | list | home ID --name NAME> --data DIR`
/// (`DIR`: the home tenant's data folder, where `cloud/tenants.json` lives; or `--file` naming the file itself).
pub fn cli(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let usage = "usage: iyakki cloud tenants <add ID --name NAME --data-dir /ABSOLUTE/PATH [--host HOST] | remove ID | list | home ID --name NAME> (--data DIR | --file DIR/cloud/tenants.json)";
    let base: PathBuf = match (opt(a, "--data"), opt(a, "--file")) {
        (Some(d), None) => PathBuf::from(d),
        (None, Some(f)) => {
            // the file is <data>/cloud/tenants.json
            let p = PathBuf::from(f);
            if p.file_name().and_then(|n| n.to_str()) != Some("tenants.json") {
                return Err(format!("--file names tenants.json\n{usage}"));
            }
            p.parent().and_then(|c| c.parent()).map(Path::to_path_buf).ok_or(usage)?
        }
        _ => return Err(usage.into()),
    };
    // positional words: those that are not an option or an option's value
    let mut pos: Vec<&String> = Vec::new();
    let mut skip = false;
    for x in a {
        if skip {
            skip = false;
        } else if x.starts_with("--") {
            skip = true;
        } else {
            pos.push(x);
        }
    }
    let mut dir = load(&base)?;
    match pos.first().map(|s| s.as_str()) {
        Some("list") => {
            match &dir.home {
                Some((id, name)) => out(&format!("home   {id}  {name}")),
                None => out("home   (default)"),
            }
            for t in &dir.tenants {
                out(&format!("tenant {}  {}  {}{}", t.id, t.name, t.data_dir, t.host.as_ref().map(|h| format!("  host {h}")).unwrap_or_default()));
            }
            Ok(())
        }
        Some("add") => {
            let id = pos.get(1).ok_or(usage)?;
            dir.add(Tenant { id: (*id).clone(), name: opt(a, "--name").ok_or("--name is needed")?, host: opt(a, "--host"), data_dir: opt(a, "--data-dir").ok_or("--data-dir is needed")? })?;
            save(&base, &dir)?;
            out(&format!("tenant {id} listed in {}", file(&base).display()));
            Ok(())
        }
        Some("remove") => {
            let id = pos.get(1).ok_or(usage)?;
            dir.remove(id)?;
            save(&base, &dir)?;
            out(&format!("tenant {id} removed from the list (its data folder is untouched)"));
            Ok(())
        }
        Some("home") => {
            let id = pos.get(1).ok_or(usage)?;
            dir.set_home(id, &opt(a, "--name").ok_or("--name is needed")?)?;
            save(&base, &dir)?;
            out(&format!("home tenant is {id}"));
            Ok(())
        }
        _ => Err(usage.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = crate::testdir::unique("cloudtenantdir", tag);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }
    fn run(a: &str) -> (Result<(), String>, Vec<String>) {
        let mut lines = Vec::new();
        let r = cli(&args(a), &mut |l| lines.push(l.to_string()));
        (r, lines)
    }

    #[test]
    fn the_tenant_directory_is_written_validated_and_read_back_by_the_reader() {
        let d = tmp("rw");
        let ds = d.display();
        assert!(run(&format!("home acme --name Acme --data {ds}")).0.is_ok());
        assert!(run(&format!("add beta --name Beta --data-dir /var/lib/iyakki-beta --host beta.example.org --data {ds}")).0.is_ok());
        assert!(run(&format!("add gamma --name Gamma --data-dir /var/lib/iyakki-gamma --data {ds}")).0.is_ok());
        // what the reader of the web service sees
        let dir = crate::webservice::tenants::load(&d);
        assert_eq!(dir.home.id, "acme");
        assert_eq!(dir.others.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["beta", "gamma"]);
        assert_eq!(dir.others[0].host.as_deref(), Some("beta.example.org"));
        // bad input is refused and the file is unchanged
        let before = std::fs::read_to_string(file(&d)).unwrap();
        for bad in [
            format!("add Bad_Id --name X --data-dir /v --data {ds}"),
            format!("add delta --name X --data-dir relative/path --data {ds}"),
            format!("add delta --name X --data-dir /a/../b --data {ds}"),
            format!("add delta --name X --data-dir /v --host not_a_host! --data {ds}"),
            format!("add acme --name Same --data-dir /v --data {ds}"),
            format!("add delta --name Dup --data-dir /var/lib/iyakki-beta --data {ds}"),
            format!("add delta --data-dir /v --data {ds}"),
            format!("remove nobody --data {ds}"),
            format!("add delta --name X --data-dir /v"),
            format!("frobnicate --data {ds}"),
        ] {
            assert!(run(&bad).0.is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_to_string(file(&d)).unwrap(), before);
        // replace by id, remove, list
        assert!(run(&format!("add beta --name Beta2 --data-dir /var/lib/iyakki-beta --data {ds}")).0.is_ok());
        assert!(run(&format!("remove gamma --data {ds}")).0.is_ok());
        let (r, lines) = run(&format!("list --file {}", file(&d).display()));
        assert!(r.is_ok() && lines.len() == 2 && lines[0].contains("acme") && lines[1].contains("Beta2") && !lines.iter().any(|l| l.contains("gamma")), "{lines:?}");
        assert!(run(&format!("list --file {ds}/other.json")).0.is_err());
        // no temporary file is left behind
        let names: Vec<_> = std::fs::read_dir(d.join("cloud")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["tenants.json"], "{names:?}");
    }

    #[test]
    fn a_broken_file_is_not_overwritten_and_the_pod_manifest_names_the_tenant_data_folder() {
        let d = tmp("broken");
        std::fs::create_dir_all(d.join("cloud")).unwrap();
        std::fs::write(file(&d), "{ broken").unwrap();
        assert!(run(&format!("add beta --name B --data-dir /v --data {}", d.display())).0.is_err());
        assert_eq!(std::fs::read_to_string(file(&d)).unwrap(), "{ broken");
        let y = crate::cloudtenant::manifests(&crate::cloudtenant::TenantConfig::example("acme")).unwrap();
        assert!(y.contains("IYAKKI_TENANT_DATA_DIR, value: /var/lib/iyakki"), "{y}");
        assert!(y.matches("name: IYAKKI_TENANT_DATA_DIR").count() == 1);
    }
}

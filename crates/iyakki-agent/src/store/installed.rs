//! What is installed: everything present on this system that the store knows or can detect, read from the real stores (MCP
//! servers, the CLI tile's tools, skills, WASM tool packages, content installed through the store, and the tiles themselves),
//! never from a catalog. This is what the STORE tile opens on.

use iyakki_sec::policy::{App, Policy};

use super::install::{record_key, scope_key, Actor};
use super::manifest::{extension_of, Meta, Permission};
use super::trust::{Level, Verdict};
use super::{builtin, Item, State, Store};
use crate::pkg::{self, newer};
use crate::services::Services;
use crate::skills::Scope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Mcp,
    Cli,
    Skill,
    App,
    Extension,
    /// Driver packs (firmware, module options, kernel modules for this machine): `extension` packages of ext_kind `driver`.
    Driver,
}

impl Type {
    pub const ALL: [Type; 6] = [Type::Mcp, Type::Cli, Type::Skill, Type::App, Type::Extension, Type::Driver];

    /// The filter chip.
    pub fn chip(self) -> &'static str {
        match self {
            Type::Mcp => "MCP",
            Type::Cli => "CLI",
            Type::Skill => "Skills",
            Type::App => "Apps",
            Type::Extension => "Extensions",
            Type::Driver => "Drivers",
        }
    }

    /// The type chip on a row.
    pub fn one(self) -> &'static str {
        match self {
            Type::Mcp => "MCP server",
            Type::Cli => "CLI tool",
            Type::Skill => "Skill",
            Type::App => "System app",
            Type::Extension => "Extension",
            Type::Driver => "Driver",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    On,
    Off,
    Update(String),
    Revoked,
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::On => "On".into(),
            Status::Off => "Turned off".into(),
            Status::Update(v) => format!("Update {v}"),
            Status::Revoked => "Revoked".into(),
        }
    }
}

/// Where "Open" goes: a tile, and what in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub tile: String,
    pub item: String,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub ty: Type,
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: String,
    pub level: Level,
    pub trust_label: String,
    pub status: Status,
    /// Bytes, 0 when not known.
    pub size: u64,
    /// Unix seconds, 0 when the system put it there.
    pub installed_at: i64,
    pub scope: Scope,
    pub publisher: String,
    pub license: String,
    pub source: String,
    pub summary: String,
    /// For extensions: the tile it is for and what it is.
    pub target: Option<String>,
    pub ext_kind: Option<String>,
    pub open: Option<Open>,
    pub permissions: Vec<String>,
    pub files: Vec<String>,
    /// The publisher's categories (empty when the item did not come with any).
    pub categories: Vec<String>,
    /// Can be removed here (a system app cannot).
    pub removable: bool,
    /// Can be turned off and on (only what was installed from a package).
    pub can_toggle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Name,
    Recent,
    Size,
    Trust,
    /// Things with an update first.
    Updates,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Recent => "recently installed or updated",
            Sort::Size => "size",
            Sort::Trust => "trust",
            Sort::Updates => "updates first",
        }
    }

    pub fn next(self) -> Sort {
        match self {
            Sort::Name => Sort::Recent,
            Sort::Recent => Sort::Size,
            Sort::Size => Sort::Trust,
            Sort::Trust => Sort::Updates,
            Sort::Updates => Sort::Name,
        }
    }
}

/// Counts for the chips: all, then each type in [`Type::ALL`] order.
pub fn counts(entries: &[Entry]) -> [usize; 7] {
    let mut c = [entries.len(), 0, 0, 0, 0, 0, 0];
    for e in entries {
        c[1 + Type::ALL.iter().position(|t| *t == e.ty).unwrap_or(0)] += 1;
    }
    c
}

/// The words a search looks through for an entry (lower case).
pub fn hay(e: &Entry) -> String {
    format!("{} {} {} {} {} {}", e.title, e.name, e.summary, e.publisher, e.ty.one(), e.target.as_deref().unwrap_or("")).to_lowercase()
}

/// The entries of a type (all when `None`) that match the words, in the chosen order.
pub fn filter<'a>(entries: &'a [Entry], ty: Option<Type>, query: &str, sort: Sort) -> Vec<&'a Entry> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut v: Vec<&Entry> = entries
        .iter()
        .filter(|e| ty.is_none_or(|t| e.ty == t))
        .filter(|e| {
            let hay = hay(e);
            words.iter().all(|w| hay.contains(w))
        })
        .collect();
    v.sort_by(|a, b| {
        // the tiles of the system always come last: what a person added is what they came to see
        let apps = (a.ty == Type::App).cmp(&(b.ty == Type::App));
        let by_name = a.title.to_lowercase().cmp(&b.title.to_lowercase());
        match sort {
            Sort::Name => apps.then(by_name),
            Sort::Recent => apps.then(b.installed_at.cmp(&a.installed_at)).then(by_name),
            Sort::Size => apps.then(b.size.cmp(&a.size)).then(by_name),
            Sort::Trust => apps.then(a.level.cmp(&b.level)).then(by_name),
            Sort::Updates => apps.then((!matches!(a.status, Status::Update(_))).cmp(&!matches!(b.status, Status::Update(_)))).then(by_name),
        }
    });
    v
}

fn scope_label(s: &Scope) -> String {
    match s {
        Scope::Shared => "everyone".into(),
        Scope::User(_) => "only you".into(),
    }
}

impl Store {
    fn level_of(&self, own: &[(String, String)], revs: &super::revocation::Revocations, kind: &str, name: &str, version: &str, sha: &str, signer: Option<&String>) -> Verdict {
        let m = pkg::Manifest { kind: kind.into(), name: name.into(), version: version.into(), license: String::new(), sha256: sha.into(), signer: signer.cloned(), meta: None, cert: None };
        self.judge(own, revs).verdict(&m)
    }

    /// Everything installed that `user` can see (their own and the shared ones). `items` (the catalog view, may be empty)
    /// only supplies "update available".
    pub fn installed(&self, svc: &Services, policy: &Policy, user: &str, items: &[Item]) -> Vec<Entry> {
        let actor = Actor::from_policy(user, policy);
        let (own, revs) = (svc.packages.trusted_keys(), self.revocations());
        let mine = Scope::User(user.to_string());
        let tile_ok = |t: &str| App::ALL.iter().any(|a| a.id() == t && *a != App::Inbox && policy.apps.contains(a));
        let open = |tile: &str, item: &str| tile_ok(tile).then(|| Open { tile: tile.into(), item: item.into() });
        let update_of = |kind: &str, name: &str| items.iter().find(|i| i.entry.kind == kind && i.entry.name == name).and_then(|i| if let State::Update(v) = &i.state { Some(v.clone()) } else { None });
        let mut out: Vec<Entry> = Vec::new();
        // 1. everything the store installed (also what is turned off)
        let records = self.records();
        for r in records.values().filter(|r| r.scope() == Scope::Shared || r.scope() == mine) {
            let copy = std::fs::read_to_string(self.dir().join("pkgs").join(format!("{}.iyp", r.sha256))).ok().and_then(|t| pkg::parse(&t).ok());
            let (meta, license) = match &copy {
                Some(p) => (p.manifest.meta.as_deref().and_then(|j| Meta::parse(j).ok()).unwrap_or_default(), p.manifest.license.clone()),
                None => (Meta::default(), String::new()),
            };
            let level = Level::ALL.into_iter().find(|l| l.id() == r.level).unwrap_or(Level::Community);
            let now_level = if r.source == "builtin" && r.signer.is_none() { level } else { self.level_of(&own, &revs, &r.kind, &r.name, &r.version, &r.sha256, r.signer.as_ref()).level.min(level.max(Level::Official)).max(if self.level_of(&own, &revs, &r.kind, &r.name, &r.version, &r.sha256, r.signer.as_ref()).level == Level::Revoked { Level::Revoked } else { level }) };
            let ext = extension_of(&r.kind, &meta);
            let ty = match r.kind.as_str() {
                "skill" => Type::Skill,
                "mcp" => Type::Mcp,
                _ if meta.ext_kind == "driver" => Type::Driver,
                _ => Type::Extension,
            };
            let tile = match (ty, &ext) {
                (Type::Skill, _) => "skills".to_string(),
                (Type::Mcp, _) => "mcp".to_string(),
                (_, Some((t, _))) if t != "any" => t.clone(),
                _ => "chat".to_string(),
            };
            let status = if now_level == Level::Revoked { Status::Revoked } else if r.disabled { Status::Off } else if let Some(v) = update_of(&r.kind, &r.name) { Status::Update(v) } else { Status::On };
            out.push(Entry {
                key: record_key(&r.kind, &r.scope(), &r.name),
                ty,
                kind: r.kind.clone(),
                name: r.name.clone(),
                title: if r.title.is_empty() { r.name.clone() } else { r.title.clone() },
                version: r.version.clone(),
                level: now_level,
                trust_label: if r.publisher.is_empty() { r.level.clone() } else { format!("{} ({})", r.level, r.publisher) },
                status,
                size: std::fs::metadata(self.dir().join("pkgs").join(format!("{}.iyp", r.sha256))).map_or(0, |m| m.len()),
                installed_at: r.installed_at,
                scope: r.scope(),
                publisher: if meta.publisher.name.is_empty() { r.publisher.clone() } else { meta.publisher.name.clone() },
                license,
                source: if r.source == "builtin" { "built into this image".into() } else { format!("the store ({})", r.source) },
                summary: meta.summary.clone(),
                target: ext.as_ref().map(|e| e.0.clone()),
                ext_kind: ext.as_ref().map(|e| e.1.clone()),
                open: if ty == Type::Driver {
                    None
                } else if r.kind == "tile" {
                    // a custom app opens in its own tile (the shell reads `tile-open:<name>`)
                    super::tilehub::allowed(policy).then(|| Open { tile: format!("tile-open:{}", r.name), item: String::new() })
                } else {
                    open(&tile, &r.name)
                },
                permissions: meta.permissions.iter().filter_map(|p| Permission::parse(p).ok()).map(|p| p.words(user)).collect(),
                files: vec![format!("{}.iyp (the verified copy the store keeps)", r.sha256.chars().take(12).collect::<String>())],
                categories: meta.categories.clone(),
                removable: true,
                can_toggle: true,
            });
        }
        let known = |out: &[Entry], kind: &str, scope: &Scope, name: &str| out.iter().any(|e| e.key == record_key(kind, scope, name));
        // 2. skills, whoever put them there
        for sk in svc.skills.list(user) {
            if known(&out, "skill", &sk.scope, &sk.name) {
                continue;
            }
            let rec = svc.packages.list(&sk.scope).into_iter().find(|(n, _)| *n == sk.name).map(|(_, i)| i);
            let shipped = builtin::SKILLS.iter().chain(builtin::SAMPLE_SKILLS.iter()).any(|(n, _)| *n == sk.name);
            let (level, label) = match &rec {
                Some(i) => {
                    let v = self.level_of(&own, &revs, "skill", &sk.name, &i.version, &i.sha256, i.signer.as_ref());
                    (v.level, v.label)
                }
                None if shipped => (Level::Official, "built into this image".to_string()),
                None => (Level::Unsigned, "written here, not from a package".to_string()),
            };
            out.push(Entry {
                key: record_key("skill", &sk.scope, &sk.name),
                ty: Type::Skill,
                kind: "skill".into(),
                name: sk.name.clone(),
                title: sk.name.clone(),
                version: rec.as_ref().map_or_else(|| if shipped { builtin::VERSION.to_string() } else { "-".into() }, |i| i.version.clone()),
                level,
                trust_label: label,
                status: if level == Level::Revoked { Status::Revoked } else { Status::On },
                size: sk.body.len() as u64,
                installed_at: 0,
                scope: sk.scope.clone(),
                publisher: if shipped { "IYAKKI".into() } else { String::new() },
                license: if shipped { "Apache-2.0".into() } else { String::new() },
                source: if shipped { "came with IYAKKI".into() } else if rec.is_some() { "a package installed in SKILLS".into() } else { "written or pasted in SKILLS".into() },
                summary: sk.description.clone(),
                target: None,
                ext_kind: None,
                open: open("skills", &sk.name),
                permissions: vec![],
                files: vec![format!("{} ({})", format!("{}.md", sk.name), scope_label(&sk.scope))],
                categories: vec![],
                removable: true,
                can_toggle: false,
            });
        }
        // 3. MCP servers added in the MCP tile (and the sample)
        for s in svc.mcp.list() {
            if known(&out, "mcp", &Scope::Shared, &s.name) {
                continue;
            }
            out.push(Entry {
                key: record_key("mcp", &Scope::Shared, &s.name),
                ty: Type::Mcp,
                kind: "mcp".into(),
                name: s.name.clone(),
                title: s.name.clone(),
                version: if s.sample { builtin::VERSION.into() } else { "-".into() },
                level: if s.sample { Level::Official } else { Level::Unsigned },
                trust_label: if s.sample { "the sample server that comes with IYAKKI".into() } else { "added by hand in the MCP tile".into() },
                status: Status::On,
                size: 0,
                installed_at: 0,
                scope: Scope::Shared,
                publisher: if s.sample { "IYAKKI".into() } else { String::new() },
                license: if s.sample { "Apache-2.0".into() } else { String::new() },
                source: if s.sample { "came with IYAKKI".into() } else { "added in the MCP tile".into() },
                summary: format!("{} {}", s.command, s.args.join(" ")).trim().to_string(),
                target: None,
                ext_kind: None,
                open: open("mcp", &s.name),
                permissions: vec![Permission::Run(s.command.clone()).words(user)],
                files: vec![],
                categories: vec![],
                removable: true,
                can_toggle: false,
            });
        }
        // 4. WASM tool packages
        for scope in [mine.clone(), Scope::Shared] {
            for (name, i) in svc.packages.list_tools(&scope) {
                if known(&out, "tool", &scope, &name) {
                    continue;
                }
                let v = self.level_of(&own, &revs, "tool", &name, &i.version, &i.sha256, i.signer.as_ref());
                let t = svc.packages.get_tool(&scope, &name);
                out.push(Entry {
                    key: record_key("tool", &scope, &name),
                    ty: Type::Extension,
                    kind: "tool".into(),
                    name: name.clone(),
                    title: name.clone(),
                    version: i.version.clone(),
                    level: v.level,
                    trust_label: v.label,
                    status: if v.level == Level::Revoked { Status::Revoked } else { Status::On },
                    size: t.as_ref().map_or(0, |t| t.module.len() as u64),
                    installed_at: 0,
                    scope: scope.clone(),
                    publisher: String::new(),
                    license: String::new(),
                    source: "a tool package installed in SKILLS".into(),
                    summary: t.as_ref().map(|t| t.manifest.description.clone()).unwrap_or_default(),
                    target: Some("chat".into()),
                    ext_kind: Some("tool".into()),
                    open: open("skills", &name),
                    permissions: t.as_ref().map(|t| t.capability_lines(user)).unwrap_or_default(),
                    files: vec![format!("{name}.iyp ({})", scope_label(&scope))],
                    categories: vec![],
                    removable: true,
                    can_toggle: false,
                });
            }
        }
        // 5. command-line tools installed from the CLI tile
        for c in svc.clis.installed() {
            let d = crate::clis::def(&c.id);
            out.push(Entry {
                key: format!("cli//{}", c.id),
                ty: Type::Cli,
                kind: "cli".into(),
                name: c.id.clone(),
                title: d.map_or_else(|| c.id.clone(), |d| d.name.to_string()),
                version: c.version.clone(),
                level: Level::Official,
                trust_label: if c.verified { "downloaded from its maker and checked against a published checksum".into() } else { "downloaded from its maker over HTTPS".into() },
                status: Status::On,
                size: 0,
                installed_at: c.at,
                scope: Scope::Shared,
                publisher: d.map_or(String::new(), |d| d.home.trim_start_matches("https://").trim_end_matches('/').to_string()),
                license: String::new(),
                source: format!("the CLI tile, installed by {}", c.by),
                summary: d.map(|d| d.about.to_string()).unwrap_or_default(),
                target: None,
                ext_kind: None,
                open: open("cli", &c.id),
                permissions: d.map(|d| d.hosts.iter().map(|h| Permission::parse(&format!("net:{h}")).map(|p| p.words(user)).unwrap_or_default()).collect()).unwrap_or_default(),
                files: c.bins.iter().map(|b| format!("{b} (linked in the CLI tile's bin folder)")).collect(),
                categories: vec![],
                removable: true,
                can_toggle: false,
            });
        }
        // 6. the tiles themselves
        for a in App::ALL {
            if a == App::Inbox || !policy.apps.contains(&a) {
                continue;
            }
            out.push(Entry {
                key: format!("app//{}", a.id()),
                ty: Type::App,
                kind: "app".into(),
                name: a.id().into(),
                title: a.label(),
                version: env!("CARGO_PKG_VERSION").into(),
                level: Level::Official,
                trust_label: "part of this IYAKKI system".into(),
                status: Status::On,
                size: 0,
                installed_at: 0,
                scope: Scope::Shared,
                publisher: "IYAKKI".into(),
                license: "Apache-2.0".into(),
                source: "built into the system image".into(),
                summary: "A tile of the system. It cannot be removed or turned off here.".into(),
                target: None,
                ext_kind: None,
                open: (a != App::Store).then(|| Open { tile: a.id().into(), item: String::new() }),
                permissions: vec![],
                files: vec![],
                categories: vec![],
                removable: false,
                can_toggle: false,
            });
        }
        let _ = (actor, scope_key(&Scope::Shared), newer("1", "0"));
        out
    }

    /// Take an entry away. System apps refuse with the reason.
    pub fn remove_installed(&self, svc: &Services, policy: &Policy, user: &str, e: &Entry) -> Result<String, String> {
        let actor = Actor::from_policy(user, policy);
        if !e.removable {
            return Err(format!("{} is part of IYAKKI and cannot be removed.", e.title));
        }
        match e.ty {
            Type::Cli => {
                if !actor.admin {
                    return Err("Only administrators can remove command-line tools.".into());
                }
                svc.clis.remove(&e.name)?;
            }
            _ => self.remove(svc, &actor, &e.kind, &e.name, &e.scope)?,
        }
        Ok(format!("Removed {}.", e.title))
    }

    /// Turn an entry off, or on again.
    pub fn toggle_installed(&self, svc: &Services, policy: &Policy, user: &str, e: &Entry) -> Result<String, String> {
        let actor = Actor::from_policy(user, policy);
        if e.ty == Type::App {
            return Err(format!("{} is part of IYAKKI and cannot be turned off here.", e.title));
        }
        if !e.can_toggle {
            return Err(format!("{} did not come from a store package, so it can only be removed.", e.title));
        }
        if e.status == Status::Off {
            self.enable(svc, &actor, &e.kind, &e.name, &e.scope).map_err(|x| x.to_string())?;
            Ok(format!("{} is on again.", e.title))
        } else {
            self.disable(svc, &actor, &e.kind, &e.name, &e.scope)?;
            Ok(format!("{} is turned off. Turn it on again here.", e.title))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::install::tests::{fx, meta, skill_pkg, Fx};
    use super::super::install::Confirm;
    use super::*;
    use crate::pkg::{new_signing_key, public_key_hex};
    use iyakki_sec::policy::Role;

    fn list(f: &Fx, role: Role, user: &str) -> Vec<Entry> {
        let p = role.template();
        let l = f.store.local_catalogs(&f.svc);
        let a = Actor::from_policy(user, &p);
        let items = f.store.items(&f.svc, &a, &l);
        f.store.installed(&f.svc, &p, user, &items)
    }

    fn of<'a>(v: &'a [Entry], ty: Type, name: &str) -> &'a Entry {
        v.iter().find(|e| e.ty == ty && e.name == name).unwrap_or_else(|| panic!("{ty:?} {name} is listed: {:?}", v.iter().map(|e| (e.ty, &e.name)).collect::<Vec<_>>()))
    }

    #[test]
    fn a_bare_system_lists_only_its_tiles_as_system_apps_which_cannot_be_removed_or_turned_off() {
        let f = fx("inst-bare");
        let v = list(&f, Role::Admin, "root");
        assert!(v.iter().all(|e| e.ty == Type::App), "no skills, servers, tools or extensions yet, and no catalog items");
        assert!(v.len() >= 20);
        let cli = of(&v, Type::App, "cli");
        assert_eq!((cli.title.as_str(), cli.level, cli.removable, cli.can_toggle), ("CLI", Level::Official, false, false));
        assert_eq!(cli.open, Some(Open { tile: "cli".into(), item: String::new() }));
        let p = Role::Admin.template();
        assert!(f.store.remove_installed(&f.svc, &p, "root", cli).unwrap_err().contains("part of IYAKKI and cannot be removed"));
        assert!(f.store.toggle_installed(&f.svc, &p, "root", cli).unwrap_err().contains("cannot be turned off"));
        // a guest sees only the tiles the role has
        let g = list(&f, Role::Guest, "guest");
        assert!(g.iter().any(|e| e.name == "chat") && !g.iter().any(|e| e.name == "cli" || e.name == "store"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn every_type_is_listed_from_the_real_stores() {
        let f = fx("inst-types");
        // a skill by hand, a shipped one, an MCP server, a CLI tool, a tool package, content from the store
        f.svc.skills.add(Scope::Shared, "---\nname: my-skill\ndescription: mine\ntriggers: mine\n---\nDo it.\n", true, true).unwrap();
        f.svc.skills.add(Scope::Shared, builtin::SKILLS[1].1, true, true).unwrap();
        f.svc.mcp.add(crate::mcp::StoredServer { name: "iyakki-demo".into(), command: "/x/iyakki".into(), args: vec!["--mcp-demo".into()], sample: true }).unwrap();
        f.svc.mcp.add(crate::mcp::StoredServer { name: "files".into(), command: "/usr/bin/files".into(), args: vec![], sample: false }).unwrap();
        crate::clis::write_fake_installed(&f.svc.clis, "kubectl", "#!/bin/sh\necho v1\n");
        let key = new_signing_key().unwrap();
        let tool = crate::pkg::build_tool(&crate::wtool::tests::manifest("shout", &["net:example.com"], serde_json::json!({})), crate::wtool::tests::ECHO_WAT.as_bytes(), Some(&key)).unwrap();
        let r = f.svc.packages.review_tool(&tool, &Scope::Shared).unwrap();
        f.svc.packages.install_tool(&r, true, true, true).unwrap();
        let theme = crate::pkg::build_generic("theme", "sunset", "1.0.0", "MIT", "{}", Some(&Meta { target: "tales".into(), ..meta("acme", &[]) }.to_json()), None, Some(&key)).unwrap().into_bytes();
        let a = Actor::from_policy("root", &Role::Admin.template());
        let rv = f.store.review_sideload(&f.svc, &a, theme, Scope::User("root".into()), &[]);
        f.store.install(&f.svc, &a, &rv, Confirm::all()).unwrap();
        let v = list(&f, Role::Admin, "root");
        let c = counts(&v);
        assert_eq!(c[0], v.len());
        assert_eq!((c[1], c[2], c[3], c[5]), (2, 1, 2, 2), "MCP, CLI, skills, extensions");
        let mine = of(&v, Type::Skill, "my-skill");
        assert_eq!((mine.level, mine.can_toggle, mine.removable), (Level::Unsigned, false, true));
        assert_eq!(mine.open, Some(Open { tile: "skills".into(), item: "my-skill".into() }));
        let py = of(&v, Type::Skill, builtin::SKILLS[1].0);
        assert_eq!((py.level, py.version.as_str(), py.publisher.as_str()), (Level::Official, "1.0.0", "IYAKKI"), "the preinstalled skills are listed as Official");
        let demo = of(&v, Type::Mcp, "iyakki-demo");
        assert_eq!(demo.level, Level::Official);
        assert_eq!(of(&v, Type::Mcp, "files").level, Level::Unsigned);
        assert_eq!(of(&v, Type::Mcp, "files").open, Some(Open { tile: "mcp".into(), item: "files".into() }));
        let k = of(&v, Type::Cli, "kubectl");
        assert_eq!((k.level, k.title.as_str()), (Level::Official, "kubectl"));
        assert_eq!(k.open, Some(Open { tile: "cli".into(), item: "kubectl".into() }));
        let t = of(&v, Type::Extension, "shout");
        assert_eq!((t.ext_kind.as_deref(), t.target.as_deref(), t.level), (Some("tool"), Some("chat"), Level::Community));
        assert!(t.permissions.iter().any(|p| p.contains("example.com")) && t.size > 0);
        let th = of(&v, Type::Extension, "sunset");
        assert_eq!((th.ext_kind.as_deref(), th.target.as_deref(), th.status.clone()), (Some("theme"), Some("tales"), Status::On));
        assert_eq!(th.open, Some(Open { tile: "tales".into(), item: "sunset".into() }));
        assert!(th.installed_at > 0 && th.size > 0 && !th.files.is_empty());
        // another account sees the shared ones but not root's personal theme
        let bob = list(&f, Role::User, "bob");
        assert!(bob.iter().any(|e| e.name == "my-skill") && !bob.iter().any(|e| e.name == "sunset"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn filters_search_and_sorts_work_on_the_real_list() {
        let f = fx("inst-filter");
        f.svc.skills.add(Scope::Shared, "---\nname: zeta-notes\ndescription: take notes\ntriggers: notes\n---\nDo it.\n", true, true).unwrap();
        f.svc.skills.add(Scope::Shared, builtin::SKILLS[2].1, true, true).unwrap();
        f.svc.mcp.add(crate::mcp::StoredServer { name: "files".into(), command: "/usr/bin/files".into(), args: vec![], sample: false }).unwrap();
        let v = list(&f, Role::Admin, "root");
        let names = |x: Vec<&Entry>| x.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(filter(&v, Some(Type::Mcp), "", Sort::Name)), vec!["files"]);
        assert_eq!(filter(&v, Some(Type::Skill), "", Sort::Name).len(), 2);
        assert_eq!(names(filter(&v, None, "take notes", Sort::Name)), vec!["zeta-notes"]);
        assert!(filter(&v, None, "qqqq", Sort::Name).is_empty());
        assert!(filter(&v, Some(Type::App), "cli", Sort::Name).iter().any(|e| e.name == "cli"));
        // trust order: Official before Unsigned
        let s = filter(&v, Some(Type::Skill), "", Sort::Trust);
        assert_eq!((s[0].level, s[1].level), (Level::Official, Level::Unsigned));
        let n = filter(&v, Some(Type::Skill), "", Sort::Name);
        assert!(n[0].title.to_lowercase() <= n[1].title.to_lowercase());
        // size: the longer body first
        let sz = filter(&v, Some(Type::Skill), "", Sort::Size);
        assert!(sz[0].size >= sz[1].size);
        assert_eq!(Sort::Name.next().next().next().next().next(), Sort::Name);
        assert_eq!(Type::ALL.map(|t| t.chip()), ["MCP", "CLI", "Skills", "Apps", "Extensions", "Drivers"]);
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn removing_and_turning_off_go_through_the_right_store_and_updates_show() {
        let f = fx("inst-actions");
        let p = Role::Admin.template();
        f.svc.skills.add(Scope::Shared, "---\nname: hand-made\ndescription: x\ntriggers: x\n---\nDo it.\n", true, true).unwrap();
        f.svc.mcp.add(crate::mcp::StoredServer { name: "files".into(), command: "/usr/bin/files".into(), args: vec![], sample: false }).unwrap();
        crate::clis::write_fake_installed(&f.svc.clis, "kubectl", "#!/bin/sh\necho v1\n");
        let a = Actor::from_policy("root", &p);
        let key = new_signing_key().unwrap();
        f.svc.packages.trust_key(&public_key_hex(&key), "Acme").unwrap();
        let rv = f.store.review_sideload(&f.svc, &a, skill_pkg("1.0.0", Some(&key), None, &meta("acme", &[])), Scope::User("root".into()), &[]);
        f.store.install(&f.svc, &a, &rv, Confirm::all()).unwrap();
        let v = list(&f, Role::Admin, "root");
        // a store-installed skill can be turned off and on; one written by hand cannot
        let mn = of(&v, Type::Skill, "meeting-notes").clone();
        assert_eq!((mn.level, mn.can_toggle), (Level::Trusted, true));
        assert!(f.store.toggle_installed(&f.svc, &p, "root", &mn).unwrap().contains("turned off"));
        let v = list(&f, Role::Admin, "root");
        let off = of(&v, Type::Skill, "meeting-notes").clone();
        assert_eq!(off.status, Status::Off);
        assert!(f.svc.skills.get(&Scope::User("root".into()), "meeting-notes").is_none());
        assert!(f.store.toggle_installed(&f.svc, &p, "root", &off).unwrap().contains("on again"));
        assert!(f.svc.skills.get(&Scope::User("root".into()), "meeting-notes").is_some());
        let hm = of(&v, Type::Skill, "hand-made").clone();
        assert!(f.store.toggle_installed(&f.svc, &p, "root", &hm).unwrap_err().contains("can only be removed"));
        // an update known from a catalog shows in the status
        let newer_pkg = skill_pkg("1.2.0", Some(&key), None, &meta("acme", &[]));
        let stick = f.dir.join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        std::fs::write(stick.join("mn.iyp"), &newer_pkg).unwrap();
        let mut e = super::super::catalog::tests::entry("meeting-notes", &newer_pkg, "mn.iyp");
        e.version = "1.2.0".into();
        e.signer = public_key_hex(&key);
        e.meta = meta("acme", &[]);
        std::fs::write(stick.join("catalog.json"), super::super::catalog::tests::catalog("stick", 1, vec![e]).to_json()).unwrap();
        f.store.add_source(&a, "S", super::super::catalog::SourceKind::Folder, stick.to_str().unwrap()).unwrap();
        let v = list(&f, Role::Admin, "root");
        assert_eq!(of(&v, Type::Skill, "meeting-notes").status, Status::Update("1.2.0".into()));
        // removing: skill, server and CLI tool; a user cannot remove a CLI tool
        for (ty, name) in [(Type::Skill, "hand-made"), (Type::Mcp, "files"), (Type::Cli, "kubectl")] {
            let e = of(&v, ty, name).clone();
            let user = Role::User.template();
            if ty == Type::Cli {
                assert!(f.store.remove_installed(&f.svc, &user, "alice", &e).unwrap_err().contains("administrators"));
            }
            assert!(f.store.remove_installed(&f.svc, &p, "root", &e).unwrap().starts_with("Removed"), "{name}");
        }
        let v = list(&f, Role::Admin, "root");
        assert!(!v.iter().any(|e| e.name == "hand-made" || e.name == "files" || e.name == "kubectl"));
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn open_actions_follow_the_tiles_the_role_has() {
        let f = fx("inst-open");
        f.svc.skills.add(Scope::Shared, "---\nname: hand-made\ndescription: x\ntriggers: x\n---\nDo it.\n", true, true).unwrap();
        let mut p = Role::User.template();
        let l = f.store.local_catalogs(&f.svc);
        let a = Actor::from_policy("alice", &p);
        let items = f.store.items(&f.svc, &a, &l);
        let v = f.store.installed(&f.svc, &p, "alice", &items);
        assert_eq!(of(&v, Type::Skill, "hand-made").open.as_ref().map(|o| o.tile.as_str()), Some("skills"));
        p.apps.remove(&App::Skills);
        let v = f.store.installed(&f.svc, &p, "alice", &items);
        assert!(of(&v, Type::Skill, "hand-made").open.is_none(), "no Open into a tile the account does not have");
        let _ = std::fs::remove_dir_all(&f.dir);
    }
}

//! The glue between the web service's cloud profile and the tenant's lists (docs/development/CLOUD-MULTITENANT-PLAN.md, sections 3a and 3b):
//! who is acting (always the signed-in account, never a name from the request), what they ask, and the answer. DENY BY DEFAULT.
//!
//! * [`Access::check`] asks [`Roles::can`] and turns the answer into what the caller may say: a person outside the tenant, or without the
//!   role, gets [`Denial::Forbidden`]; a tenant member who is not in a namespace gets [`Denial::NotFound`], the same as for a namespace
//!   that does not exist, so names do not leak. Every answer carries the audit line.
//! * A tenant with no roles file yet allows nothing. The only way out is [`Access::bootstrap`]: the first call makes its caller the
//!   tenant's owner and writes the file; every later call is refused. (The web route requires a service administrator to call it.)
//! * Changes go through [`Access::change`]: the lists are reloaded, the `Roles` function (which enforces who may do what) runs, the lists
//!   are saved with `cloudroles::save` and the line is appended with `cloudroles::append_audit`.
//! * A removal goes through [`Access::revoke`]: the same change, then the rotation of the vault scopes the person held (`cloudrotate.rs`);
//!   a rotation that cannot be done is recorded as pending and never undoes the removal.
//! * Paths come from [`Access::place`] only (`StorageLayout::locate` + `ensure` + `jail`); [`Access::after_checkout`] removes links that
//!   leave the work area after a clone or checkout.
//!
//! The lists are cached per file and reloaded when the file's modification time or size changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::cloudkeys::MemberKeys;
use crate::cloudrotate::{self, Report};
use crate::cloudroles::{self, Action, Change, Decision, Roles, Why};
use crate::cloudstorage::{self, Scope, StorageLayout};
use crate::cloudvaultd::Scope as VaultScope;
use crate::cloudvaultlink::Link;

/// Who is taken out of what ([`Access::revoke`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revoke {
    Tenant { who: String },
    Namespace { ns: String, who: String },
}

/// What the caller may say about a refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denial {
    /// 403: the person is not in the tenant, or their role is too low for something they can see.
    Forbidden(String),
    /// 404: no such namespace, as far as this person may know.
    NotFound(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub denial: Option<Denial>,
    /// One line for the service's audit trail (names only, no content).
    pub audit: String,
}

impl Verdict {
    pub fn ok(&self) -> bool {
        self.denial.is_none()
    }
}

type Stamp = (Option<SystemTime>, u64);

struct Cached {
    stamp: Stamp,
    roles: Option<Roles>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Held while the lists are changed, so two changes never overwrite each other.
fn change_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn stamp(file: &Path) -> Stamp {
    match std::fs::metadata(file) {
        Ok(m) => (m.modified().ok(), m.len()),
        Err(_) => (None, 0),
    }
}

/// The access rules of one tenant, whose data directory is `data_dir`.
#[derive(Clone, Debug)]
pub struct Access {
    data_dir: PathBuf,
    layout: StorageLayout,
}

impl Access {
    pub fn new(data_dir: &Path) -> Access {
        Access { data_dir: data_dir.to_path_buf(), layout: StorageLayout::default() }
    }

    pub fn roles_file(&self) -> PathBuf {
        cloudroles::path(&self.data_dir)
    }

    /// The folder the places of the tenant's files live under (the volume's mount, in the first version a folder of the data directory).
    pub fn files_root(&self) -> PathBuf {
        self.data_dir.join("cloud").join("files")
    }

    /// The current lists: `Ok(None)` when there is no file yet. A file that cannot be read is an error, which callers treat as a refusal.
    pub fn roles(&self) -> Result<Option<Roles>, String> {
        let file = self.roles_file();
        let now = stamp(&file);
        let mut c = lock(cache());
        if let Some(h) = c.get(&file) {
            if h.stamp == now {
                return Ok(h.roles.clone());
            }
        }
        let roles = cloudroles::load(&file)?;
        c.insert(file, Cached { stamp: now, roles: roles.clone() });
        Ok(roles)
    }

    /// May `who` (the signed-in account) do `what`? Deny by default: no lists, an unreadable file, an unknown person.
    pub fn check(&self, who: &str, what: &Action) -> Verdict {
        let line = |res: &str| format!("access {res} {who} {what:?}");
        let roles = match self.roles() {
            Ok(Some(r)) => r,
            Ok(None) => return Verdict { denial: Some(Denial::Forbidden("no access".into())), audit: line("denied(no-roles-file)") },
            Err(_) => return Verdict { denial: Some(Denial::Forbidden("no access".into())), audit: line("denied(roles-unreadable)") },
        };
        match roles.can(who, what) {
            Decision::Allowed => Verdict { denial: None, audit: line("allowed") },
            Decision::Denied(why) => {
                let denial = match &why {
                    // the same words for "not yours" and "not there"
                    Why::NotInNamespace | Why::NoSuchNamespace => Denial::NotFound("no such namespace".into()),
                    Why::NotInTenant => Denial::Forbidden("no access".into()),
                    Why::RoleTooLow => Denial::Forbidden("your role does not allow this".into()),
                };
                Verdict { denial: Some(denial), audit: line(&format!("denied({why:?})")) }
            }
        }
    }

    /// The first owner. Works only while there is no lists file; the caller becomes the tenant's owner and the file is written. Anyone
    /// calling later is refused, and so is a name that cannot be a folder.
    pub fn bootstrap(&self, who: &str) -> Result<Change, String> {
        let _g = lock(change_lock());
        let file = self.roles_file();
        if cloudroles::load(&file)?.is_some() {
            return Err("this tenant already has an owner".into());
        }
        let r = Roles::new(who)?;
        let c = Change(format!("tenant: {who} became the first owner"));
        self.save_and_audit(&r, &c)?;
        Ok(c)
    }

    /// Apply one change by `by`: `f` is one of the `Roles` change functions (they check who may do what). Saved and audited only when it
    /// succeeded. A tenant without lists cannot be changed here (see [`Access::bootstrap`]).
    pub fn change(&self, f: impl FnOnce(&mut Roles) -> Result<Change, String>) -> Result<Change, String> {
        let _g = lock(change_lock());
        let Some(mut r) = cloudroles::load(&self.roles_file())? else { return Err("no access".into()) };
        let c = f(&mut r)?;
        self.save_and_audit(&r, &c)?;
        Ok(c)
    }

    /// A removal of a person (from the tenant, or from one namespace) followed by the rotation of the vault scopes they held. The role
    /// change is saved and audited first and stays whatever happens to the rotation; what could not be rotated is kept as pending
    /// (`cloudrotate.rs`). `keys` are the acting person's vault secret, if this side holds one.
    pub fn revoke(&self, by: &str, what: &Revoke, link: &Link, keys: Option<&MemberKeys>) -> Result<(Change, Report), String> {
        let mut scopes = Vec::new();
        let (c, who) = match what {
            Revoke::Tenant { who } => (
                self.change(|r| {
                    scopes = cloudrotate::scopes_for_tenant_removal(r, who);
                    r.remove_tenant_member(by, who)
                })?,
                who,
            ),
            Revoke::Namespace { ns, who } => (
                self.change(|r| {
                    scopes = vec![VaultScope::Namespace(ns.clone())];
                    r.remove_namespace_member(by, ns, who)
                })?,
                who,
            ),
        };
        let report = cloudrotate::rotate(&self.data_dir, link, by, keys, who, &scopes);
        Ok((c, report))
    }

    fn save_and_audit(&self, r: &Roles, c: &Change) -> Result<(), String> {
        cloudroles::save(&self.roles_file(), r)?;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        cloudroles::append_audit(&cloudroles::audit_path(&self.data_dir), now, c)
    }

    /// `person`'s work folder in `ns`, made if missing. Call [`Access::check`] first; this never decides who may.
    pub fn place(&self, person: &str, ns: &str) -> Result<PathBuf, String> {
        let p = self.layout.locate(person, &Scope::Person { namespace: ns.into() })?;
        let root = self.files_root();
        std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        cloudstorage::ensure(&self.files_root(), &p)
    }

    /// A path the person gave, relative to their work folder in `ns`: located, ensured and jailed (`..`, absolute paths and links refused).
    pub fn path_in(&self, person: &str, ns: &str, rel: &str) -> Result<PathBuf, String> {
        cloudstorage::jail(&self.place(person, ns)?, rel)
    }

    /// Run after a clone or checkout into `base`: every link that leaves it is removed. Returns what was removed.
    pub fn after_checkout(&self, base: &Path) -> Result<Vec<PathBuf>, String> {
        let mut gone = Vec::new();
        while let Some(bad) = cloudstorage::links_leaving(base) {
            std::fs::remove_file(&bad).map_err(|e| format!("{}: cannot remove a link that leaves the folder: {e}", bad.display()))?;
            gone.push(bad);
            if gone.len() > 10_000 {
                return Err("too many links that leave the folder".into());
            }
        }
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudroles::{NsRole, TenantRole};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloudaccess-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// acme: owner olga, admin adam, members mia and max; namespace web (admin mia, editor max, viewer vic); data (admin olga).
    fn acme(tag: &str) -> Access {
        let a = Access::new(&dir(tag));
        a.bootstrap("olga").unwrap();
        a.change(|r| r.add_tenant_member("olga", "adam", TenantRole::Admin)).unwrap();
        for m in ["mia", "max", "vic"] {
            a.change(|r| r.add_tenant_member("olga", m, TenantRole::Member)).unwrap();
        }
        a.change(|r| r.create_namespace("adam", "web")).unwrap();
        a.change(|r| r.add_namespace_member("adam", "web", "mia", NsRole::Admin)).unwrap();
        a.change(|r| r.add_namespace_member("mia", "web", "max", NsRole::Editor)).unwrap();
        a.change(|r| r.add_namespace_member("mia", "web", "vic", NsRole::Viewer)).unwrap();
        a.change(|r| r.create_namespace("olga", "data")).unwrap();
        a
    }

    fn read(ns: &str) -> Action {
        Action::ReadNamespace(ns.into())
    }
    fn write(ns: &str) -> Action {
        Action::WriteNamespace(ns.into())
    }

    #[test]
    fn a_tenant_without_lists_denies_everything_until_the_first_owner_bootstraps() {
        let a = Access::new(&dir("nolists"));
        for who in ["olga", "eve"] {
            for what in [Action::EnterTenant, read("web"), write("web"), Action::ManageTenantMembers] {
                let v = a.check(who, &what);
                assert_eq!(v.denial, Some(Denial::Forbidden("no access".into())));
                assert!(v.audit.contains("no-roles-file"), "{}", v.audit);
            }
        }
        // a change cannot start a tenant either
        assert!(a.change(|r| r.create_namespace("olga", "web")).is_err());
        a.bootstrap("olga").unwrap();
        assert!(a.check("olga", &Action::EnterTenant).ok());
        assert!(!a.check("eve", &Action::EnterTenant).ok());
        // the second caller is refused, even the same person
        assert!(a.bootstrap("eve").unwrap_err().contains("already"));
        assert!(a.bootstrap("olga").is_err());
        assert!(!a.check("eve", &Action::EnterTenant).ok());
        // a name that cannot be a folder cannot be the owner
        assert!(Access::new(&dir("badowner")).bootstrap("../x").is_err());
    }

    #[test]
    fn a_stranger_is_denied_everywhere() {
        let a = acme("stranger");
        for what in [Action::EnterTenant, Action::ManageTenantMembers, Action::ManageNamespaces, Action::SeeNamespace("web".into()), read("web"), write("web"), Action::ManageNamespaceMembers("web".into()), read("nope")] {
            let v = a.check("eve", &what);
            assert_eq!(v.denial, Some(Denial::Forbidden("no access".into())), "{what:?}");
        }
        assert!(a.change(|r| r.add_tenant_member("eve", "eve", TenantRole::Owner)).is_err());
        assert!(a.change(|r| r.create_namespace("eve", "mine")).is_err());
        assert!(!a.roles().unwrap().unwrap().namespaces.contains_key("mine"));
    }

    #[test]
    fn a_member_outside_a_namespace_gets_the_answer_for_a_namespace_that_does_not_exist() {
        let a = acme("outside");
        // max is in web, not in data
        let hidden = a.check("max", &read("data"));
        let absent = a.check("max", &read("nothing-here"));
        assert_eq!(hidden.denial, absent.denial);
        assert_eq!(hidden.denial, Some(Denial::NotFound("no such namespace".into())));
        for what in [write("data"), Action::ManageNamespaceMembers("data".into()), Action::SeeNamespace("data".into())] {
            assert_eq!(a.check("max", &what).denial, a.check("max", &match what.clone() {
                Action::WriteNamespace(_) => write("nothing-here"),
                Action::ManageNamespaceMembers(_) => Action::ManageNamespaceMembers("nothing-here".into()),
                _ => Action::SeeNamespace("nothing-here".into()),
            }).denial, "{what:?}");
        }
        // and the list shown to them does not hold the name
        let r = a.roles().unwrap().unwrap();
        assert_eq!(r.visible_namespaces("max").iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["web"]);
    }

    #[test]
    fn a_tenant_admin_sees_the_names_but_cannot_read() {
        let a = acme("admin");
        // adam made web, so he administers it; olga made data. adam is not in data.
        assert!(a.check("adam", &Action::SeeNamespace("data".into())).ok());
        assert_eq!(a.check("adam", &read("data")).denial, Some(Denial::NotFound("no such namespace".into())));
        assert!(!a.check("adam", &write("data")).ok());
        let names: Vec<String> = a.roles().unwrap().unwrap().visible_namespaces("adam").into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["data", "web"]);
    }

    #[test]
    fn an_editor_writes_and_a_viewer_only_reads() {
        let a = acme("roles");
        assert!(a.check("max", &read("web")).ok() && a.check("max", &write("web")).ok());
        assert!(a.check("vic", &read("web")).ok());
        let v = a.check("vic", &write("web"));
        assert_eq!(v.denial, Some(Denial::Forbidden("your role does not allow this".into())));
        assert!(v.audit.contains("denied(RoleTooLow)"));
        assert!(!a.check("vic", &Action::ManageNamespaceMembers("web".into())).ok());
        assert!(a.check("mia", &Action::ManageNamespaceMembers("web".into())).ok());
    }

    #[test]
    fn paths_that_leave_the_work_folder_are_refused() {
        let a = acme("paths");
        let base = a.place("max", "web").unwrap();
        assert!(base.ends_with("ns/web/people/max") && base.is_dir());
        assert_eq!(a.path_in("max", "web", "src/main.rs").unwrap(), base.join("src").join("main.rs"));
        for bad in ["../mia/secret", "/etc/passwd", "a/../../b", ".."] {
            assert!(a.path_in("max", "web", bad).is_err(), "{bad}");
        }
        // names that cannot be folders never reach the disk
        assert!(a.place("../max", "web").is_err());
        assert!(a.place("max", "../data").is_err());
        #[cfg(unix)]
        {
            let other = a.place("mia", "web").unwrap();
            std::fs::write(other.join("secret"), "x").unwrap();
            std::os::unix::fs::symlink(&other, base.join("peek")).unwrap();
            assert!(a.path_in("max", "web", "peek/secret").is_err(), "a link is not followed");
            // a link planted by a checkout is removed, a link inside the folder stays
            std::fs::write(base.join("real"), "y").unwrap();
            std::os::unix::fs::symlink(base.join("real"), base.join("inside")).unwrap();
            let gone = a.after_checkout(&base).unwrap();
            assert_eq!(gone, vec![base.join("peek")]);
            assert!(!base.join("peek").exists() && std::fs::symlink_metadata(base.join("inside")).is_ok());
            assert!(cloudstorage::links_leaving(&base).is_none());
        }
        // another person's folder is a different place
        assert_ne!(a.place("mia", "web").unwrap(), base);
    }

    #[test]
    fn every_change_is_saved_and_has_a_line_in_the_audit_file() {
        let a = acme("audit");
        let text = std::fs::read_to_string(cloudroles::audit_path(&a.data_dir)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // bootstrap, adam, 3 members, web, 3 namespace members, data
        assert_eq!(lines.len(), 1 + 1 + 3 + 1 + 3 + 1);
        assert!(lines[0].contains("olga became the first owner"));
        assert!(lines.iter().all(|l| l.split_once('\t').is_some_and(|(t, _)| t.parse::<u64>().is_ok())));
        // a refused change writes nothing and does not touch the lists
        let before = std::fs::read_to_string(a.roles_file()).unwrap();
        assert!(a.change(|r| r.remove_tenant_member("mia", "olga")).is_err());
        assert_eq!(std::fs::read_to_string(cloudroles::audit_path(&a.data_dir)).unwrap().lines().count(), lines.len());
        assert_eq!(std::fs::read_to_string(a.roles_file()).unwrap(), before);
        // a change shows through the cache at once
        assert!(a.check("max", &write("web")).ok());
        a.change(|r| r.remove_tenant_member("olga", "max")).unwrap();
        assert!(!a.check("max", &write("web")).ok());
    }

    #[test]
    fn an_unreadable_lists_file_denies_everything() {
        let a = acme("broken");
        std::fs::write(a.roles_file(), "{ not json").unwrap();
        let v = a.check("olga", &Action::EnterTenant);
        assert!(!v.ok() && v.audit.contains("roles-unreadable"));
        assert!(a.change(|r| r.create_namespace("olga", "x")).is_err());
    }
}

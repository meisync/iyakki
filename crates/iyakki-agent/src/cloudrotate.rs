//! Key rotation when a role is revoked (docs/development/CLOUD-MULTITENANT-PLAN.md, 3a: "removal rotates keys").
//!
//! A person taken out of a tenant or a namespace still holds a wrap of the scope key in the vault service; the roles alone do not end that.
//! After the role change is saved, [`rotate`] makes the acting administrator's side open the values, make a new key, wrap it for those who
//! stay and seal everything again (`cloudvaultd::Client::rotate_out`). Which scopes: leaving a tenant rotates the `tenant` scope and every
//! namespace the person was in ([`scopes_for_tenant_removal`]); leaving a namespace rotates that namespace.
//!
//! The role change is never lost for the want of a rotation. When the acting person holds no key, when the vault is unreachable, refuses
//! (an administrator of the tenant who is not in the namespace holds no wrap there) or is misconfigured, the scope and the person are kept
//! in `cloud/rotation-pending.json` and a line "rotation pending" goes to the audit trail; [`retry_pending`] (run by
//! `iyakki cloud reconcile --retry-rotations`) tries again with the keys of whoever runs it. A tenant with no vault configured has nothing
//! to rotate and records nothing. A scope that does not exist in the vault, or in which the person holds no wrap, needs nothing and is
//! reported as such.
//!
//! The acting person's secret is read from `cloud/vault-keys/USER.key` (64 hex digits, mode 0600) on the tenant's volume when the change
//! comes through the tenant's own service, or from the file named on the command line; a pod normally holds none, so the usual path
//! for a web removal is "pending", finished by an administrator at a machine that has the key. Nothing secret is written to the audit
//! trail or the pending file (a scope name, a person's name and the error's first words).

use crate::cloudkeys::MemberKeys;
use crate::cloudroles::{self, Change, Roles};
use crate::cloudstorage::plain_name;
use crate::cloudvaultd::Scope;
use crate::cloudvaultlink::{Link, VaultTarget};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    /// `tenant` or `namespace:NAME`.
    pub scope: String,
    pub who: String,
    pub since: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub rotated: Vec<String>,
    /// Nothing to rotate (no such scope in the vault, or the person held no wrap), or no vault configured.
    pub nothing: Vec<String>,
    pub pending: Vec<String>,
}

pub fn pending_path(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("rotation-pending.json")
}

pub fn keys_path(data_dir: &Path, user: &str) -> Option<PathBuf> {
    plain_name(user).then(|| data_dir.join("cloud").join("vault-keys").join(format!("{user}.key")))
}

/// The acting person's secret, when this volume holds one for them.
pub fn actor_keys(data_dir: &Path, user: &str) -> Option<MemberKeys> {
    MemberKeys::from_hex(&std::fs::read_to_string(keys_path(data_dir, user)?).ok()?).ok()
}

pub fn load_pending(data_dir: &Path) -> Result<Vec<Pending>, String> {
    let p = pending_path(data_dir);
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn save_pending(data_dir: &Path, list: &[Pending]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let p = pending_path(data_dir);
    std::fs::create_dir_all(p.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(serde_json::to_string_pretty(list).map_err(|e| e.to_string())?.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn audit(data_dir: &Path, line: String) {
    if let Err(e) = cloudroles::append_audit(&cloudroles::audit_path(data_dir), now(), &Change(line)) {
        log::warn!("rotation: the audit line could not be written: {e}");
    }
}

fn short(e: &str) -> String {
    e.chars().filter(|c| !c.is_control()).take(120).collect()
}

/// The scopes to rotate when `who` leaves the tenant: the tenant scope and each namespace they are in (read from the lists BEFORE the change).
pub fn scopes_for_tenant_removal(before: &Roles, who: &str) -> Vec<Scope> {
    let mut v = vec![Scope::Tenant];
    v.extend(before.namespaces.iter().filter(|(_, m)| m.contains_key(who)).map(|(n, _)| Scope::Namespace(n.clone())));
    v
}

fn try_one(target: &VaultTarget, actor: &str, keys: Option<&MemberKeys>, scope: &Scope, who: &str) -> Result<bool, String> {
    let keys = keys.ok_or("the acting person holds no vault key here")?;
    target.client(actor, keys.clone())?.rotate_out(scope, who)
}

fn set_pending(list: &mut Vec<Pending>, scope: &Scope, who: &str, reason: &str) {
    let (s, r) = (scope.name(), short(reason));
    match list.iter_mut().find(|p| p.scope == s && p.who == who) {
        Some(p) => p.reason = r,
        None => list.push(Pending { scope: s, who: who.into(), since: now(), reason: r }),
    }
}

/// Rotate `who` out of each of `scopes`, as `actor`. Never fails: whatever cannot be done is recorded as pending (and audited).
pub fn rotate(data_dir: &Path, link: &Link, actor: &str, keys: Option<&MemberKeys>, who: &str, scopes: &[Scope]) -> Report {
    let mut rep = Report::default();
    let mut list = load_pending(data_dir).unwrap_or_default();
    let mut dirty = false;
    for s in scopes {
        let res = match link {
            Link::NotConfigured => Ok(false),
            Link::Invalid(e) => Err(format!("the vault address is not usable: {e}")),
            Link::Target(t) => try_one(t, actor, keys, s, who),
        };
        let name = s.name();
        match res {
            Ok(true) => {
                audit(data_dir, format!("rotation: {name} rotated by {actor}, {who} no longer holds a wrap"));
                rep.rotated.push(name);
            }
            Ok(false) => {
                if link.configured() {
                    audit(data_dir, format!("rotation: {name} needs none for {who} (no scope in the vault, or no wrap for them)"));
                }
                rep.nothing.push(name);
            }
            Err(e) => {
                audit(data_dir, format!("rotation pending: {name} for {who} (by {actor}): {}", short(&e)));
                set_pending(&mut list, s, who, &e);
                dirty = true;
                rep.pending.push(name);
            }
        }
        let before = list.len();
        if !rep.pending.contains(&s.name()) {
            list.retain(|p| !(p.scope == s.name() && p.who == who));
            dirty |= list.len() != before;
        }
    }
    if dirty {
        if let Err(e) = save_pending(data_dir, &list) {
            log::error!("rotation: the pending list could not be saved: {e}");
        }
    }
    rep
}

/// Try again for every pending entry with the keys of each of `actors` in turn; the first that works settles the entry. Entries nobody
/// could settle stay (with the last reason).
pub fn retry_pending(data_dir: &Path, link: &Link, actors: &[(String, MemberKeys)]) -> Result<Report, String> {
    let list = load_pending(data_dir)?;
    let mut rep = Report::default();
    let mut left = Vec::new();
    for p in list {
        let Some(scope) = Scope::parse(&p.scope) else {
            // an entry that is not a scope is junk; keep it visible rather than drop it silently
            rep.pending.push(p.scope.clone());
            left.push(p);
            continue;
        };
        let mut last = p.reason.clone();
        let mut done = None;
        for (actor, keys) in actors {
            let res = match link {
                Link::NotConfigured => Err("no vault is configured".to_string()),
                Link::Invalid(e) => Err(format!("the vault address is not usable: {e}")),
                Link::Target(t) => try_one(t, actor, Some(keys), &scope, &p.who),
            };
            match res {
                Ok(r) => {
                    done = Some((actor.clone(), r));
                    break;
                }
                Err(e) => last = short(&format!("{actor}: {e}")),
            }
        }
        match done {
            Some((actor, true)) => {
                audit(data_dir, format!("rotation: {} rotated by {actor} (was pending), {} no longer holds a wrap", p.scope, p.who));
                rep.rotated.push(p.scope);
            }
            Some((_, false)) => {
                audit(data_dir, format!("rotation: {} needs none for {} (was pending)", p.scope, p.who));
                rep.nothing.push(p.scope);
            }
            None => {
                rep.pending.push(p.scope.clone());
                left.push(Pending { reason: last, ..p });
            }
        }
    }
    save_pending(data_dir, &left)?;
    Ok(rep)
}

/// `iyakki cloud reconcile --retry-rotations --tenant-data DIR [--vault URL] [--vault-fingerprint FP] [--tenant NAME] --as USER --vault-key FILE`
/// (the address, pin and tenant default to the `IYAKKI_VAULT_*` and `IYAKKI_TENANT` variables).
pub fn retry_command(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let opt = |n: &str| a.iter().position(|x| x == n).and_then(|i| a.get(i + 1)).cloned();
    let usage = "usage: iyakki cloud reconcile --retry-rotations --tenant-data DIR --as USER --vault-key FILE [--vault URL] [--vault-fingerprint FP] [--tenant NAME]";
    let dir = opt("--tenant-data").ok_or(usage)?;
    let (user, kf) = (opt("--as").ok_or(usage)?, opt("--vault-key").ok_or(usage)?);
    let env = |k: &str| std::env::var(k).ok();
    let link = Link::from_values(
        opt("--vault").or_else(|| env(crate::cloudvaultlink::ENV_URL)).as_deref(),
        opt("--vault-fingerprint").or_else(|| env(crate::cloudvaultlink::ENV_FINGERPRINT)).as_deref(),
        opt("--tenant").or_else(|| env(crate::cloudvaultlink::ENV_TENANT)).as_deref(),
    );
    let keys = MemberKeys::from_hex(&std::fs::read_to_string(&kf).map_err(|e| format!("{kf}: {e}"))?)?;
    let n = load_pending(Path::new(&dir))?.len();
    let rep = retry_pending(Path::new(&dir), &link, &[(user, keys)])?;
    out(&format!("rotations pending before: {n}; rotated: {}; nothing to do: {}; still pending: {}", rep.rotated.len(), rep.nothing.len(), rep.pending.len()));
    for s in &rep.pending {
        out(&format!("  still pending: {s} (see {})", pending_path(Path::new(&dir)).display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudaccess::{Access, Revoke};
    use crate::cloudroles::{NsRole, TenantRole};
    use crate::cloudvaultd::{start, Client, Principal, Running, Store};

    fn keys(n: u8) -> MemberKeys {
        MemberKeys::from_seed([n; 32])
    }

    struct Fx {
        dir: PathBuf,
        run: Running,
        /// the tenant's own data directory (the same files the vault service reads its lists from)
        tdir: PathBuf,
    }

    impl Fx {
        fn client(&self, n: u8, u: &str) -> Client {
            Client { host: "127.0.0.1".into(), port: self.run.local_addr().port(), fingerprint: Some(self.run.fingerprint.clone()), who: Principal::Member { tenant: "acme".into(), user: u.into() }, keys: keys(n) }
        }

        fn link(&self) -> Link {
            Link::from_values(Some(&format!("https://127.0.0.1:{}", self.run.local_addr().port())), Some(&self.run.fingerprint), Some("acme"))
        }

        fn access(&self) -> Access {
            Access::new(&self.tdir)
        }

        fn audit(&self) -> String {
            std::fs::read_to_string(cloudroles::audit_path(&self.tdir)).unwrap_or_default()
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            self.run.stop();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// acme: alice owner (administers dev and ops), bob member (editor in dev), carol member, dave admin (in no namespace). Scopes in the vault:
    /// tenant (alice, bob, dave), namespace dev (alice, bob), namespace ops (alice).
    fn fixture(tag: &str) -> Fx {
        let dir = crate::testdir::unique("rotate", tag);
        let s = Store::open(&dir).unwrap();
        let tdir = dir.join("tenants").join("acme");
        let mut r = Roles::new("alice").unwrap();
        r.add_tenant_member("alice", "bob", TenantRole::Member).unwrap();
        r.add_tenant_member("alice", "carol", TenantRole::Member).unwrap();
        r.add_tenant_member("alice", "dave", TenantRole::Admin).unwrap();
        r.create_namespace("alice", "dev").unwrap();
        r.create_namespace("alice", "ops").unwrap();
        r.add_namespace_member("alice", "dev", "bob", NsRole::Editor).unwrap();
        cloudroles::save(&cloudroles::path(&tdir), &r).unwrap();
        for (n, u) in [(1u8, "alice"), (2, "bob"), (3, "carol"), (4, "dave")] {
            s.enroll(&Principal::Member { tenant: "acme".into(), user: u.into() }, &keys(n).public()).unwrap();
        }
        drop(s);
        let run = start("127.0.0.1:0".parse().unwrap(), &dir, None).unwrap();
        let fx = Fx { dir, run, tdir };
        let alice = fx.client(1, "alice");
        for s in [Scope::Tenant, Scope::Namespace("dev".into()), Scope::Namespace("ops".into())] {
            alice.create_scope(&s).unwrap();
            alice.put(&s, "token", b"value-one").unwrap();
        }
        alice.add_member(&Scope::Tenant, "bob").unwrap();
        alice.add_member(&Scope::Tenant, "dave").unwrap();
        alice.add_member(&Scope::Namespace("dev".into()), "bob").unwrap();
        fx
    }

    fn generation(fx: &Fx, s: &Scope) -> u64 {
        fx.client(1, "alice").scope_key(s).unwrap().0
    }

    #[test]
    fn removing_a_namespace_member_rotates_that_namespace_and_cuts_them_off() {
        let fx = fixture("ns");
        let dev = Scope::Namespace("dev".into());
        let bob = fx.client(2, "bob");
        assert_eq!(bob.get(&dev, "token").unwrap(), b"value-one");
        let (c, rep) = fx.access().revoke("alice", &Revoke::Namespace { ns: "dev".into(), who: "bob".into() }, &fx.link(), Some(&keys(1))).unwrap();
        assert!(c.0.contains("removed bob"));
        assert_eq!(rep, Report { rotated: vec!["namespace:dev".into()], ..Report::default() });
        assert!(bob.get(&dev, "token").unwrap_err().starts_with("403"));
        assert_eq!(fx.client(1, "alice").get(&dev, "token").unwrap(), b"value-one", "old values survive");
        assert_eq!(generation(&fx, &dev), 2);
        assert_eq!(generation(&fx, &Scope::Tenant), 1, "bob is still in the tenant");
        assert!(fx.audit().contains("rotation: namespace:dev rotated by alice"), "{}", fx.audit());
        assert!(load_pending(&fx.tdir).unwrap().is_empty());
    }

    #[test]
    fn removing_a_tenant_member_rotates_the_tenant_scope_and_each_namespace_they_were_in_only() {
        let fx = fixture("tenant");
        let (dev, ops) = (Scope::Namespace("dev".into()), Scope::Namespace("ops".into()));
        let (_, rep) = fx.access().revoke("alice", &Revoke::Tenant { who: "bob".into() }, &fx.link(), Some(&keys(1))).unwrap();
        assert_eq!(rep.rotated, ["tenant", "namespace:dev"]);
        assert!(rep.pending.is_empty());
        assert_eq!((generation(&fx, &Scope::Tenant), generation(&fx, &dev), generation(&fx, &ops)), (2, 2, 1), "ops never held bob");
        let bob = fx.client(2, "bob");
        for s in [&Scope::Tenant, &dev] {
            assert!(bob.get(s, "token").is_err(), "{s:?}");
        }
        assert!(fx.access().roles().unwrap().unwrap().tenant_role("bob").is_none(), "the role change stands");
    }

    #[test]
    fn without_a_key_the_removal_stands_the_rotation_is_pending_and_a_retry_finishes_it() {
        let fx = fixture("pending");
        let dev = Scope::Namespace("dev".into());
        let (_, rep) = fx.access().revoke("alice", &Revoke::Namespace { ns: "dev".into(), who: "bob".into() }, &fx.link(), None).unwrap();
        assert_eq!(rep.pending, ["namespace:dev"]);
        assert!(fx.access().roles().unwrap().unwrap().ns_role("dev", "bob").is_none());
        let p = load_pending(&fx.tdir).unwrap();
        assert_eq!((p[0].scope.as_str(), p[0].who.as_str()), ("namespace:dev", "bob"));
        assert!(fx.audit().contains("rotation pending: namespace:dev for bob"), "{}", fx.audit());
        assert!(!fx.audit().contains(&keys(1).seed_hex()) && !std::fs::read_to_string(pending_path(&fx.tdir)).unwrap().contains("value-one"));
        assert_eq!(generation(&fx, &dev), 1, "nothing was rotated");
        // a retry by someone who cannot (carol holds no wrap in dev, nor any role there) leaves it pending
        let rep = retry_pending(&fx.tdir, &fx.link(), &[("carol".into(), keys(3))]).unwrap();
        assert_eq!(rep.pending, ["namespace:dev"]);
        assert_eq!(load_pending(&fx.tdir).unwrap().len(), 1);
        // alice retries: done, the file is empty, bob is cut off
        let rep = retry_pending(&fx.tdir, &fx.link(), &[("carol".into(), keys(3)), ("alice".into(), keys(1))]).unwrap();
        assert_eq!(rep.rotated, ["namespace:dev"]);
        assert!(load_pending(&fx.tdir).unwrap().is_empty());
        assert!(fx.client(2, "bob").get(&dev, "token").unwrap_err().starts_with("403"));
        assert!(fx.audit().contains("was pending"));
        // and the command form reads the secret from a file
        let kf = fx.dir.join("alice.key");
        std::fs::write(&kf, keys(1).seed_hex()).unwrap();
        let mut said = Vec::new();
        retry_command(&["--tenant-data".into(), fx.tdir.display().to_string(), "--as".into(), "alice".into(), "--vault-key".into(), kf.display().to_string(), "--vault".into(), format!("https://127.0.0.1:{}", fx.run.local_addr().port()), "--vault-fingerprint".into(), fx.run.fingerprint.clone(), "--tenant".into(), "acme".into()], &mut |l| said.push(l.to_string())).unwrap();
        assert!(said[0].contains("pending before: 0"), "{said:?}");
        assert!(retry_command(&[], &mut |_| {}).unwrap_err().contains("usage"));
    }

    #[test]
    fn an_unreachable_or_misconfigured_vault_leaves_the_removal_and_a_pending_entry() {
        let fx = fixture("down");
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let dead = Link::from_values(Some(&format!("https://127.0.0.1:{port}")), None, Some("acme"));
        let (_, rep) = fx.access().revoke("alice", &Revoke::Tenant { who: "bob".into() }, &dead, Some(&keys(1))).unwrap();
        assert_eq!(rep.pending, ["tenant", "namespace:dev"]);
        assert_eq!(load_pending(&fx.tdir).unwrap().len(), 2);
        // still down on retry: still pending, one entry each, the reason kept
        let rep = retry_pending(&fx.tdir, &dead, &[("alice".into(), keys(1))]).unwrap();
        assert_eq!((rep.rotated.len(), rep.pending.len()), (0, 2));
        // a bad address and a missing tenant name are pending as well, never a panic
        let bad = Link::from_values(Some("http://plain"), None, Some("acme"));
        let rep = retry_pending(&fx.tdir, &bad, &[("alice".into(), keys(1))]).unwrap();
        assert_eq!(rep.pending.len(), 2);
        let no_tenant = Link::from_values(Some(&format!("https://127.0.0.1:{}", fx.run.local_addr().port())), Some(&fx.run.fingerprint), None);
        assert_eq!(retry_pending(&fx.tdir, &no_tenant, &[("alice".into(), keys(1))]).unwrap().pending.len(), 2);
        // the vault comes back: the retry settles both
        let rep = retry_pending(&fx.tdir, &fx.link(), &[("alice".into(), keys(1))]).unwrap();
        assert_eq!(rep.rotated, ["tenant", "namespace:dev"]);
        assert!(load_pending(&fx.tdir).unwrap().is_empty());
    }

    #[test]
    fn an_administrator_outside_a_namespace_rotates_what_they_can_and_leaves_the_rest_pending() {
        let fx = fixture("admin");
        // dave (tenant admin, holds the tenant wrap, not in dev) removes bob from the tenant
        let (_, rep) = fx.access().revoke("dave", &Revoke::Tenant { who: "bob".into() }, &fx.link(), Some(&keys(4))).unwrap();
        assert_eq!(rep.rotated, ["tenant"]);
        assert_eq!(rep.pending, ["namespace:dev"], "dave has no part in dev: the vault refuses, alice finishes it");
        let p = load_pending(&fx.tdir).unwrap();
        assert!(p[0].reason.contains("403"), "{p:?}");
        let rep = retry_pending(&fx.tdir, &fx.link(), &[("dave".into(), keys(4)), ("alice".into(), keys(1))]).unwrap();
        assert_eq!(rep.rotated, ["namespace:dev"]);
    }

    #[test]
    fn denied_changes_rotate_nothing_and_record_nothing() {
        let fx = fixture("denied");
        let before = generation(&fx, &Scope::Tenant);
        // a plain member cannot remove anybody; an editor cannot remove from the namespace; a stranger is nobody
        for (by, what) in [
            ("carol", Revoke::Tenant { who: "bob".into() }),
            ("bob", Revoke::Namespace { ns: "dev".into(), who: "alice".into() }),
            ("mallory", Revoke::Tenant { who: "bob".into() }),
            ("dave", Revoke::Tenant { who: "alice".into() }),
            ("alice", Revoke::Tenant { who: "alice".into() }),
            ("alice", Revoke::Namespace { ns: "dev".into(), who: "carol".into() }),
        ] {
            assert!(fx.access().revoke(by, &what, &fx.link(), Some(&keys(1))).is_err(), "{by} {what:?}");
        }
        assert_eq!(generation(&fx, &Scope::Tenant), before);
        assert!(load_pending(&fx.tdir).unwrap().is_empty());
        assert!(!fx.audit().contains("rotation"), "{}", fx.audit());
        assert!(fx.access().roles().unwrap().unwrap().tenant_role("bob").is_some());
    }

    #[test]
    fn no_vault_configured_means_nothing_to_rotate_and_nothing_pending() {
        let fx = fixture("none");
        let (_, rep) = fx.access().revoke("alice", &Revoke::Tenant { who: "carol".into() }, &Link::NotConfigured, None).unwrap();
        assert_eq!(rep.nothing, ["tenant"]);
        assert!(load_pending(&fx.tdir).unwrap().is_empty());
    }

    #[test]
    fn a_scope_missing_from_the_vault_or_a_person_without_a_wrap_needs_no_rotation() {
        let fx = fixture("nothing");
        // carol was never given a wrap anywhere; the tenant scope exists, dev does not hold her
        let (_, rep) = fx.access().revoke("alice", &Revoke::Tenant { who: "carol".into() }, &fx.link(), Some(&keys(1))).unwrap();
        assert_eq!(rep.nothing, ["tenant"]);
        assert_eq!(generation(&fx, &Scope::Tenant), 1);
        // a namespace that has no scope in the vault
        let mut r = fx.access().roles().unwrap().unwrap();
        r.create_namespace("alice", "docs").unwrap();
        r.add_namespace_member("alice", "docs", "dave", NsRole::Viewer).unwrap();
        cloudroles::save(&cloudroles::path(&fx.tdir), &r).unwrap();
        let (_, rep) = fx.access().revoke("alice", &Revoke::Namespace { ns: "docs".into(), who: "dave".into() }, &fx.link(), Some(&keys(1))).unwrap();
        assert_eq!(rep.nothing, ["namespace:docs"]);
        assert!(load_pending(&fx.tdir).unwrap().is_empty());
    }

    #[test]
    fn keys_are_read_only_for_plain_names_from_the_volume() {
        let d = crate::testdir::unique("rotate", "keys");
        assert!(keys_path(&d, "../x").is_none() && actor_keys(&d, "../x").is_none());
        assert!(actor_keys(&d, "alice").is_none());
        let p = keys_path(&d, "alice").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not hex").unwrap();
        assert!(actor_keys(&d, "alice").is_none());
        std::fs::write(&p, keys(1).seed_hex()).unwrap();
        assert_eq!(actor_keys(&d, "alice").unwrap().public(), keys(1).public());
        let _ = std::fs::remove_dir_all(&d);
    }
}

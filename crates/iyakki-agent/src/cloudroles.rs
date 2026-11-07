//! Who may do what in a cloud tenant (docs/development/CLOUD-MULTITENANT-PLAN.md, section 3a). Deny by default, checked by the server for
//! every route and every vault read.
//!
//! * A person who is not a member of the tenant has no access to anything in it ([`Roles::can`] says `Denied(NotInTenant)`).
//! * A member of the tenant who is not a member of a namespace cannot open it, read its files or its vault. A tenant owner or admin can
//!   *see that it exists* (to add people) but not read it; they join it themselves, and that is recorded.
//! * Tenant roles: owner, admin, member. Namespace roles: admin, editor, viewer.
//!
//! [`Roles`] is plain data (it is saved as `roles.json` on the tenant's volume); every change returns the [`Change`] line the audit trail
//! keeps. Nothing here starts a process or reads a clock: the caller passes who is acting.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TenantRole {
    Member,
    Admin,
    Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NsRole {
    Viewer,
    Editor,
    Admin,
}

/// What a person asks to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Sign in to the tenant, see its name and the namespaces they are in.
    EnterTenant,
    /// Add or remove tenant members, change their roles.
    ManageTenantMembers,
    /// Create or delete a namespace.
    ManageNamespaces,
    /// See that a namespace exists (its name, who administers it), not its content.
    SeeNamespace(String),
    /// Open it, list and read its files, read its vault.
    ReadNamespace(String),
    /// Change its files, write its vault.
    WriteNamespace(String),
    /// Add or remove its members and change their roles.
    ManageNamespaceMembers(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    NotInTenant,
    NotInNamespace,
    NoSuchNamespace,
    RoleTooLow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Denied(Why),
}

impl Decision {
    pub fn ok(&self) -> bool {
        *self == Decision::Allowed
    }
}

/// One line for the audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    #[serde(default)]
    pub tenant: BTreeMap<String, TenantRole>,
    /// namespace -> person -> role.
    #[serde(default)]
    pub namespaces: BTreeMap<String, BTreeMap<String, NsRole>>,
    /// Named lists of people (no nesting): owner -> group -> people. The owner is a namespace, or [`TENANT_WIDE`] for groups every team may use.
    #[serde(default)]
    pub groups: BTreeMap<String, BTreeMap<String, std::collections::BTreeSet<String>>>,
    /// Approver roles, per namespace: scope (an environment such as `prod`, or a kind such as `k3s.cluster`) -> who holds it. Grants only
    /// "may approve requests of that scope in this team" and nothing else.
    #[serde(default)]
    pub approvers: BTreeMap<String, BTreeMap<String, Holders>>,
}

/// The owner key of groups every team may use.
pub const TENANT_WIDE: &str = "*";
pub const MAX_GROUPS: usize = 50;
pub const MAX_GROUP_SIZE: usize = 200;
pub const MAX_HOLDERS: usize = 50;
pub const MAX_EXPANDED: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Holders {
    #[serde(default)]
    pub people: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub groups: std::collections::BTreeSet<String>,
}

fn plain(s: &str) -> bool {
    crate::cloudstorage::plain_name(s)
}

impl Roles {
    /// A tenant that has just been made: its first owner, no namespaces.
    pub fn new(owner: &str) -> Result<Roles, String> {
        if !plain(owner) {
            return Err(format!("\"{owner}\" is not a valid account name"));
        }
        let mut r = Roles::default();
        r.tenant.insert(owner.into(), TenantRole::Owner);
        Ok(r)
    }

    pub fn parse(text: &str) -> Result<Roles, String> {
        serde_json::from_str(text).map_err(|e| format!("roles: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn tenant_role(&self, who: &str) -> Option<TenantRole> {
        self.tenant.get(who).copied()
    }

    pub fn ns_role(&self, ns: &str, who: &str) -> Option<NsRole> {
        self.namespaces.get(ns).and_then(|m| m.get(who)).copied()
    }

    /// The decision for `who` doing `what`. Anything not listed is denied.
    pub fn can(&self, who: &str, what: &Action) -> Decision {
        use Decision::*;
        use Why::*;
        let Some(t) = self.tenant_role(who) else { return Denied(NotInTenant) };
        let admin = t >= TenantRole::Admin;
        let ns_exists = |ns: &str| self.namespaces.contains_key(ns);
        match what {
            Action::EnterTenant => Allowed,
            Action::ManageTenantMembers | Action::ManageNamespaces => {
                if admin {
                    Allowed
                } else {
                    Denied(RoleTooLow)
                }
            }
            Action::SeeNamespace(ns) => {
                if !ns_exists(ns) {
                    Denied(NoSuchNamespace)
                } else if admin || self.ns_role(ns, who).is_some() {
                    Allowed
                } else {
                    // a plain member does not even learn the name
                    Denied(NotInNamespace)
                }
            }
            Action::ReadNamespace(ns) => match (ns_exists(ns), self.ns_role(ns, who)) {
                (false, _) => Denied(NoSuchNamespace),
                (true, Some(_)) => Allowed,
                (true, None) => Denied(NotInNamespace),
            },
            Action::WriteNamespace(ns) => match (ns_exists(ns), self.ns_role(ns, who)) {
                (false, _) => Denied(NoSuchNamespace),
                (true, Some(r)) if r >= NsRole::Editor => Allowed,
                (true, Some(_)) => Denied(RoleTooLow),
                (true, None) => Denied(NotInNamespace),
            },
            Action::ManageNamespaceMembers(ns) => match (ns_exists(ns), self.ns_role(ns, who)) {
                (false, _) => Denied(NoSuchNamespace),
                (true, Some(NsRole::Admin)) => Allowed,
                (true, Some(_)) => Denied(RoleTooLow),
                (true, None) => Denied(NotInNamespace),
            },
        }
    }

    /// The namespaces `who` is shown: the ones they are in; for a tenant admin or owner also the others' names (to add people), marked
    /// as not joined. Empty for a person who is not in the tenant.
    pub fn visible_namespaces(&self, who: &str) -> Vec<(String, Option<NsRole>)> {
        let Some(t) = self.tenant_role(who) else { return Vec::new() };
        self.namespaces
            .iter()
            .filter_map(|(n, m)| {
                let mine = m.get(who).copied();
                (mine.is_some() || t >= TenantRole::Admin).then(|| (n.clone(), mine))
            })
            .collect()
    }

    /// A team's admin, or a tenant admin or owner, may manage the team's groups and approver roles.
    pub fn can_manage_team(&self, who: &str, ns: &str) -> bool {
        self.ns_role(ns, who) == Some(NsRole::Admin) || self.tenant_role(who).is_some_and(|t| t >= TenantRole::Admin)
    }

    fn manage_owner(&self, by: &str, owner: &str) -> Result<(), String> {
        let ok = if owner == TENANT_WIDE { self.tenant_role(by).is_some_and(|t| t >= TenantRole::Admin) } else { self.namespaces.contains_key(owner) && self.can_manage_team(by, owner) };
        if ok {
            Ok(())
        } else if self.tenant_role(by).is_none() {
            Err("no access".into())
        } else {
            Err("not allowed (team administrators and tenant administrators only)".into())
        }
    }

    /// Set the members of a group (people only: a group cannot contain a group). Audited with who was added and removed.
    pub fn set_group(&mut self, by: &str, owner: &str, group: &str, members: &[String]) -> Result<Change, String> {
        self.manage_owner(by, owner)?;
        if !plain(group) || (owner != TENANT_WIDE && !plain(owner)) {
            return Err("a group name is letters, digits, - and _".into());
        }
        let mut set = std::collections::BTreeSet::new();
        for m in members {
            if !plain(m) || self.tenant_role(m).is_none() {
                return Err(format!("{m} is not a person in this tenant (groups hold people, not other groups)"));
            }
            set.insert(m.clone());
        }
        if set.len() > MAX_GROUP_SIZE {
            return Err(format!("a group has at most {MAX_GROUP_SIZE} people"));
        }
        let g = self.groups.entry(owner.into()).or_default();
        if !g.contains_key(group) && g.len() >= MAX_GROUPS {
            return Err(format!("at most {MAX_GROUPS} groups"));
        }
        let old = g.get(group).cloned().unwrap_or_default();
        let added: Vec<&String> = set.difference(&old).collect();
        let removed: Vec<&String> = old.difference(&set).collect();
        let line = format!("group {owner}/{group}: {by} added [{}] removed [{}]", added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","), removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","));
        g.insert(group.into(), set);
        Ok(Change(line))
    }

    pub fn delete_group(&mut self, by: &str, owner: &str, group: &str) -> Result<Change, String> {
        self.manage_owner(by, owner)?;
        let g = self.groups.get_mut(owner).ok_or("no such group")?;
        g.remove(group).ok_or("no such group")?;
        if g.is_empty() {
            self.groups.remove(owner);
        }
        for (ns, m) in self.approvers.iter_mut() {
            if owner == TENANT_WIDE || ns == owner {
                for h in m.values_mut() {
                    h.groups.remove(group);
                }
            }
        }
        Ok(Change(format!("group {owner}/{group}: {by} deleted it")))
    }

    fn scope_ok(s: &str) -> bool {
        !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !s.starts_with('.')
    }

    /// Give (`grant`) or take away the approver role for `scope` in team `ns` to people and groups (a group of the team or a tenant-wide one).
    pub fn change_approvers(&mut self, by: &str, ns: &str, scope: &str, people: &[String], groups: &[String], grant: bool) -> Result<Change, String> {
        self.manage_owner(by, ns)?;
        if ns == TENANT_WIDE || !Self::scope_ok(scope) {
            return Err("a scope is an environment or kind name".into());
        }
        for p in people {
            if !plain(p) || self.tenant_role(p).is_none() {
                return Err(format!("{p} is not a person in this tenant"));
            }
        }
        for g in groups {
            let known = self.groups.get(ns).is_some_and(|m| m.contains_key(g)) || self.groups.get(TENANT_WIDE).is_some_and(|m| m.contains_key(g));
            if grant && !known {
                return Err(format!("no group {g} in this team or tenant-wide"));
            }
        }
        let h = self.approvers.entry(ns.into()).or_default().entry(scope.into()).or_default();
        for p in people {
            if grant { h.people.insert(p.clone()); } else { h.people.remove(p); }
        }
        for g in groups {
            if grant { h.groups.insert(g.clone()); } else { h.groups.remove(g); }
        }
        if h.people.len() > MAX_HOLDERS || h.groups.len() > MAX_HOLDERS {
            return Err(format!("at most {MAX_HOLDERS} people and {MAX_HOLDERS} groups per approver role"));
        }
        if h.people.is_empty() && h.groups.is_empty() {
            if let Some(m) = self.approvers.get_mut(ns) {
                m.remove(scope);
            }
        }
        let verb = if grant { "granted" } else { "revoked" };
        Ok(Change(format!("approver {ns}/{scope}: {by} {verb} people [{}] groups [{}]", people.join(","), groups.join(","))))
    }

    /// The people who hold the approver role for any of `scopes` in `ns` right now: groups expanded, people who left the tenant dropped,
    /// duplicates collapsed, bounded.
    pub fn approver_holders(&self, ns: &str, scopes: &[&str]) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let Some(m) = self.approvers.get(ns) else { return out };
        for s in scopes {
            let Some(h) = m.get(*s) else { continue };
            out.extend(h.people.iter().cloned());
            for g in &h.groups {
                let members = self.groups.get(ns).and_then(|x| x.get(g)).or_else(|| self.groups.get(TENANT_WIDE).and_then(|x| x.get(g)));
                out.extend(members.into_iter().flatten().cloned());
            }
        }
        out.retain(|p| self.tenant_role(p).is_some());
        while out.len() > MAX_EXPANDED {
            let last = out.iter().next_back().cloned();
            if let Some(l) = last { out.remove(&l); }
        }
        out
    }

    fn need(&self, by: &str, what: Action) -> Result<(), String> {
        match self.can(by, &what) {
            Decision::Allowed => Ok(()),
            Decision::Denied(Why::NotInTenant) => Err("no access".into()),
            Decision::Denied(w) => Err(format!("not allowed ({w:?})")),
        }
    }

    // ---- changes: each checks who is acting and returns the audit line -------------------------------------------------------------

    pub fn add_tenant_member(&mut self, by: &str, who: &str, role: TenantRole) -> Result<Change, String> {
        self.need(by, Action::ManageTenantMembers)?;
        if !plain(who) {
            return Err(format!("\"{who}\" is not a valid account name"));
        }
        if role == TenantRole::Owner && self.tenant_role(by) != Some(TenantRole::Owner) {
            return Err("only an owner makes an owner".into());
        }
        if self.tenant_role(who).is_some_and(|r| r > self.tenant_role(by).unwrap_or(TenantRole::Member)) {
            return Err("that person has a higher role than yours".into());
        }
        self.tenant.insert(who.into(), role);
        Ok(Change(format!("tenant: {by} made {who} {role:?}")))
    }

    /// Removing a member takes them out of every namespace too. The last owner cannot be removed.
    pub fn remove_tenant_member(&mut self, by: &str, who: &str) -> Result<Change, String> {
        self.need(by, Action::ManageTenantMembers)?;
        let Some(role) = self.tenant_role(who) else { return Err(format!("{who} is not a member")) };
        if role > self.tenant_role(by).unwrap_or(TenantRole::Member) {
            return Err("that person has a higher role than yours".into());
        }
        if role == TenantRole::Owner && self.tenant.values().filter(|r| **r == TenantRole::Owner).count() == 1 {
            return Err("the last owner cannot be removed".into());
        }
        self.tenant.remove(who);
        for m in self.namespaces.values_mut() {
            m.remove(who);
        }
        Ok(Change(format!("tenant: {by} removed {who} (and from every namespace; their keys are to be rotated, sessions ended)")))
    }

    pub fn create_namespace(&mut self, by: &str, ns: &str) -> Result<Change, String> {
        self.need(by, Action::ManageNamespaces)?;
        if !plain(ns) {
            return Err(format!("\"{ns}\" is not a valid namespace name"));
        }
        if self.namespaces.contains_key(ns) {
            return Err(format!("namespace {ns} exists"));
        }
        // the maker administers it: no namespace is left without a member who can add others
        self.namespaces.entry(ns.into()).or_default().insert(by.into(), NsRole::Admin);
        Ok(Change(format!("namespace {ns}: made by {by}, who administers it")))
    }

    pub fn delete_namespace(&mut self, by: &str, ns: &str) -> Result<Change, String> {
        self.need(by, Action::ManageNamespaces)?;
        if self.namespaces.remove(ns).is_none() {
            return Err(format!("no namespace {ns}"));
        }
        Ok(Change(format!("namespace {ns}: deleted by {by}")))
    }

    /// Add `who` (a member of the tenant) to a namespace. A namespace's administrator does it; a tenant admin or owner who is not in the
    /// namespace may add *themselves* (the join is recorded as such), not anyone else.
    pub fn add_namespace_member(&mut self, by: &str, ns: &str, who: &str, role: NsRole) -> Result<Change, String> {
        let t = self.tenant_role(by).ok_or("no access")?;
        if !self.namespaces.contains_key(ns) {
            return Err(format!("no namespace {ns}"));
        }
        if self.tenant_role(who).is_none() {
            return Err(format!("{who} is not a member of the tenant; add them to the tenant first"));
        }
        let by_ns = self.ns_role(ns, by);
        let line = if by_ns == Some(NsRole::Admin) {
            format!("namespace {ns}: {by} made {who} {role:?}")
        } else if by_ns.is_none() && by == who && t >= TenantRole::Admin {
            format!("namespace {ns}: tenant {t:?} {by} JOINED it as {role:?} (was not a member)")
        } else {
            return Err("not allowed (only the namespace's administrators add people)".into());
        };
        self.namespaces.get_mut(ns).map(|m| m.insert(who.into(), role));
        Ok(Change(line))
    }

    pub fn remove_namespace_member(&mut self, by: &str, ns: &str, who: &str) -> Result<Change, String> {
        self.need(by, Action::ManageNamespaceMembers(ns.into()))?;
        let m = self.namespaces.get_mut(ns).ok_or_else(|| format!("no namespace {ns}"))?;
        let Some(role) = m.get(who).copied() else { return Err(format!("{who} is not in {ns}")) };
        if role == NsRole::Admin && m.values().filter(|r| **r == NsRole::Admin).count() == 1 {
            return Err("the last administrator of a namespace cannot be removed".into());
        }
        m.remove(who);
        Ok(Change(format!("namespace {ns}: {by} removed {who} (the namespace's key is to be rotated, their sessions there ended)")))
    }
}

// ------------------------------------------------------------------------------------------------------------------ the file

/// Where a tenant's lists are kept, under the data directory of its volume (`/var/lib/iyakki`).
pub fn path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("cloud").join("roles.json")
}

/// The audit trail of changes, one line each, next to the lists.
pub fn audit_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("cloud").join("roles-audit.log")
}

/// The lists in `file`, `None` when there is no file yet (a tenant whose first owner has not been made).
pub fn load(file: &std::path::Path) -> Result<Option<Roles>, String> {
    match std::fs::read_to_string(file) {
        Ok(t) => Roles::parse(&t).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// Write the lists whole and atomically (a temporary file, then a rename), readable by the owner only.
pub fn save(file: &std::path::Path, r: &Roles) -> Result<(), String> {
    let dir = file.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, r.to_json()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))
}

/// Append one line of the audit trail: `unix-seconds <TAB> change`. A line never holds a newline of its own.
pub fn append_audit(file: &std::path::Path, now: u64, c: &Change) -> Result<(), String> {
    use std::io::Write;
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    writeln!(f, "{now}\t{}", c.0.replace(['\n', '\r'], " ")).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// acme: owner `olga`, admin `adam`, members `mia`, `max`; namespace `web` (admin `mia`, editor `max`) and `data` (admin `olga`).
    fn acme() -> Roles {
        let mut r = Roles::new("olga").unwrap();
        r.add_tenant_member("olga", "adam", TenantRole::Admin).unwrap();
        r.add_tenant_member("olga", "mia", TenantRole::Member).unwrap();
        r.add_tenant_member("adam", "max", TenantRole::Member).unwrap();
        r.create_namespace("adam", "web").unwrap();
        r.add_namespace_member("adam", "web", "mia", NsRole::Admin).unwrap();
        r.add_namespace_member("mia", "web", "max", NsRole::Editor).unwrap();
        r.create_namespace("olga", "data").unwrap();
        r
    }

    #[test]
    fn a_stranger_has_no_access_to_anything() {
        let r = acme();
        for a in [Action::EnterTenant, Action::ManageTenantMembers, Action::SeeNamespace("web".into()), Action::ReadNamespace("web".into()), Action::WriteNamespace("web".into())] {
            assert_eq!(r.can("eve", &a), Decision::Denied(Why::NotInTenant), "{a:?}");
        }
        assert!(r.visible_namespaces("eve").is_empty());
    }

    #[test]
    fn a_tenant_member_without_the_namespace_cannot_open_it_or_even_see_it() {
        let r = acme();
        assert!(r.can("mia", &Action::EnterTenant).ok());
        assert_eq!(r.can("mia", &Action::ReadNamespace("data".into())), Decision::Denied(Why::NotInNamespace));
        assert_eq!(r.can("mia", &Action::SeeNamespace("data".into())), Decision::Denied(Why::NotInNamespace));
        assert_eq!(r.visible_namespaces("mia"), vec![("web".to_string(), Some(NsRole::Admin))], "only the namespace they are in");
    }

    #[test]
    fn a_tenant_admin_sees_the_names_but_cannot_read_a_namespace_they_are_not_in() {
        let r = acme();
        assert!(r.can("olga", &Action::SeeNamespace("web".into())).ok());
        assert_eq!(r.can("olga", &Action::ReadNamespace("web".into())), Decision::Denied(Why::NotInNamespace));
        assert_eq!(r.can("adam", &Action::WriteNamespace("data".into())), Decision::Denied(Why::NotInNamespace));
        assert_eq!(r.visible_namespaces("adam").len(), 2, "both names, to add people");
        assert_eq!(r.visible_namespaces("adam").iter().filter(|(_, m)| m.is_none()).count(), 1, "data is marked not joined");
    }

    #[test]
    fn namespace_roles_decide_read_write_and_managing() {
        let r = acme();
        let ns = || "web".to_string();
        assert!(r.can("max", &Action::ReadNamespace(ns())).ok() && r.can("max", &Action::WriteNamespace(ns())).ok());
        assert_eq!(r.can("max", &Action::ManageNamespaceMembers(ns())), Decision::Denied(Why::RoleTooLow));
        assert!(r.can("mia", &Action::ManageNamespaceMembers(ns())).ok());
        let mut r = r;
        r.add_namespace_member("mia", "web", "max", NsRole::Viewer).unwrap();
        assert!(r.can("max", &Action::ReadNamespace(ns())).ok());
        assert_eq!(r.can("max", &Action::WriteNamespace(ns())), Decision::Denied(Why::RoleTooLow), "a viewer reads only");
        assert_eq!(r.can("mia", &Action::ReadNamespace("nope".into())), Decision::Denied(Why::NoSuchNamespace));
    }

    #[test]
    fn only_tenant_admins_manage_members_and_nobody_raises_beyond_themselves() {
        let mut r = acme();
        assert!(r.add_tenant_member("mia", "zed", TenantRole::Member).is_err(), "a member cannot add");
        assert!(r.add_tenant_member("adam", "zed", TenantRole::Owner).is_err(), "an admin cannot make an owner");
        assert!(r.add_tenant_member("adam", "olga", TenantRole::Member).is_err(), "nor lower an owner");
        assert!(r.add_tenant_member("olga", "../x", TenantRole::Member).is_err(), "names are checked");
        assert!(r.remove_tenant_member("adam", "olga").is_err());
        assert!(r.remove_tenant_member("olga", "olga").is_err(), "the last owner stays");
    }

    #[test]
    fn removing_a_member_removes_them_from_every_namespace_and_the_line_says_keys_rotate() {
        let mut r = acme();
        let c = r.remove_tenant_member("adam", "max").unwrap();
        assert!(c.0.contains("rotated"), "{}", c.0);
        assert!(r.ns_role("web", "max").is_none() && r.tenant_role("max").is_none());
        assert_eq!(r.can("max", &Action::ReadNamespace("web".into())), Decision::Denied(Why::NotInTenant));
    }

    #[test]
    fn a_tenant_admin_joins_a_namespace_only_themselves_and_it_is_recorded() {
        let mut r = acme();
        assert!(r.add_namespace_member("adam", "data", "mia", NsRole::Editor).is_err(), "not for someone else");
        let c = r.add_namespace_member("adam", "data", "adam", NsRole::Viewer).unwrap();
        assert!(c.0.contains("JOINED"), "{}", c.0);
        assert!(r.can("adam", &Action::ReadNamespace("data".into())).ok());
        assert!(r.add_namespace_member("mia", "data", "mia", NsRole::Admin).is_err(), "a plain member cannot join");
        assert!(r.add_namespace_member("olga", "data", "stranger", NsRole::Viewer).is_err(), "must be in the tenant first");
    }

    #[test]
    fn the_last_administrator_of_a_namespace_stays_and_the_maker_administers_it() {
        let mut r = acme();
        assert_eq!(r.ns_role("data", "olga"), Some(NsRole::Admin));
        assert!(r.remove_namespace_member("olga", "data", "olga").is_err());
        r.add_namespace_member("olga", "data", "adam", NsRole::Admin).unwrap();
        assert!(r.remove_namespace_member("adam", "data", "olga").is_ok(), "with a second administrator the first may go");
        assert!(r.remove_namespace_member("adam", "data", "adam").is_err(), "and now adam is the last");
        assert!(r.remove_namespace_member("max", "web", "mia").is_err(), "an editor cannot remove");
    }

    #[test]
    fn the_lists_survive_a_save_and_unknown_keys_are_refused() {
        let r = acme();
        assert_eq!(Roles::parse(&r.to_json()).unwrap(), r);
        assert!(Roles::parse(r#"{"surprise":1}"#).is_err());
    }

    #[test]
    fn the_lists_are_saved_atomically_loaded_back_and_the_audit_trail_appends_lines() {
        let d = std::env::temp_dir().join(format!("iyakki-cloudroles-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(load(&path(&d)).unwrap(), None, "no file yet");
        let r = acme();
        save(&path(&d), &r).unwrap();
        assert_eq!(load(&path(&d)).unwrap(), Some(r));
        assert!(!path(&d).with_extension("json.tmp").exists(), "the temporary file is gone");
        append_audit(&audit_path(&d), 100, &Change("tenant: olga made x Member".into())).unwrap();
        append_audit(&audit_path(&d), 101, &Change("two\nlines".into())).unwrap();
        let t = std::fs::read_to_string(audit_path(&d)).unwrap();
        assert_eq!(t.lines().count(), 2, "{t}");
        assert!(t.starts_with("100\ttenant: olga"), "{t}");
        std::fs::write(path(&d), "{ not json").unwrap();
        assert!(load(&path(&d)).is_err());
    }
}

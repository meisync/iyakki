//! Who may do what is the core's to say, never the UI's (D-SEC1-VOUCH, alpha.7 SEC1; the same rule as D-SPLIT-03-POLICY, kept in one place).
//!
//! A served call may carry an argument that CLAIMS a right: a policy, a role, a session, an "is administrator" flag, the list of administrators,
//! the "may keep own / shared skills" flags. The UI program is a different process, so every one of those is only a claim. Before a call is
//! dispatched, [`vouch`] looks the call up in [`RULES`], takes the NAME of the account the call is about, reads that account from the core's own
//! accounts and writes the core's reading over the claim. A name the core does not know is refused and the call never runs. What the UI
//! sent in those places is never read by anything.
//!
//! Since alpha.8 (SEC3, `served_login.rs`) the account the call names is also held to the account signed in on the connection, the shared-scope
//! flags come from that account, and `approved` is a core-issued one-time ticket.
//!
//! The table is the one reviewed list. `served_auth_audit.rs` (tests) walks every served method and fails when a method takes an
//! authority-carrying argument that is neither in this table nor in the reviewed allow-list, so a new method cannot reopen the hole.

use serde_json::Value;

use crate::served::NOT_SERVED;
use crate::Registry;

/// What the core writes in place of an argument (the index counts the call's arguments, `self` not included).
pub(crate) enum Claim {
    /// A `Policy`: the account's own, resolved from its role and overrides.
    Policy(usize),
    /// A `Role`: the account's own.
    Role(usize),
    /// An "is this person an administrator" flag: true when the account's policy may manage the settings (what the shell sends).
    Admin(usize),
    /// A list of the names of the administrators: every account whose policy may manage the settings.
    Admins(usize),
    /// An accounts `Session` (user, role, policy): rebuilt from the account the session NAMES; the role and policy in it are dropped.
    Session(usize),
    /// The `can_shared` / `can_own` flags of a skill or package call. The scope (a `Scope` at argument `scope`, or its `scope` field when
    /// `field` is set) names the owner of a personal scope; for it `can_own` is the account's `skills` right. The shared scope names nobody: see
    /// `ALLOWED` (D-SEC1-SHARED).
    ScopeFlags { scope: usize, field: Option<&'static str>, shared: usize, own: usize },
}

/// One served call that carries a claim: the trait, the method, where the account's name is (None: the name is inside the claim, or none is
/// needed) and the claims the core overwrites.
pub(crate) struct Rule {
    pub iface: &'static str,
    pub method: &'static str,
    pub user: Option<usize>,
    pub claims: &'static [Claim],
}

const fn rule(iface: &'static str, method: &'static str, user: Option<usize>, claims: &'static [Claim]) -> Rule {
    Rule { iface, method, user, claims }
}

/// Every served call whose arguments claim a right, with what the core puts instead.
pub(crate) const RULES: &[Rule] = &[
    // the file rules and the person's own library (ServicesHandle)
    rule("ServicesHandle", "scope_check_read", Some(1), &[Claim::Policy(0)]),
    rule("ServicesHandle", "scope_check_write", Some(1), &[Claim::Policy(0)]),
    rule("ServicesHandle", "scope_roots", Some(1), &[Claim::Policy(0)]),
    rule("ServicesHandle", "scope_roots_with_home", Some(1), &[Claim::Policy(0)]),
    rule("ServicesHandle", "rag_refresh_tick", Some(0), &[Claim::Policy(1)]),
    // sign-in and sign-out: whether the person is an administrator is the account's, not the UI's word
    rule("ServicesHandle", "on_login", Some(0), &[Claim::Admin(2)]),
    rule("ServicesHandle", "on_logout", Some(0), &[Claim::Admin(1)]),
    rule("ServicesHandle", "on_resume", Some(0), &[Claim::Admin(1)]),
    rule("ServicesHandle", "sharing_notice_tick", Some(0), &[Claim::Admin(1)]),
    rule("ServicesFacetB3", "computer_announce", None, &[Claim::Admins(0)]),
    // flows: the environment of a person (the guard of served_flows.rs also re-reads the policy and replaces every `by`)
    rule("ServicesHandle", "flows_jobs", Some(0), &[Claim::Policy(1)]),
    rule("FlowsFacet", "open", Some(0), &[Claim::Policy(1)]),
    rule("FlowsFacet", "answer_permission", Some(0), &[Claim::Policy(1)]),
    rule("FlowsFacet", "start_for_login", Some(0), &[Claim::Policy(1)]),
    // the store's launcher and its update check
    rule("StoreHub", "tiles", Some(1), &[Claim::Policy(0)]),
    rule("StoreHub", "add_dev_tile", Some(1), &[Claim::Policy(0)]),
    rule("StoreUpdates", "update_check_run", Some(1), &[Claim::Policy(0)]),
    // clusters: the role of the person who switches, starts or stops
    rule("ClusterFacet", "switch", Some(2), &[Claim::Role(3)]),
    rule("FleetApi", "start", Some(2), &[Claim::Role(3)]),
    rule("FleetApi", "stop", Some(2), &[Claim::Role(3)]),
    // the dashboard host judges a path by a policy
    rule("DashHost", "read_ok", Some(1), &[Claim::Policy(0)]),
    // the accounts: the actor of a change is rebuilt from its name (AccountsGuarded does the same; both must hold)
    rule("AccountsApi", "create_user", None, &[Claim::Session(0)]),
    rule("AccountsApi", "delete_user", None, &[Claim::Session(0)]),
    rule("AccountsApi", "set_role", None, &[Claim::Session(0)]),
    rule("AccountsApi", "set_overrides", None, &[Claim::Session(0)]),
    rule("AccountsApi", "set_password", None, &[Claim::Session(0)]),
    // skills and packages
    rule("SkillApi", "add", None, &[Claim::ScopeFlags { scope: 0, field: None, shared: 2, own: 3 }]),
    rule("SkillApi", "remove", None, &[Claim::ScopeFlags { scope: 0, field: None, shared: 2, own: 3 }]),
    rule("PackageApi", "install", None, &[Claim::ScopeFlags { scope: 0, field: Some("scope"), shared: 2, own: 3 }]),
    rule("PackageApi", "remove", None, &[Claim::ScopeFlags { scope: 0, field: None, shared: 2, own: 3 }]),
];

/// Authority-carrying arguments of served methods that the core does NOT overwrite, each with the reviewed reason. `Open` ones are known
/// holes with an owner decision pending; `Data` ones are not authority at all (the value is what the person asks for, and the check is made
/// on the account the call names). The audit test fails for a method that is in neither list.
#[cfg(test)]
pub(crate) enum Why {
    /// The argument is the thing asked for, not a right: the target role of a new account, a skill scope, a chat session.
    Data(&'static str),
    /// A known gap recorded in `docs/development/ALPHA-7-DECISIONS.md` (none since alpha.8 SEC3; the variant stays so a new gap is listed
    /// in the open, never silently).
    #[allow(dead_code)]
    Open(&'static str),
}

#[cfg(test)]
pub(crate) const ALLOWED: &[(&str, &str, &str, Why)] = &[
    ("AccountsApi", "create_user", "role", Why::Data("the role the administrator gives the new account; who may give it is the actor, rebuilt from its name")),
    ("AccountsApi", "set_role", "role", Why::Data("the role the administrator sets; the actor is rebuilt from its name")),
    ("AccountsApi", "set_overrides", "ov", Why::Data("the overrides the administrator sets; the actor is rebuilt from its name")),
    ("SessionApi", "save", "s", Why::Data("a chat session (the history), not an account session")),
    ("SessionApi", "remember_turn", "s", Why::Data("a chat session (the history), not an account session")),
    ("FlowsRuns", "decide", "approved", Why::Data("the person's answer to a pending approval (yes or no); who answers is the account, `by` is replaced in the guard of served_flows.rs")),
];

impl Registry<'_> {
    /// The role of the account `user`, read from the core's accounts.
    pub(crate) fn role_of(&self, user: &str) -> Result<iyakki_sec::policy::Role, String> {
        let accounts = self.side().accounts.ok_or_else(|| Self::missing("accounts"))?;
        accounts.get_public(user).map(|r| r.role).ok_or_else(|| no_account(user))
    }

    /// Whether the account `user` may manage the settings (the shell's "is administrator").
    pub(crate) fn admin_of(&self, user: &str) -> Result<bool, String> {
        self.policy_of(user).map(|p| p.manage_settings)
    }

    /// The names of the accounts that may manage the settings.
    pub(crate) fn admins(&self) -> Result<Vec<String>, String> {
        let accounts = self.side().accounts.ok_or_else(|| Self::missing("accounts"))?;
        Ok(accounts.list_public().into_iter().filter(|r| r.policy().manage_settings).map(|r| r.name).collect())
    }
}

fn no_account(user: &str) -> String {
    format!("{NOT_SERVED}: there is no account {user}")
}

fn slot(a: &mut Value, i: usize) -> Result<&mut Value, String> {
    a.as_array_mut().and_then(|v| v.get_mut(i)).ok_or_else(|| format!("{NOT_SERVED}: the call has no argument {i}"))
}

fn name_at(a: &Value, i: usize) -> Result<String, String> {
    a.get(i).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("{NOT_SERVED}: the call names no account"))
}

fn put<T: serde::Serialize>(a: &mut Value, i: usize, v: &T) -> Result<(), String> {
    *slot(a, i)? = serde_json::to_value(v).map_err(|e| format!("the core's reading cannot be encoded: {e}"))?;
    Ok(())
}

/// The owner a `Scope` value names: `Some(name)` for a personal scope, `None` for the shared one.
fn owner_of(scope: &Value) -> Option<String> {
    scope.get("User").and_then(Value::as_str).map(str::to_string)
}

/// Overwrite every claim of the call with the core's reading. Refuses (and the call does not run) when the account a claim is about is not
/// one of the core's. A call that is not in [`RULES`] is untouched.
pub(crate) fn vouch(reg: &Registry<'_>, iface: &str, method: &str, a: &mut Value) -> Result<(), String> {
    let Some(r) = RULES.iter().find(|r| r.iface == iface && r.method == method) else { return Ok(()) };
    let user = match r.user {
        Some(i) => Some(name_at(a, i)?),
        None => None,
    };
    let need = || user.clone().ok_or_else(|| format!("{NOT_SERVED}: the call names no account"));
    for c in r.claims {
        match c {
            Claim::Policy(i) => put(a, *i, &reg.policy_of(&need()?)?)?,
            Claim::Role(i) => put(a, *i, &reg.role_of(&need()?)?)?,
            Claim::Admin(i) => put(a, *i, &reg.admin_of(&need()?)?)?,
            Claim::Admins(i) => put(a, *i, &reg.admins()?)?,
            Claim::Session(i) => {
                let who = a.get(*i).and_then(|s| s.get("user")).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("{NOT_SERVED}: the session names no account"))?;
                let rec = reg.side().accounts.ok_or_else(|| Registry::missing("accounts"))?.get_public(&who).ok_or_else(|| no_account(&who))?;
                put(a, *i, &iyakki_accounts::Session { user: rec.name.clone(), role: rec.role, policy: rec.policy() })?;
            }
            Claim::ScopeFlags { scope, field, shared, own } => {
                let s = a.get(*scope).ok_or_else(|| format!("{NOT_SERVED}: the call has no scope"))?;
                let s = match field {
                    Some(f) => s.get(*f).cloned().unwrap_or(Value::Null),
                    None => s.clone(),
                };
                if let Some(owner) = owner_of(&s) {
                    let p = reg.policy_of(&owner)?;
                    put(a, *own, &p.skills)?;
                    put(a, *shared, &false)?; // a personal scope never needs the shared right
                }
            }
        }
    }
    Ok(())
}

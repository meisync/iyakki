//! `api:*` scopes of the web service (F-WEBSVC-06, first part): what a program that is paired through developer mode may call. The
//! route table is grouped into families; a scope is a family (`api:flows`: every route of it) or a family narrowed to reading
//! (`api:flows:read`: only GET). Only an administrator grants scopes, on the device, one computer and one family at a time; a new
//! computer holds none; the administrator routes and the sign-in routes are in no family that can be granted.
//!
//! Not built yet: the sign-in of a program with its paired key at the web service, the check of these scopes on every request and the
//! Settings page that lists them (see the row in `FEATURES.md`).

use iyakki_sec::policy::Policy;

use super::DevMode;
use crate::webservice::routes::{Access, Route, ROUTES};

pub use iyakki_shared::devmode::apiscopes::FAMILIES;

/// The family of a route; `None` for the routes that need no sign-in or only end the token.
pub fn family_of(r: &Route) -> Option<&'static str> {
    if r.access == Access::Public || r.id == "logout" {
        return None;
    }
    let id = r.id;
    Some(match id {
        // an administrator route is in the admin family whatever its name starts with (checked first, so a new `files_...` admin route cannot fall into `api:files`)
        "metrics" => "api:metrics",
        _ if r.access == Access::Admin => "api:admin",
        "whoami" | "models" | "tools" | "openapi" => "api:status",
        "approvals_list" => "api:chat",
        // a program never answers an approval: it could start a run and approve its own tool calls, which would make `api:chat` the
        // whole account. A person answers on the device, in the chat or from their own sign-in.
        "approvals_decide" | "tapprovals_approve" | "tapprovals_deny" => "api:admin",
        "data_call" => "api:data",
        _ if id.starts_with("data_") => "api:data",
        _ if id.starts_with("runs_") => "api:chat",
        _ if id.starts_with("flows_") => "api:flows",
        _ if id.starts_with("sched_") => "api:schedules",
        _ if id.starts_with("files_") => "api:files",
        _ if id.starts_with("git_") => "api:git",
        _ if id.starts_with("notif_") => "api:notify",
        _ if id.starts_with("tile_") || id == "events" => "api:tiles",
        _ if id.starts_with("admin_") || id.starts_with("webaccess_") || r.access == Access::Admin => "api:admin",
        _ if id.starts_with("tenant") || id.starts_with("tcfg") || id.starts_with("tapprovals_") || id.starts_with("tteam_") || id.starts_with("ns_") => "api:tenant",
        _ => "api:admin",
    })
}

/// Can an administrator grant this scope (`api:flows` or `api:flows:read`)?
pub fn grantable(scope: &str) -> bool {
    let base = scope.strip_suffix(":read").unwrap_or(scope);
    FAMILIES.iter().any(|(f, _)| *f == base) && base != "api:admin"
}

/// May a program holding `held` make this call? A family scope allows every method; the `:read` form only GET and HEAD.
pub fn allows(held: &[String], r: &Route) -> bool {
    let Some(f) = family_of(r) else { return false };
    if f == "api:admin" {
        return false;
    }
    let read = matches!(r.method, "GET" | "HEAD");
    held.iter().any(|s| s == f || (read && s.strip_suffix(":read") == Some(f)))
}

/// The scopes in force for the families of a route table (for the page and for tests): every family with the routes in it.
pub fn routes_in(family: &str) -> Vec<&'static Route> {
    ROUTES.iter().filter(|r| family_of(r) == Some(family)).collect()
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug, PartialEq, Eq)]
struct ApiState {
    /// computer name -> scopes
    grants: std::collections::BTreeMap<String, Vec<String>>,
}

impl DevMode {
    /// Drop the `api:*` grants kept under a computer's name (when its pairing is removed: a new computer with the same name starts with none).
    pub(super) fn api_forget(&self, computer: &str) {
        let _one_at_a_time = super::keys_lock();
        let mut st = self.read::<ApiState>("api.json");
        if st.grants.remove(computer).is_some() {
            let _ = self.write("api.json", &st);
        }
    }

    /// The `api:*` scopes a paired computer holds (none until an administrator grants them; none for a computer that is no longer paired).
    pub fn api_scopes(&self, computer: &str) -> Vec<String> {
        if !self.keys().iter().any(|k| k.name == computer) {
            return Vec::new();
        }
        self.read::<ApiState>("api.json").grants.get(computer).cloned().unwrap_or_default()
    }

    /// Every paired computer with its `api:*` scopes (for the Settings page).
    pub fn api_grants(&self) -> Vec<(String, Vec<String>)> {
        let names: Vec<String> = self.keys().into_iter().map(|k| k.name).collect();
        self.read::<ApiState>("api.json").grants.into_iter().filter(|(c, s)| names.contains(c) && !s.is_empty()).collect()
    }

    /// Grant or take away one scope (administrators, on the device). Granting `api:flows` while `api:flows:read` is held replaces it, and
    /// the other way round, so a computer holds one form of a family.
    pub fn set_api_scope(&self, policy: &Policy, user: &str, computer: &str, scope: &str, on: bool) -> Result<Vec<String>, String> {
        Self::need_admin(policy)?;
        if !grantable(scope) {
            return Err(format!("`{scope}` is not an API scope that can be granted (the families are {}; add :read for reading only)", FAMILIES.iter().filter(|(f, _)| *f != "api:admin").map(|(f, _)| *f).collect::<Vec<_>>().join(", ")));
        }
        let _one_at_a_time = super::keys_lock();
        if !self.keys().iter().any(|k| k.name == computer) {
            return Err(format!("no paired computer called {computer}"));
        }
        let mut st: ApiState = self.read("api.json");
        let base = scope.strip_suffix(":read").unwrap_or(scope).to_string();
        let list = st.grants.entry(computer.to_string()).or_default();
        list.retain(|s| s.strip_suffix(":read").unwrap_or(s) != base);
        if on {
            list.push(scope.to_string());
        }
        list.sort();
        let now = list.clone();
        if now.is_empty() {
            st.grants.remove(computer);
        }
        self.write("api.json", &st)?;
        self.log(user, user, if on { "devmode.api.grant" } else { "devmode.api.revoke" }, &format!("{computer} {scope} -> now: {}", if now.is_empty() { "nothing".to_string() } else { now.join(", ") }), true);
        Ok(now)
    }

    /// Take away every `api:*` scope of a computer (for instance when it is unpaired).
    pub fn clear_api_scopes(&self, policy: &Policy, user: &str, computer: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut st: ApiState = self.read("api.json");
        if st.grants.remove(computer).is_some() {
            self.write("api.json", &st)?;
            self.log(user, user, "devmode.api.revoke", &format!("{computer} all"), true);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devmode::DevKey;
    use iyakki_sec::policy::Role;

    fn dm(tag: &str) -> DevMode {
        let d = std::env::temp_dir().join(format!("iyakki-apiscopes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let dm = DevMode::open(&d);
        let key = |n: &str| DevKey { name: n.into(), key: format!("{:0>64}", n.len()), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into()] };
        dm.write("keys.json", &vec![key("laptop"), key("ci-runner")]).unwrap();
        dm
    }

    fn route(id: &str) -> &'static Route {
        ROUTES.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no route {id}"))
    }

    #[test]
    fn every_route_that_needs_a_sign_in_is_in_exactly_one_family_and_the_admin_family_is_never_grantable() {
        for r in ROUTES {
            match family_of(r) {
                None => assert!(r.access == Access::Public || r.id == "logout", "{} has no family", r.id),
                Some(f) => assert!(FAMILIES.iter().any(|(n, _)| *n == f), "{}: {f}", r.id),
            }
        }
        for (f, _) in FAMILIES {
            assert!(!routes_in(f).is_empty(), "{f} holds no route");
        }
        assert!(routes_in("api:admin").iter().all(|r| r.access == Access::Admin || r.id.starts_with("admin_") || r.id.starts_with("webaccess_") || ["approvals_decide", "tapprovals_approve", "tapprovals_deny"].contains(&r.id)));
        // the converse, the safety net for a new route: every administrator route is in the admin family
        assert!(ROUTES.iter().filter(|r| r.access == Access::Admin && r.id != "metrics").all(|r| family_of(r) == Some("api:admin")));
        assert_eq!(family_of(route("metrics")), Some("api:metrics"), "the numbers are the one administrator route a paired computer can be given");
        assert!(routes_in("api:admin").iter().any(|r| r.id == "tenant_bootstrap"), "an administrator route stays admin even under a tenant path");
        assert!(!grantable("api:admin") && !grantable("api:admin:read") && !grantable("install") && !grantable("api:nothing") && !grantable("api:flows:write"));
        assert!(grantable("api:flows") && grantable("api:flows:read") && grantable("api:status") && grantable("api:metrics") && grantable("api:metrics:read"));
        // the sign-in and health routes are in no family
        for id in ["healthz", "login", "version", "logout"] {
            assert_eq!(family_of(route(id)), None);
        }
    }

    #[test]
    fn a_program_cannot_answer_an_approval_so_api_chat_is_not_the_whole_account() {
        let all: Vec<String> = FAMILIES.iter().map(|(f, _)| f.to_string()).collect();
        for id in ["approvals_decide", "tapprovals_approve", "tapprovals_deny"] {
            assert!(!allows(&all, route(id)), "{id} must stay a person's");
            assert_eq!(family_of(route(id)), Some("api:admin"));
        }
        let chat = vec!["api:chat".to_string()];
        assert!(allows(&chat, route("approvals_list")) && allows(&chat, route("runs_start")));
    }

    #[test]
    fn a_family_scope_allows_its_routes_the_read_form_only_reading_and_nothing_else_opens() {
        let flows = vec!["api:flows".to_string()];
        let read = vec!["api:flows:read".to_string()];
        assert!(allows(&flows, route("flows_list")) && allows(&flows, route("flows_run")));
        assert!(allows(&read, route("flows_list")) && !allows(&read, route("flows_run")), "reading only: no POST");
        assert!(!allows(&flows, route("files_list")) && !allows(&flows, route("runs_start")), "another family is another scope");
        assert!(!allows(&[], route("flows_list")), "nothing held, nothing allowed");
        let everything: Vec<String> = FAMILIES.iter().map(|(f, _)| f.to_string()).collect();
        assert!(!allows(&everything, route("admin_status")) && !allows(&everything, route("webaccess_set")) && !allows(&everything, route("tenant_bootstrap")), "even all families do not reach the administrator routes");
        assert!(!allows(&everything, route("login")), "the sign-in route is not a program's to call with a scope");
        let status = vec!["api:status".to_string()];
        assert!(allows(&status, route("whoami")) && allows(&status, route("models")) && !allows(&status, route("runs_start")));
    }

    #[test]
    fn only_an_administrator_grants_one_computer_one_family_at_a_time_and_it_can_be_taken_back() {
        let d = dm("grant");
        let admin = Role::Admin.template();
        let user = Role::User.template();
        assert!(d.api_scopes("laptop").is_empty(), "a computer starts with nothing");
        assert!(d.set_api_scope(&user, "bob", "laptop", "api:flows", true).is_err(), "a user cannot grant");
        assert!(d.set_api_scope(&admin, "root", "laptop", "api:admin", true).is_err());
        assert!(d.set_api_scope(&admin, "root", "ghost", "api:flows", true).unwrap_err().contains("no paired computer"));
        assert_eq!(d.set_api_scope(&admin, "root", "laptop", "api:flows", true).unwrap(), vec!["api:flows".to_string()]);
        assert_eq!(d.set_api_scope(&admin, "root", "laptop", "api:files:read", true).unwrap(), vec!["api:files:read".to_string(), "api:flows".to_string()]);
        // narrowing: the read form replaces the full form of the same family
        assert_eq!(d.set_api_scope(&admin, "root", "laptop", "api:flows:read", true).unwrap(), vec!["api:files:read".to_string(), "api:flows:read".to_string()]);
        assert!(d.api_scopes("ci-runner").is_empty(), "another computer is unaffected");
        assert_eq!(d.api_grants(), vec![("laptop".to_string(), vec!["api:files:read".to_string(), "api:flows:read".to_string()])]);
        // revoking one, then all
        assert_eq!(d.set_api_scope(&admin, "root", "laptop", "api:flows", false).unwrap(), vec!["api:files:read".to_string()]);
        d.clear_api_scopes(&admin, "root", "laptop").unwrap();
        assert!(d.api_scopes("laptop").is_empty() && d.api_grants().is_empty());
        // unpairing the computer ends its scopes at once
        d.set_api_scope(&admin, "root", "ci-runner", "api:status", true).unwrap();
        let mut keys = d.keys();
        keys.retain(|k| k.name != "ci-runner");
        d.write("keys.json", &keys).unwrap();
        assert!(d.api_scopes("ci-runner").is_empty() && d.api_grants().is_empty());
    }
}

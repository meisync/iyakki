//! Who a notification is for: people named, people with a role, or the team. Always checked against the tenant's roles: a name that is
//! not a member of the tenant gets nothing. Pure.

use super::sink::{Recipients, RoleSel};
use crate::cloudroles::{NsRole, Roles, TenantRole, MAX_EXPANDED, TENANT_WIDE};
use std::collections::BTreeSet;

fn valid(name: &str) -> bool {
    iyakki_sec::policy::valid_username(name)
}

/// The members, sorted and without repeats. `roles` is `None` on a machine without tenant roles: named people pass, nothing else
/// can be expanded.
pub fn expand(roles: Option<&Roles>, team: Option<&str>, who: &Recipients) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    match (who, roles) {
        (Recipients::People(p), None) => out.extend(p.iter().cloned()),
        (_, None) => {}
        (Recipients::People(p), Some(r)) => out.extend(p.iter().filter(|n| r.tenant_role(n).is_some()).cloned()),
        (Recipients::Team, Some(r)) => match team.and_then(|t| r.namespaces.get(t)) {
            Some(m) => out.extend(m.keys().filter(|n| r.tenant_role(n).is_some()).cloned()),
            None => {
                if team.is_none() {
                    out.extend(r.tenant.keys().cloned())
                }
            }
        },
        (Recipients::Roles(sels), Some(r)) => {
            for s in sels {
                match s {
                    RoleSel::TenantAdmin => out.extend(r.tenant.iter().filter(|(_, x)| **x >= TenantRole::Admin).map(|(n, _)| n.clone())),
                    RoleSel::TeamAdmin => {
                        if let Some(m) = team.and_then(|t| r.namespaces.get(t)) {
                            out.extend(m.iter().filter(|(n, x)| **x == NsRole::Admin && r.tenant_role(n).is_some()).map(|(n, _)| n.clone()));
                        }
                    }
                    RoleSel::Approver(scope) => {
                        if let Some(t) = team {
                            out.extend(r.approver_holders(t, &[scope.as_str()]));
                        }
                    }
                    RoleSel::Group(g) => {
                        let m = team.and_then(|t| r.groups.get(t).and_then(|x| x.get(g))).or_else(|| r.groups.get(TENANT_WIDE).and_then(|x| x.get(g)));
                        out.extend(m.into_iter().flatten().filter(|n| r.tenant_role(n).is_some()).cloned());
                    }
                }
            }
        }
    }
    out.into_iter().filter(|n| valid(n)).take(MAX_EXPANDED).collect()
}

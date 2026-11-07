//! Notifications from flows and from the scheduler: a thin adapter over the tenant's notification hub (`notifyhub`).
//!
//! The contract is the hub's ([`Notification`], [`NotifySink`]); there is no second trait here. This file does three things:
//! maps a flow `notify` node (`to`, `severity`, `title`, `body`) to a [`Notification`] with the source `flow:<id>:<run>`
//! ([`from_node`]), fills the texts from run data while refusing secrets ([`render`]), and offers [`say`] to the scheduler and the
//! approval bridge for the common "tell this person and these roles" case. Durability, dedupe, channels, quiet hours, reminders and
//! escalation are the hub's (`notifyhub::service::Hub`, written to the tenant volume before any delivery is tried). A waiting
//! `approval` node is announced by the hub from the approvals file, so nothing here announces it.

pub use crate::notifyhub::sink::{Notification, NotifySink, Receipt, Recipients, RoleSel, Severity};
use serde_json::Value;
use std::collections::BTreeMap;

/// The severity words of a flow node (`info`, `warn`, `error`, `critical` and the hub's own) as the hub's levels.
pub fn severity_of(s: &str) -> Severity {
    match s.trim().to_ascii_lowercase().as_str() {
        "low" => Severity::Low,
        "warn" | "warning" | "high" | "error" => Severity::High,
        "critical" => Severity::Critical,
        _ => Severity::Normal,
    }
}

/// `to` of a notify node: `me` / `owner` (the person the run belongs to), `team`, `role:admin` (the team's admins),
/// `role:tenant_admin`, `role:approver:<scope>`, `role:group:<name>`, or a list of people.
pub fn recipients_from_node(to: &Value, owner: &str) -> Result<Recipients, String> {
    let one = |s: &str| -> Result<Recipients, String> {
        match s.trim() {
            "" | "me" | "owner" => Ok(Recipients::People(vec![owner.to_string()])),
            "team" => Ok(Recipients::Team),
            "role:admin" | "role:team_admin" => Ok(Recipients::Roles(vec![RoleSel::TeamAdmin])),
            "role:tenant_admin" => Ok(Recipients::Roles(vec![RoleSel::TenantAdmin])),
            r => {
                if let Some(scope) = r.strip_prefix("role:approver:") {
                    Ok(Recipients::Roles(vec![RoleSel::Approver(scope.to_string())]))
                } else if let Some(g) = r.strip_prefix("role:group:") {
                    Ok(Recipients::Roles(vec![RoleSel::Group(g.to_string())]))
                } else if r.starts_with("role:") {
                    Err(format!("`{r}` is not a role a notification can go to (admin, tenant_admin, approver:<scope>, group:<name>)"))
                } else if crate::cloudstorage::plain_name(r) {
                    Ok(Recipients::People(vec![r.to_string()]))
                } else {
                    Err(format!("`{r}` is not a person or a role"))
                }
            }
        }
    };
    match to {
        Value::Null => one(""),
        Value::String(s) => one(s),
        Value::Array(a) => {
            let mut people = Vec::new();
            for x in a {
                match one(x.as_str().ok_or("`to` is a word or a list of people")?)? {
                    Recipients::People(p) => people.extend(p),
                    _ => return Err("a list in `to` holds people only; use one role or `team` by itself".into()),
                }
            }
            Ok(Recipients::People(people))
        }
        _ => Err("`to` is a word or a list of people".into()),
    }
}

/// Where a flow notification comes from.
pub struct FlowNote<'a> {
    pub tenant: &'a str,
    pub team: &'a str,
    pub flow: &'a str,
    pub run: &'a str,
    /// The person or schedule owner the run belongs to.
    pub owner: &'a str,
}

/// A notify node's settings (`to`, `severity`, `title`, `body`, optional `link`) plus run data, as one [`Notification`]. Channels are
/// not named here: the hub's rules and the tenant's channels decide where a notification goes besides the INBOX.
pub fn from_node(ctx: &FlowNote, cfg: &Value, vars: &BTreeMap<String, String>) -> Result<Notification, String> {
    let text = |k: &str| cfg.get(k).and_then(Value::as_str).unwrap_or("");
    let title = render(text("title"), vars)?;
    if title.trim().is_empty() {
        return Err("a notification needs a title".into());
    }
    let body = render(text("body"), vars)?;
    let recipients = recipients_from_node(cfg.get("to").unwrap_or(&Value::Null), ctx.owner)?;
    Ok(Notification {
        tenant: ctx.tenant.into(),
        team: Some(ctx.team.to_string()).filter(|t| !t.is_empty()),
        recipients,
        severity: severity_of(text("severity")),
        title: title.chars().take(120).collect(),
        body: body.chars().take(600).collect(),
        link: Some(crate::inbox::Link::new("flows", ctx.run)),
        dedupe_key: format!("flow:{}:{}:{}", ctx.flow, ctx.run, title.chars().take(40).collect::<String>()),
        source: format!("flow:{}:{}", ctx.flow, ctx.run),
    })
}

/// Fill `{{name}}` in a notification text from run data. A name that is a secret or vault reference is refused, and so is a value that
/// looks like one: a notification text never carries a secret.
pub fn render(template: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    let lower = template.to_ascii_lowercase();
    if lower.contains("vault:") || lower.contains("uvault:") || lower.contains("{{secret") || lower.contains("{{ secret") || lower.contains("{{vault") {
        return Err("a notification text cannot use a secret or a vault entry".into());
    }
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { return Err("a `{{` is not closed".into()) };
        let key = after[..j].trim();
        if key.starts_with("secret") || key.starts_with("vault") || key.starts_with("uvault") {
            return Err("a notification text cannot use a secret or a vault entry".into());
        }
        let v = vars.get(key).ok_or_else(|| format!("`{key}` is not run data a notification can use"))?;
        if v.contains("vault:") {
            return Err(format!("`{key}` holds a vault reference and is not put into a text"));
        }
        out.push_str(v);
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// What the scheduler and the approval bridge say: to named people and/or roles of the team. One notification per kind of recipient,
/// the dedupe key told apart by a suffix. Returns how many the hub accepted.
#[allow(clippy::too_many_arguments)]
pub fn say(sink: &dyn NotifySink, tenant: &str, team: &str, people: Vec<String>, roles: Vec<RoleSel>, sev: Severity, title: &str, body: &str, link: Option<(&str, &str)>, key: &str, source: &str) -> usize {
    let mk = |r: Recipients, suffix: &str| Notification {
        tenant: tenant.into(),
        team: Some(team.to_string()).filter(|t| !t.is_empty()),
        recipients: r,
        severity: sev,
        title: title.chars().take(120).collect(),
        body: body.chars().take(600).collect(),
        link: link.map(|(a, i)| crate::inbox::Link::new(a, i)),
        dedupe_key: format!("{key}:{suffix}"),
        source: source.into(),
    };
    let mut sent = 0;
    if !people.is_empty() && sink.emit(&mk(Recipients::People(people), "p")).is_ok() {
        sent += 1;
    }
    if !roles.is_empty() && sink.emit(&mk(Recipients::Roles(roles), "r")).is_ok() {
        sent += 1;
    }
    sent
}

/// An engine notice (`Host::notify`: a flow's `notify` step, a failure) as a notification to the person the run belongs to.
pub fn from_node_notice(tenant: &str, team: &str, user: &str, n: &iyakki_flow::host::Notice) -> Notification {
    Notification {
        tenant: tenant.into(),
        team: Some(team.to_string()).filter(|t| !t.is_empty()),
        recipients: Recipients::People(vec![user.to_string()]),
        severity: severity_of(&n.level),
        title: n.title.chars().take(120).collect(),
        body: n.body.chars().take(600).collect(),
        link: Some(crate::inbox::Link::new("flows", if n.run_id.is_empty() { &n.flow_id } else { &n.run_id })),
        dedupe_key: format!("notice:{}:{}", n.run_id, n.title.chars().take(40).collect::<String>()),
        source: format!("flow:{}:{}", n.flow_id, n.run_id),
    }
}

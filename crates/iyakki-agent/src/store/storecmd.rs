//! The terminal's `store` lifecycle verbs (`list --outdated`, `update`, `remove`, `rollback`, `pin`, `unpin`) as text in, text out.
//! The terminal (`cli.rs`) only parses the line and prints the answer; the rules are [`super::lifecycle`]'s.

use iyakki_sec::policy::Policy;

use super::catalog::HttpFetch;
use super::install::{Actor, Confirm};
use super::lifecycle::{Ctx, Done, Life, LifeError, RemoveOpts};
use super::Store;
use crate::services::Services;

pub const USAGE: &str = "store list [--outdated|--all] | update <id>|--all [--permissions] [--yes] | remove <id> [--keep-data|--delete-data --confirm=<name>] [--yes] | rollback <id> | pin <id> | unpin <id>  (an id is kind:name, e.g. skill:meeting-notes, cli:kubectl)";

/// Split a command line into positional words and `--flags` (`--confirm=NAME` keeps its value).
pub fn split(words: &[&str]) -> (Vec<String>, Vec<String>) {
    let (mut pos, mut flags) = (vec![], vec![]);
    for w in words {
        if w.starts_with("--") { flags.push(w.to_string()) } else { pos.push(w.to_string()) }
    }
    (pos, flags)
}

fn flag(flags: &[String], name: &str) -> bool {
    flags.iter().any(|f| f == &format!("--{name}"))
}

fn flag_value(flags: &[String], name: &str) -> Option<String> {
    flags.iter().find_map(|f| f.strip_prefix(&format!("--{name}=")).map(str::to_string))
}

/// The lifecycle list as lines.
pub fn list_text(lives: &[Life], services: &[super::lifecycle::ServiceLife], outdated_only: bool) -> String {
    let mut out = String::new();
    for l in lives.iter().filter(|l| l.entry.ty != super::installed::Type::App && (!outdated_only || l.has_update())) {
        let latest = l.latest.clone().unwrap_or_else(|| "-".into());
        out.push_str(&format!("{:<34} {:<10} {:<10} {}{}\n", l.id, l.entry.version, latest, l.status_line(), if l.can_rollback() { format!("  (can roll back to {})", l.previous.clone().unwrap_or_default()) } else { String::new() }));
    }
    for s in services.iter().filter(|s| !outdated_only || s.latest.is_some()) {
        out.push_str(&format!("{:<34} {:<10} {:<10} {}\n", format!("service:{}", s.id), s.version, s.latest.clone().unwrap_or_else(|| "-".into()), s.note));
    }
    if out.is_empty() {
        return if outdated_only { "Everything is up to date as far as the catalogs on this device say. (Press c in STORE to look for news.)".into() } else { "Nothing installed through the store.".into() };
    }
    out.trim_end().to_string()
}

fn err_text(e: LifeError) -> String {
    match e {
        LifeError::NeedsConfirm(m) => format!("{m} (nothing was changed; add the flag shown in `store` usage to confirm)"),
        LifeError::Refused(m) | LifeError::Failed(m) => m,
    }
}

/// Run a lifecycle verb. `None`: not one of ours (the terminal handles it as before).
pub fn run(svc: &Services, policy: &Policy, user: &str, words: &[&str]) -> Option<Result<String, String>> {
    let (pos, flags) = split(words);
    let verb = pos.first().map(String::as_str).unwrap_or("list");
    let id = pos.get(1).map(String::as_str);
    let store = Store::for_services(svc);
    let fetch = HttpFetch(svc.quick_upstream());
    let loaded = store.local_catalogs(svc);
    let lives = store.lifecycle(svc, policy, user, &loaded, &Ctx::default());
    let actor = Actor::from_policy(user, policy);
    let audit = |_: &str, _: &str| {};
    let find = |id: Option<&str>| -> Result<Life, String> {
        let id = id.ok_or_else(|| format!("usage: {USAGE}"))?;
        Store::find_life(&lives, id).cloned()
    };
    Some(match verb {
        "list" if flag(&flags, "outdated") || flag(&flags, "all") => Ok(list_text(&lives, &store.services(svc, user), flag(&flags, "outdated"))),
        "outdated" => Ok(list_text(&lives, &store.services(svc, user), true)),
        "update" if flag(&flags, "all") || id == Some("all") => {
            let rep = store.update_all(svc, policy, user, &loaded, &fetch, &audit, &mut |_| {});
            Ok(if rep.items.is_empty() { "Nothing to update.".to_string() } else { rep.summary().trim_end().to_string() })
        }
        "update" => find(id).and_then(|l| {
            if l.entry.ty == super::installed::Type::Driver {
                return Err("Driver packs change the whole machine: update them from the Drivers page of the STORE tile.".into());
            }
            let plan = store.plan_update(svc, &actor, &l, &loaded, &fetch).map_err(err_text)?;
            let c = Confirm { permissions: flag(&flags, "permissions"), warnings: flag(&flags, "yes") };
            store.apply_update(svc, &actor, &plan, c, &audit).map_err(|e| match e {
                LifeError::NeedsConfirm(m) => format!("{}\n\n{m}\nNothing was changed. Run again with --permissions (new permissions) and --yes (warnings) to confirm.", plan.summary().trim_end()),
                other => err_text(other),
            })
        }),
        "rollback" => find(id).and_then(|l| store.rollback(svc, &actor, &l, &audit).map_err(err_text)),
        "pin" => find(id).and_then(|l| {
            if !l.can_pin() {
                return Err(format!("{} cannot be pinned: it has nothing to update.", l.entry.title));
            }
            store.pin(l.key(), &l.entry.version)?;
            Ok(format!("{} is pinned at {}: updates are skipped until you run `store unpin {}`.", l.entry.title, l.entry.version, l.id))
        }),
        "unpin" => find(id).and_then(|l| store.unpin(l.key()).map(|_| format!("{} can be updated again.", l.entry.title))),
        "remove" => {
            let l = match find(id) {
                Ok(l) => l,
                // not something the lifecycle knows: the terminal's older `store remove <skill>` handles it
                Err(_) => return None,
            };
            let plan = store.plan_remove(svc, user, &lives, &l);
            let opts = RemoveOpts { delete_data: flag(&flags, "delete-data"), typed: flag_value(&flags, "confirm").unwrap_or_default(), ack_dependents: flag(&flags, "yes") };
            if flag(&flags, "delete-data") && flag(&flags, "keep-data") {
                return Some(Err("Choose --keep-data or --delete-data, not both.".into()));
            }
            store.remove_item(svc, policy, user, &lives, &l, &opts, &audit).map_err(|e| match e {
                LifeError::NeedsConfirm(m) => format!("{}\n{m}\nNothing was changed. Add --yes to accept the dependents warning; to delete its data add --delete-data --confirm={}.", plan.summary().trim_end(), l.entry.name),
                other => err_text(other),
            })
        }
        _ => return None,
    })
}

/// A summary line for the Update all review: what would happen to each item.
pub fn update_all_review(lives: &[Life]) -> String {
    let rows = Store::update_all_preview(lives);
    if rows.is_empty() {
        return "Nothing to update.".into();
    }
    rows.iter().map(|(t, s, ok)| format!("{} {t}: {s}", if *ok { "will update" } else { "will be skipped:" })).collect::<Vec<_>>().join("\n")
}

/// The result of an Update all run as the tile shows it.
pub fn report_line(done: &Done) -> &'static str {
    match done {
        Done::Updated(_) => "updated",
        Done::Skipped(_) => "skipped",
        Done::Failed(_) => "failed",
    }
}

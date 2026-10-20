//! The assistant's view of the store: `store_search` and `store_info` read what is already on the device (the built-in,
//! cached and local catalogs, never the network); `store_install` always asks, shows the same review the tile shows,
//! and is on the list of calls that Auto and Autopilot never run by themselves.

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use super::catalog::HttpFetch;
use super::install::{Actor, Confirm};
use super::manifest::canonical_set;
use super::{filter, Filter, Item, PriceFilter, State, Store};
use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;
use crate::skills::Scope;

pub fn is_store_tool(name: &str) -> bool {
    matches!(name, "store_search" | "store_info" | "store_install" | "store_outdated" | "store_update" | "store_remove" | "store_rollback")
}

fn offered(p: &Policy) -> bool {
    p.apps.contains(&App::Store)
}

pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !offered(p) {
        return vec![];
    }
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let tool = |name: &str, desc: &str, schema: Value, risk: Risk, untrusted: bool| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted };
    vec![
        tool(
            "store_search",
            "Search the IYAKKI app store (skills, tools, MCP servers, themes, command-line tools) that is on this device: built-in, cached and local catalogs, no network. `query` is free text; optional `kind` (skill, tool, mcp, cli, theme, template, agent) and `free` (true for free apps only). Returns numbered results with an id, type, trust level, price and whether it is installed. Use it when the person wants to do something a skill or tool might already cover, and tell them \"there is a skill for this in the store\". Read-only.",
            obj(json!({"query": {"type": "string"}, "kind": {"type": "string"}, "free": {"type": "boolean"}, "limit": {"type": "integer"}}), &[]),
            Risk::Safe,
            true,
        ),
        tool("store_info", "Details of one store app by `id` (from store_search): publisher, trust level and what it means, version, price, the permissions it asks for in plain words, size, licence, description. Read-only.", obj(json!({"id": {"type": "string"}}), &["id"]), Risk::Safe, true),
        tool(
            "store_install",
            "Install an app from the store by `id` (from store_search). The person is shown the same review as in the STORE tile (who signed it, its trust level, the permissions it asks for in plain words) and must approve; this always asks, even in Auto mode. `scope` is `me` (default) or `everyone` (needs the role for it). Paid apps cannot be installed without a licence; command-line tools open the CLI tile instead and are not installed here.",
            obj(json!({"id": {"type": "string"}, "scope": {"type": "string", "enum": ["me", "everyone"]}}), &["id"]),
            Risk::Ask,
            false,
        ),
        tool(
            "store_outdated",
            "List what is installed (skills, tools, MCP servers, command-line tools, themes, templates, driver packs) that has a newer version in the catalogs ON THIS DEVICE (no network; the person presses Check for updates in STORE to look for news), with installed and latest versions, where each came from, the changelog line, new permissions the update lists, and anything pinned, revoked or whose store is gone. Also lists platform services with an upgrade path. Read-only. Use it before store_update.",
            obj(json!({"all": {"type": "boolean"}}), &[]),
            Risk::Safe,
            true,
        ),
        tool(
            "store_update",
            "Update one installed item to the latest version from the store it came from (`id` from store_outdated, such as skill:meeting-notes), or every item that can be updated without a new decision (`all`: true). The person approves a card that names each item and its versions; this always asks, even in Auto mode, and is never run automatically. An update that asks for MORE permissions than the installed version is NOT applied by you: tell the person to update it in the STORE tile where the new permissions are shown. The new version is checked after installing and the old one is put back if it fails; the old version is kept for one rollback. Platform services are upgraded with platform_upgrade, driver packs with driver_install.",
            obj(json!({"id": {"type": "string"}, "all": {"type": "boolean"}}), &[]),
            Risk::Ask,
            false,
        ),
        tool(
            "store_remove",
            "Uninstall an installed item by `id` (from store_outdated or store_search). By default its data and vault entries are KEPT. `delete_data` true also deletes them and needs `confirm`, which must be the item's name typed by the person (ask with ask_user; never fill it in). `acknowledge_dependents` true only after the person was told what uses it. Always asks, even in Auto mode, and the card says exactly what is deleted and what is kept. Built-in items cannot be removed; platform services are removed with platform_remove.",
            obj(json!({"id": {"type": "string"}, "delete_data": {"type": "boolean"}, "confirm": {"type": "string"}, "acknowledge_dependents": {"type": "boolean"}}), &["id"]),
            Risk::Ask,
            false,
        ),
        tool(
            "store_rollback",
            "Go back to the previous version of an installed item, which the store keeps for one generation, by `id`. The item is then pinned at that version so the update does not come straight back. Always asks, even in Auto mode, and is never run automatically.",
            obj(json!({"id": {"type": "string"}}), &["id"]),
            Risk::Ask,
            false,
        ),
    ]
}

fn lives_now(svc: &Services, p: &Policy, user: &str) -> Vec<super::lifecycle::Life> {
    let st = Store::for_services(svc);
    st.lifecycle(svc, p, user, &st.local_catalogs(svc), &super::lifecycle::Ctx::default())
}

fn arg_bool(args: &Value, k: &str) -> bool {
    args.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// What an approval card for the writers says: the exact items and what changes.
fn describe_writer(svc: &Services, p: &Policy, user: &str, name: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).unwrap_or("");
    let lives = lives_now(svc, p, user);
    let st = Store::for_services(svc);
    if name == "store_update" && arg_bool(args, "all") {
        let rows = Store::update_all_preview(&lives);
        if rows.is_empty() {
            return "update everything that has an update (nothing has one now, as far as this device knows)".into();
        }
        return format!("UPDATE ALL: {}. Items that would need new permissions, are pinned or come from another store are skipped.", rows.iter().map(|(t, d, ok)| format!("{t} {d}{}", if *ok { "" } else { " [skipped]" })).collect::<Vec<_>>().join("; "));
    }
    let Ok(l) = Store::find_life(&lives, s("id")) else { return format!("{} “{}” (not found among the installed items)", name.trim_start_matches("store_"), s("id")) };
    match name {
        "store_update" => format!(
            "UPDATE “{}” from {} to {} (from {}){}. {} The old version is kept for one rollback; if the new one fails its check the old one is put back.",
            l.entry.title,
            l.entry.version,
            l.latest.clone().unwrap_or_else(|| "the newest".into()),
            if l.source_name.is_empty() { "its store".into() } else { l.source_name.clone() },
            if l.changelog.is_empty() { String::new() } else { format!(" — {}", l.changelog) },
            if l.added_permissions.is_empty() { "No new permissions are listed.".to_string() } else { format!("It lists NEW permissions ({}), which you cannot grant through the assistant: nothing will be changed; use the STORE tile.", l.added_permissions.join("; ")) }
        ),
        "store_remove" => {
            let plan = st.plan_remove(svc, user, &lives, l);
            let data = if arg_bool(args, "delete_data") { format!("ALSO DELETES its data ({}).", plan.data.iter().map(|d| d.0.clone()).chain(plan.vault.iter().map(|v| format!("vault entry {v}"))).collect::<Vec<_>>().join(", ")) } else { "Its data and vault entries are KEPT.".to_string() };
            format!("REMOVE “{}” {}: {} {} {}", l.entry.title, l.entry.version, plan.removes.join("; "), data, if plan.dependents.is_empty() { String::new() } else { format!("Used by: {}.", plan.dependents.join(", ")) })
        }
        _ => format!("ROLL BACK “{}” from {} to {} and pin it there", l.entry.title, l.entry.version, l.previous.clone().unwrap_or_else(|| "(no earlier version is kept)".into())),
    }
}

fn outdated_text(svc: &Services, p: &Policy, user: &str, all: bool) -> String {
    let st = Store::for_services(svc);
    let lives = lives_now(svc, p, user);
    let text = super::storecmd::list_text(&lives, &st.services(svc, user), !all);
    let mut extra = String::new();
    for l in lives.iter().filter(|l| l.has_update()) {
        extra.push_str(&format!("\n{}: {}{}", l.id, if l.changelog.is_empty() { "no changelog".to_string() } else { l.changelog.clone() }, if l.added_permissions.is_empty() { String::new() } else { format!(" · asks for MORE: {}", l.added_permissions.join("; ")) }));
    }
    format!("{text}{extra}")
}

fn actor(user: &str, p: &Policy) -> Actor {
    Actor::from_policy(user, p)
}

fn find<'a>(items: &'a [Item], id: &str) -> Option<&'a Item> {
    let id = id.trim().to_lowercase();
    items.iter().find(|i| i.entry.id == id || i.entry.name == id).or_else(|| items.iter().find(|i| i.entry.title().to_lowercase() == id))
}

/// What the person has arranged for an item (read-only here): their own title, tags and group.
fn mine(org: &super::organize::Org, i: &Item) -> String {
    let k = super::organize::item_key(&i.entry.kind, &i.entry.name);
    let mut out = String::new();
    if let Some(a) = org.alias_of(&k) {
        out.push_str(&format!(" · the person calls it “{a}”"));
    }
    if !org.tags_of(&k).is_empty() {
        out.push_str(&format!(" · tags: {}", org.tags_of(&k).join(", ")));
    }
    if let Some(g) = org.group_of(&k) {
        out.push_str(&format!(" · group: {}", g.name));
    }
    out
}

fn line(n: usize, i: &Item, org: &super::organize::Org) -> String {
    format!("{n}. {} [{}] {} · {} · {} · {}{}{}", i.entry.id, i.entry.kind, i.title(), i.verdict.level.label(), i.entry.meta.pricing.label(), i.state.label(), if i.entry.demo { " (made-up example)" } else { "" }, mine(org, i))
}

/// One line for the approval prompt (it uses what is already on the device, so it is quick and needs no network).
pub fn describe(svc: &Services, p: &Policy, user: &str, name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).unwrap_or("");
    match name {
        "store_search" => Some(format!("search the store for “{}”", s("query"))),
        "store_info" => Some(format!("look up {} in the store", s("id"))),
        "store_install" => {
            let st = Store::for_services(svc);
            let loaded = st.local_catalogs(svc);
            let items = st.items(svc, &Actor::system(), &loaded);
            let who = if s("scope") == "everyone" { "for everyone" } else { "for you" };
            Some(match find(&items, s("id")) {
                Some(i) => {
                    let perms = if i.entry.meta.permissions.is_empty() { "no extra access".to_string() } else { i.entry.meta.permissions.iter().filter_map(|p| super::manifest::Permission::parse(p).ok()).map(|p| p.words(user)).collect::<Vec<_>>().join(", ") };
                    format!("install “{}” {} {who}: {} ({}), by {}, {}; it asks for: {perms}. You will see the full review.", i.title(), i.entry.version, i.verdict.level.label(), i.verdict.label, if i.entry.meta.publisher.name.is_empty() { "an unknown publisher" } else { &i.entry.meta.publisher.name }, i.entry.meta.pricing.label())
                }
                None => format!("install “{}” from the store {who} (not found on this device)", s("id")),
            })
        }
        "store_update" | "store_remove" | "store_rollback" => Some(describe_writer(svc, p, user, name, args)),
        _ => None,
    }
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy, user: &str) -> Result<String, String> {
    if !offered(p) {
        return Err("your account may not use the store".into());
    }
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let st = Store::for_services(svc);
    let a = actor(user, p);
    let loaded = st.local_catalogs(svc);
    let items = st.items(svc, &a, &loaded);
    let org = super::organize::Org::load(st.dir(), user);
    match name {
        "store_search" => {
            let f = Filter { query: s("query").unwrap_or("").to_string(), kind: s("kind").map(str::to_string), price: if args.get("free").and_then(Value::as_bool) == Some(true) { PriceFilter::Free } else { PriceFilter::All }, ..Default::default() };
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(8).clamp(1, 12) as usize;
            let mut found = filter(&items, &f);
            // what the person calls things, tags and groups also find them
            if !f.query.trim().is_empty() {
                let words: Vec<String> = f.query.to_lowercase().split_whitespace().map(str::to_string).collect();
                for i in &items {
                    let hay = org.hay(&super::organize::item_key(&i.entry.kind, &i.entry.name));
                    if !found.iter().any(|x| std::ptr::eq(*x, i)) && words.iter().all(|w| hay.contains(w.as_str())) {
                        found.push(i);
                    }
                }
            }
            if found.is_empty() {
                return Ok(format!("Nothing in the store matches “{}”. (Only catalogs on this device were searched; the person can add one in STORE, Settings.)", f.query));
            }
            let mut out = format!("{} result(s) for “{}”:\n", found.len().min(limit), f.query);
            for (n, i) in found.iter().take(limit).enumerate() {
                out.push_str(&line(n + 1, i, &org));
                out.push('\n');
            }
            Ok(out)
        }
        "store_info" => {
            let i = find(&items, s("id").ok_or("give the app's id")?).ok_or("no such app in the store catalogs on this device")?;
            let e = &i.entry;
            let perms = if e.meta.permissions.is_empty() { "none listed".to_string() } else { e.meta.permissions.iter().filter_map(|p| super::manifest::Permission::parse(p).ok()).map(|p| p.words(user)).collect::<Vec<_>>().join("; ") };
            Ok(format!(
                "{} ({}) {} · {}\n{}: {}\n{}\npublisher: {}\nprice: {}\npermissions: {}\nlicense: {} · size {} bytes\nstate: {}\n\n{}\n{}",
                i.title(),
                e.kind,
                e.version,
                e.id,
                i.verdict.level.label(),
                i.verdict.level.plain(),
                i.verdict.label,
                if e.meta.publisher.name.is_empty() { "unknown" } else { &e.meta.publisher.name },
                e.meta.pricing.label(),
                perms,
                if e.license.is_empty() { "-" } else { &e.license },
                e.size,
                i.state.label(),
                e.meta.summary,
                if e.demo { "This is a made-up example and cannot be installed." } else { "" }
            ) + &mine(&org, i).replacen(" · ", "\nthe person's arrangement: ", 1))
        }
        "store_install" => {
            let i = find(&items, s("id").ok_or("give the app's id")?).ok_or("no such app in the store catalogs on this device")?;
            let scope = if s("scope") == Some("everyone") { Scope::Shared } else { a.default_scope() };
            let fetch = HttpFetch(svc.quick_upstream());
            let known = Store::known_names(&items);
            let r = st.review_item(svc, &a, i, &loaded, &fetch, scope, &known)?;
            if let Some(why) = &r.refusal {
                return Err(format!("Not installed: {why}"));
            }
            // what the person was asked about must be what is installed
            if r.verdict.level != i.verdict.level && !(i.source == "builtin") {
                return Err(format!("Not installed: the package is {} but the catalog listed it as {}. Open the STORE tile to look at it.", r.verdict.level.label(), i.verdict.level.label()));
            }
            if i.source != "builtin" && canonical_set(&i.entry.meta.permissions) != r.permissions.iter().map(|p| p.canonical.clone()).collect::<Vec<_>>() {
                return Err("Not installed: the package asks for different permissions than the catalog listed. Open the STORE tile to look at it.".into());
            }
            if matches!(i.state, State::Installed) {
                return Err("It is already installed.".into());
            }
            let rec = st.install(svc, &a, &r, Confirm::all()).map_err(|e| format!("Not installed: {e}"))?;
            Ok(format!("Installed {} {} ({}). {}", rec.title, rec.version, r.verdict.level.label(), if r.permissions.is_empty() { "It asks for no extra access.".to_string() } else { format!("It may: {}.", r.permissions.iter().map(|p| p.words.clone()).collect::<Vec<_>>().join("; ")) }))
        }
        "store_outdated" => Ok(outdated_text(svc, p, user, arg_bool(args, "all"))),
        "store_update" => {
            let fetch = HttpFetch(svc.quick_upstream());
            let audit = |_: &str, _: &str| {};
            if arg_bool(args, "all") {
                let rep = st.update_all(svc, p, user, &loaded, &fetch, &audit, &mut |_| {});
                return Ok(if rep.items.is_empty() { "Nothing to update.".into() } else { rep.summary() });
            }
            let lives = st.lifecycle(svc, p, user, &loaded, &super::lifecycle::Ctx::default());
            let l = Store::find_life(&lives, s("id").ok_or("give the id (from store_outdated)")?)?;
            if l.entry.ty == super::installed::Type::Driver {
                return Err("Driver packs change the whole machine: use driver_install, or the Drivers page of STORE.".into());
            }
            let plan = st.plan_update(svc, &a, l, &loaded, &fetch).map_err(|e| e.to_string())?;
            // the assistant never grants new permissions or accepts warnings: that is the person's decision in the tile
            st.apply_update(svc, &a, &plan, Confirm::default(), &audit).map_err(|e| match e {
                super::lifecycle::LifeError::NeedsConfirm(m) => format!("Not updated: {m} Tell the person to update it in the STORE tile, where the new permissions are shown."),
                other => other.to_string(),
            })
        }
        "store_rollback" => {
            let lives = st.lifecycle(svc, p, user, &loaded, &super::lifecycle::Ctx::default());
            let l = Store::find_life(&lives, s("id").ok_or("give the id")?)?;
            st.rollback(svc, &a, l, &|_, _| {}).map_err(|e| e.to_string())
        }
        "store_remove" => {
            let lives = st.lifecycle(svc, p, user, &loaded, &super::lifecycle::Ctx::default());
            let l = Store::find_life(&lives, s("id").ok_or("give the id")?)?;
            let opts = super::lifecycle::RemoveOpts { delete_data: arg_bool(args, "delete_data"), typed: s("confirm").unwrap_or("").to_string(), ack_dependents: arg_bool(args, "acknowledge_dependents") };
            st.remove_item(svc, p, user, &lives, l, &opts, &|_, _| {}).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown tool `{name}`")),
    }
}

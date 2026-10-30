//! The routes added for diagnosis: `kube` (scope kube), `state`, `inbox` and `diag` (scope debug), and `click` (scope input). The scope
//! is checked by [`super::api::handle`] before any of these run; each one logs what it did (verb and kind, counts, never contents).

use serde_json::{json, Value};

use super::api::{Ctx, Out};
use super::diag;
use super::input::{self, InputEv};
use super::kube;
use super::scopes::{self, Scope};
use super::state;

/// How long a route waits for the shell to draw a frame.
pub const SCREEN_WAIT_MS: u64 = 2500;
/// Titles of the inbox in one answer, and the longest title and body.
pub const MAX_INBOX: usize = 100;
pub const MAX_TITLE: usize = 120;
pub const MAX_BODY: usize = 300;

pub fn route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    match path {
        "/v1/dev/kube" => kube_route(cx, action, body),
        "/v1/dev/state" => state_route(cx, action),
        "/v1/dev/click" => click_route(cx, action, body),
        "/v1/dev/inbox" => inbox_route(cx, action, q("bodies") == "1"),
        "/v1/dev/diag" => diag_route(cx, action),
        _ => Out::err(404, "not found"),
    }
}

fn kube_route(cx: &Ctx, action: &str, body: &[u8]) -> Out {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Out::err(400, "bad request") };
    let args: Vec<String> = v["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let plan = match kube::plan(&args) {
        Ok(p) => p,
        Err(e) => {
            // the verb is all the log keeps of a refused command
            let verb: String = args.first().map(|v| v.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(20).collect()).unwrap_or_default();
            cx.log(action, &format!("refused {verb}: {}", e.chars().take(120).collect::<String>()), false);
            return Out::err(403, &e);
        }
    };
    let Some(k) = cx.kube else { return Out::err(503, "the cluster is not available here") };
    match kube::run(k, &plan) {
        Ok(v) => {
            cx.did(action, &plan.audit());
            Out::json(200, &v)
        }
        Err(e) => {
            cx.log(action, &format!("{}: {e}", plan.audit()), false);
            Out::err(502, &e)
        }
    }
}

fn screen(cx: &Ctx) -> Result<state::Snapshot, String> {
    state::fetch(cx.dm.data_dir(), SCREEN_WAIT_MS)
}

fn state_route(cx: &Ctx, action: &str) -> Out {
    match screen(cx) {
        Ok(s) => {
            cx.log(action, &format!("{}: {} lines, {} buttons{}", s.app, s.lines.len(), s.buttons.len(), if s.text_hidden { ", text hidden" } else { "" }), true);
            Out::json(200, &s.json())
        }
        Err(e) => {
            cx.log(action, &e, false);
            Out::err(503, &e)
        }
    }
}

fn click_route(cx: &Ctx, action: &str, body: &[u8]) -> Out {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Out::err(400, "bad request") };
    let label = v["label"].as_str().unwrap_or("");
    if label.trim().is_empty() || label.len() > 200 {
        return Out::err(422, "label: the name of a button (see `iyakki dev state`)");
    }
    if input::secret_focus(cx.dm.data_dir()) && !(cx.key.has(Scope::InputPassword) && scopes::check(cx.key, cx.policy, Scope::InputPassword).is_ok()) {
        return cx.refuse(action, "a password field has focus on the device: input is refused unless the computer holds the input:password scope");
    }
    let snap = match screen(cx) {
        Ok(s) => s,
        Err(e) => {
            cx.log(action, &e, false);
            return Out::err(503, &e);
        }
    };
    match snap.target(label) {
        Ok((x, y, name)) => match input::push(cx.dm.data_dir(), &[InputEv::Tap(x, y)]) {
            Ok(_) => {
                cx.did(action, &format!("click {:?} at {x},{y} in {}", name.chars().take(60).collect::<String>(), snap.app));
                Out::json(200, &json!({ "clicked": name, "x": x, "y": y, "app": snap.app }))
            }
            Err(e) => {
                cx.log(action, &e, false);
                Out::err(429, &e)
            }
        },
        Err(e) => {
            cx.log(action, &format!("refused: {}", e.chars().take(150).collect::<String>()), false);
            Out::err(if e.contains("secret") { 403 } else { 404 }, &e)
        }
    }
}

/// The inbox as numbers and titles: counts by kind and by source, then the newest titles. Bodies only when asked for (and allowed).
pub fn inbox_report(items: &[crate::inbox::Notification], bodies: bool) -> Value {
    let mut by_kind: Vec<(String, usize)> = Vec::new();
    let mut by_source: Vec<(String, usize)> = Vec::new();
    let bump = |v: &mut Vec<(String, usize)>, k: String| match v.iter_mut().find(|(n, _)| *n == k) {
        Some(e) => e.1 += 1,
        None => v.push((k, 1)),
    };
    for n in items {
        bump(&mut by_kind, n.kind.name().to_string());
        bump(&mut by_source, if n.source.is_empty() { "(none)".to_string() } else { n.source.chars().take(40).collect() });
    }
    by_source.sort_by(|a, b| b.1.cmp(&a.1));
    by_source.truncate(20);
    let clip = |s: &str, n: usize| -> String { super::debug::scrub(&s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(n).collect::<String>()) };
    let list: Vec<Value> = items
        .iter()
        .take(MAX_INBOX)
        .map(|n| {
            let mut o = json!({ "id": n.id, "time": n.time, "kind": n.kind.name(), "source": n.source.chars().take(40).collect::<String>(), "read": n.read, "flagged": n.flagged, "title": clip(&n.title, MAX_TITLE) });
            if bodies {
                o["body"] = json!(clip(&n.body, MAX_BODY));
            }
            o
        })
        .collect();
    json!({
        "total": items.len(), "unread": items.iter().filter(|n| !n.read).count(),
        "by_kind": by_kind.iter().map(|(k, n)| json!({ "kind": k, "count": n })).collect::<Vec<_>>(),
        "by_source": by_source.iter().map(|(k, n)| json!({ "source": k, "count": n })).collect::<Vec<_>>(),
        "items": list, "bodies": bodies, "shown": items.len().min(MAX_INBOX),
    })
}

fn inbox_route(cx: &Ctx, action: &str, bodies: bool) -> Out {
    // message bodies are the person's: they need the scope that also reads the data folder (administrators only)
    if bodies {
        if let Err(e) = scopes::check(cx.key, cx.policy, Scope::FilesSystem) {
            return cx.refuse(action, &format!("--bodies needs the files:system scope as well: {e}"));
        }
    }
    let Some(svc) = cx.svc else { return Out::err(503, "the inbox is not available here") };
    let items = svc.inbox.list(&cx.key.account);
    let v = inbox_report(&items, bodies);
    cx.log(action, &format!("{} notifications{}", items.len(), if bodies { ", with bodies" } else { "" }), true);
    Out::json(200, &v)
}

fn diag_route(cx: &Ctx, action: &str) -> Out {
    // the screen is optional: a device with nobody signed in still gives the rest
    let snap = state::fetch(cx.dm.data_dir(), 800).ok();
    let text = diag::bundle(&diag::Input { paths: cx.paths, hostname: cx.hostname, data_dir: cx.dm.data_dir(), firewall_file: &cx.firewall_file, audit_tail: cx.audit_tail, kube: if scopes::check(cx.key, cx.policy, Scope::Kube).is_ok() { cx.kube } else { None }, screen: snap.as_ref(), admin: cx.policy.manage_settings });
    cx.log(action, &format!("{} bytes", text.len()), true);
    Out::json(200, &json!({ "text": text }))
}

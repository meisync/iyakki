//! The routes of developer mode beyond install/run/logs/screenshot, as plain functions of (who, method, path, query, body) so they are
//! tested without a network; the listener (`iyakki_remote::devserver`) only authenticates the request and calls [`handle`].
//!
//! Every route is in [`ROUTES`] with the scope it needs; [`scope_of`] is the single place the listener, the tests and the documents
//! read that from. [`handle`] checks the scope (and the account's role) BEFORE anything else happens, then logs the action in the
//! developer-mode activity log and the system audit log.

use std::path::PathBuf;

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};

use super::debug::{self, Paths};
use super::files::{self, Sandbox};
use super::input::{self, InputEv};
use super::scopes::{self, Scope};
use super::{DevKey, DevMode};

/// The most one speed-check request may carry.
pub const SPEED_MAX_BODY: usize = 8 << 20;

/// Every signed route and what it needs (`None`: any paired computer).
pub const ROUTES: &[(&str, &str, Option<Scope>)] = &[
    ("GET", "/v1/dev/whoami", None),
    // the throughput check of `iyakki dev usb` (any paired computer): counts the bytes of the body and throws them away
    ("POST", "/v1/dev/speed", None),
    ("POST", "/v1/dev/install", Some(Scope::Install)),
    ("POST", "/v1/dev/uninstall", Some(Scope::Install)),
    ("GET", "/v1/dev/list", Some(Scope::Install)),
    ("GET", "/v1/dev/logs", Some(Scope::Install)),
    ("POST", "/v1/dev/open", Some(Scope::Install)),
    ("GET", "/v1/dev/screenshot", Some(Scope::Install)),
    ("GET", "/v1/dev/info", Some(Scope::Debug)),
    ("GET", "/v1/dev/ps", Some(Scope::Debug)),
    ("GET", "/v1/dev/dmesg", Some(Scope::Debug)),
    ("GET", "/v1/dev/syslog", Some(Scope::Debug)),
    ("GET", "/v1/dev/status", Some(Scope::Debug)),
    ("GET", "/v1/dev/state", Some(Scope::Debug)),
    ("GET", "/v1/dev/inbox", Some(Scope::Debug)),
    ("GET", "/v1/dev/diag", Some(Scope::Debug)),
    ("POST", "/v1/dev/kube", Some(Scope::Kube)),
    ("POST", "/v1/dev/click", Some(Scope::Input)),
    ("GET", "/v1/dev/files/ls", Some(Scope::Files)),
    ("GET", "/v1/dev/files/stat", Some(Scope::Files)),
    ("GET", "/v1/dev/files/get", Some(Scope::Files)),
    ("POST", "/v1/dev/files/put", Some(Scope::Files)),
    ("POST", "/v1/dev/files/rm", Some(Scope::Files)),
    ("POST", "/v1/dev/files/mkdir", Some(Scope::Files)),
    ("POST", "/v1/dev/input", Some(Scope::Input)),
    ("GET", "/v1/dev/config/list", Some(Scope::Config)),
    ("GET", "/v1/dev/config/get", Some(Scope::Config)),
    ("POST", "/v1/dev/config/set", Some(Scope::Config)),
    ("GET", "/v1/dev/config/export", Some(Scope::Config)),
    ("POST", "/v1/dev/config/import", Some(Scope::Config)),
    ("POST", "/v1/dev/config/provider", Some(Scope::Config)),
    ("POST", "/v1/dev/config/call", Some(Scope::Config)),
    ("GET", "/v1/dev/app/info", Some(Scope::Config)),
    ("GET", "/v1/dev/app/permissions", Some(Scope::Config)),
    ("GET", "/v1/dev/app/data", Some(Scope::Config)),
    ("POST", "/v1/dev/app/data/set", Some(Scope::Config)),
    ("POST", "/v1/dev/app/data/rm", Some(Scope::Config)),
    ("POST", "/v1/dev/app/clear", Some(Scope::Config)),
    ("POST", "/v1/dev/app/reset", Some(Scope::Config)),
    ("POST", "/v1/dev/app/grant", Some(Scope::Config)),
    ("POST", "/v1/dev/app/revoke", Some(Scope::Config)),
    // the scope depends on what is asked for (shell or power): checked inside
    ("POST", "/v1/dev/grant/ask", None),
    ("POST", "/v1/dev/shell/open", Some(Scope::Shell)),
    ("POST", "/v1/dev/shell/exec", Some(Scope::Shell)),
    ("GET", "/v1/dev/shell/read", Some(Scope::Shell)),
    ("POST", "/v1/dev/shell/stdin", Some(Scope::Shell)),
    ("POST", "/v1/dev/shell/signal", Some(Scope::Shell)),
    ("POST", "/v1/dev/shell/close", Some(Scope::Shell)),
    ("POST", "/v1/dev/forward/start", Some(Scope::Forward)),
    ("POST", "/v1/dev/forward/connect", Some(Scope::Forward)),
    ("POST", "/v1/dev/forward/send", Some(Scope::Forward)),
    ("GET", "/v1/dev/forward/recv", Some(Scope::Forward)),
    ("POST", "/v1/dev/forward/close", Some(Scope::Forward)),
    ("POST", "/v1/dev/forward/stop", Some(Scope::Forward)),
    ("POST", "/v1/dev/update/plan", Some(Scope::Update)),
    ("POST", "/v1/dev/update/stage", Some(Scope::Update)),
    ("GET", "/v1/dev/update/staged", Some(Scope::Update)),
    ("POST", "/v1/dev/update/apply", Some(Scope::Update)),
    ("POST", "/v1/dev/update/abort", Some(Scope::Update)),
    ("GET", "/v1/dev/update/status", Some(Scope::Update)),
    ("POST", "/v1/dev/patch/plan", Some(Scope::Update)),
    ("POST", "/v1/dev/patch/stage", Some(Scope::Update)),
    ("POST", "/v1/dev/patch/apply", Some(Scope::Update)),
    ("POST", "/v1/dev/patch/remove", Some(Scope::Update)),
    ("GET", "/v1/dev/patch/list", Some(Scope::Update)),
    ("POST", "/v1/dev/power/reboot", Some(Scope::Power)),
    ("POST", "/v1/dev/power/poweroff", Some(Scope::Power)),
];

/// The scope a route needs: `Some(None)` for a route any paired computer may call, `None` for a route that does not exist.
pub fn scope_of(method: &str, path: &str) -> Option<Option<Scope>> {
    ROUTES.iter().find(|(m, p, _)| *m == method && *p == path).map(|(_, _, s)| *s)
}

/// An answer: status, content type, body.
#[derive(Debug, Clone)]
pub struct Out {
    pub status: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
}

impl Out {
    pub fn json(status: u16, v: &Value) -> Out {
        Out { status, ctype: "application/json", body: serde_json::to_vec(v).unwrap_or_default() }
    }

    pub fn err(status: u16, msg: &str) -> Out {
        Out::json(status, &json!({ "error": msg }))
    }

    pub fn bytes(body: Vec<u8>) -> Out {
        Out { status: 200, ctype: "application/octet-stream", body }
    }
}

/// Who is asking, and what the device lends this call.
pub struct Ctx<'a> {
    pub dm: &'a DevMode,
    pub key: &'a DevKey,
    pub policy: &'a Policy,
    pub role: &'a str,
    pub hostname: &'a str,
    pub paths: &'a Paths,
    /// The newest n lines of the system audit log.
    pub audit_tail: &'a dyn Fn(usize) -> Vec<String>,
    /// Write to the system audit log: (action, detail). The computer and account are added by the caller.
    pub audit: &'a dyn Fn(&str, &str),
    pub firewall_file: PathBuf,
    /// The account's home folder when it is not the usual one (tests); None on a device.
    pub home: Option<PathBuf>,
    /// The services of the device (settings, vault, store); None where there are none (some tests).
    pub svc: Option<&'a crate::services::Services>,
    /// Settings that live outside `settings.json` (host name, display size, firewall).
    pub ext: Option<&'a dyn super::config::ConfigExt>,
    /// The port developer mode listens on (a forward to it is refused).
    pub own_port: u16,
    /// Restart (true) or switch off (false) the device, a moment after the answer is sent.
    pub power: &'a dyn Fn(bool),
    /// The boot partition (system update); None where there is none.
    pub boot: Option<&'a dyn super::update::BootMount>,
    /// The text of `/etc/iyakki-release`.
    pub release: &'a str,
    /// The data folder is RAM only.
    pub volatile: bool,
    /// Bytes free on the data partition, when it is not to be read from the file system (tests).
    pub data_free: Option<u64>,
    /// The device's managed `kubectl` (scope `kube`); None where there is no cluster (some tests).
    pub kube: Option<&'a dyn super::kube::Kubectl>,
}

impl Ctx<'_> {
    pub(super) fn log(&self, action: &str, detail: &str, ok: bool) {
        self.dm.log(&self.key.name, &self.key.account, action, detail, ok);
    }

    /// A refusal: logged as refused, answered 403.
    pub(super) fn refuse(&self, action: &str, why: &str) -> Out {
        self.log(action, why, false);
        Out::err(403, why)
    }

    /// An action that changes something: activity log and audit log.
    pub(super) fn did(&self, action: &str, detail: &str) {
        self.log(action, detail, true);
        (self.audit)(&format!("devmode.{}", action.trim_start_matches("dev.")), detail);
    }

    fn admin(&self) -> bool {
        self.policy.manage_settings
    }

    pub(super) fn sandbox(&self) -> Sandbox {
        Sandbox::for_account(self.policy, &self.key.account, self.dm.data_dir(), self.key.has(Scope::FilesSystem) && scopes::check(self.key, self.policy, Scope::FilesSystem).is_ok(), self.home.as_deref())
    }
}

fn num(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

/// Serve a route of this module. `None`: not one of mine (the caller answers 404). The scope is checked first.
pub fn handle(cx: &Ctx, method: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Option<Out> {
    let need = scope_of(method, path)?;
    let action = format!("dev.{}", path.trim_start_matches("/v1/dev/").replace('/', "."));
    if let Some(s) = need {
        if let Err(e) = scopes::check(cx.key, cx.policy, s) {
            return Some(cx.refuse(&action, &e));
        }
    }
    match path {
        "/v1/dev/speed" => {
            if body.len() > SPEED_MAX_BODY {
                return Some(cx.refuse(&action, "the speed check sends at most 8 MiB per request"));
            }
            Some(Out::json(200, &json!({ "received": body.len() })))
        }
        "/v1/dev/info" => {
            let v = debug::info(cx.paths, cx.hostname);
            cx.log(&action, "", true);
            Some(Out::json(200, &v))
        }
        "/v1/dev/ps" => {
            let all = cx.admin();
            let v = debug::ps(cx.paths, all, &cx.key.account);
            cx.log(&action, if all { "all processes" } else { "own processes" }, true);
            Some(Out::json(200, &v))
        }
        "/v1/dev/dmesg" => {
            if !cx.admin() {
                return Some(cx.refuse(&action, "the kernel log is for administrator accounts"));
            }
            let grep = q("grep");
            let v = debug::kmsg_json(cx.paths, num(&q("since")).unwrap_or(0), &grep, num(&q("max")).unwrap_or(500) as usize);
            if num(&q("since")).unwrap_or(0) == 0 {
                cx.log(&action, &format!("grep {:?}", grep.chars().take(40).collect::<String>()), true);
            }
            Some(Out::json(200, &v))
        }
        "/v1/dev/syslog" => {
            if !cx.admin() {
                return Some(cx.refuse(&action, "the system logs are for administrator accounts"));
            }
            let src = q("src");
            match debug::syslog(cx.paths, if src.is_empty() { "shell" } else { &src }, num(&q("n")).unwrap_or(200) as usize, cx.audit_tail) {
                Ok(v) => {
                    cx.log(&action, &src, true);
                    Some(Out::json(200, &v))
                }
                Err(e) => Some(Out::err(422, &e)),
            }
        }
        "/v1/dev/status" => {
            let v = debug::status(cx.paths, cx.dm.data_dir(), &cx.firewall_file);
            cx.log(&action, "", true);
            Some(Out::json(200, &v))
        }
        "/v1/dev/state" | "/v1/dev/inbox" | "/v1/dev/diag" | "/v1/dev/kube" | "/v1/dev/click" => Some(super::api_diag::route(cx, &action, path, q, body)),
        p if p.starts_with("/v1/dev/files/") => Some(files_route(cx, &action, p, q, body)),
        "/v1/dev/input" => Some(input_route(cx, &action, body)),
        "/v1/dev/grant/ask" => Some(grant_route(cx, &action, body)),
        p if p.starts_with("/v1/dev/shell/") => Some(shell_route(cx, &action, p, q, body)),
        p if p.starts_with("/v1/dev/forward/") => Some(forward_route(cx, &action, p, q, body)),
        p if p.starts_with("/v1/dev/update/") => Some(update_route(cx, &action, p, q, body)),
        p if p.starts_with("/v1/dev/patch/") => Some(patch_route(cx, &action, p, q, body)),
        p if p.starts_with("/v1/dev/power/") => Some(power_route(cx, &action, p)),
        p if p.starts_with("/v1/dev/config/") || p.starts_with("/v1/dev/app/") => Some(config_route(cx, &action, p, q, body)),
        _ => None,
    }
}

fn files_route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    let sb = cx.sandbox();
    let p = q("path");
    let fail = |e: String| {
        cx.log(action, &format!("refused {}: {e}", p.chars().take(80).collect::<String>()), false);
        Out::err(403, &e)
    };
    match path {
        "/v1/dev/files/ls" => match files::ls(&sb, &p) {
            Ok(v) => {
                cx.log(action, &p, true);
                Out::json(200, &v)
            }
            Err(e) => fail(e),
        },
        "/v1/dev/files/stat" => match files::stat(&sb, &p, q("hash") == "1") {
            Ok(v) => Out::json(200, &v),
            Err(e) => fail(e),
        },
        "/v1/dev/files/get" => {
            let offset = num(&q("offset")).unwrap_or(0);
            match files::get(&sb, &p, offset, num(&q("len")).map_or(files::MAX_CHUNK, |n| n as usize)) {
                Ok(b) => {
                    if offset == 0 {
                        cx.did(action, &format!("pull {p}"));
                    }
                    Out::bytes(b)
                }
                Err(e) => fail(e),
            }
        }
        "/v1/dev/files/put" => {
            let offset = num(&q("offset")).unwrap_or(0);
            let mode = num(&q("mode")).map(|m| m as u32);
            match files::put(&sb, &p, offset, num(&q("total")).unwrap_or(0), &q("sha256"), q("overwrite") == "1", mode, body) {
                Ok(v) => {
                    if v["done"].as_bool() == Some(true) {
                        cx.did(action, &format!("push {} ({} bytes)", v["path"].as_str().unwrap_or(&p), v["have"]));
                    }
                    Out::json(200, &v)
                }
                Err((code, v)) => {
                    cx.log(action, &format!("refused {}: {}", p.chars().take(80).collect::<String>(), v["error"].as_str().unwrap_or("")), false);
                    Out::json(code, &v)
                }
            }
        }
        "/v1/dev/files/rm" => match files::rm(&sb, &p, q("recursive") == "1") {
            Ok(v) => {
                cx.did(action, &format!("remove {}", v["removed"].as_str().unwrap_or(&p)));
                Out::json(200, &v)
            }
            Err(e) => fail(e),
        },
        "/v1/dev/files/mkdir" => match files::mkdir(&sb, &p) {
            Ok(v) => {
                cx.did(action, &format!("mkdir {}", v["path"].as_str().unwrap_or(&p)));
                Out::json(200, &v)
            }
            Err(e) => fail(e),
        },
        _ => Out::err(404, "not found"),
    }
}

fn input_route(cx: &Ctx, action: &str, body: &[u8]) -> Out {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Out::err(400, "bad request") };
    let i = |k: &str| v[k].as_i64().and_then(|n| i32::try_from(n).ok());
    let ev = match v["kind"].as_str().unwrap_or("") {
        "tap" => match (i("x"), i("y")) {
            (Some(x), Some(y)) => InputEv::Tap(x, y),
            _ => return Out::err(422, "tap needs x and y"),
        },
        "swipe" => match (i("x1"), i("y1"), i("x2"), i("y2")) {
            (Some(a), Some(b), Some(c), Some(d)) => InputEv::Swipe(a, b, c, d, v["ms"].as_u64().unwrap_or(300).min(5000) as u32),
            _ => return Out::err(422, "swipe needs x1 y1 x2 y2"),
        },
        "key" => InputEv::Key(v["name"].as_str().unwrap_or("").to_string()),
        "text" => InputEv::Text(v["text"].as_str().unwrap_or("").to_string()),
        other => return Out::err(422, &format!("unknown input {other:?}: tap, swipe, key or text")),
    };
    if let Err(e) = input::validate(&ev) {
        return Out::err(422, &e);
    }
    if input::secret_focus(cx.dm.data_dir()) && !(cx.key.has(Scope::InputPassword) && scopes::check(cx.key, cx.policy, Scope::InputPassword).is_ok()) {
        return cx.refuse(action, "a password field has focus on the device: input is refused unless the computer holds the input:password scope");
    }
    match input::push(cx.dm.data_dir(), &[ev.clone()]) {
        Ok(_) => {
            // text is never written to the log (it may be a secret); the kind and the length are
            let what = match &ev {
                InputEv::Tap(x, y) => format!("tap {x} {y}"),
                InputEv::Swipe(a, b, c, d, _) => format!("swipe {a},{b} to {c},{d}"),
                InputEv::Key(k) => format!("key {k}"),
                InputEv::Text(t) => format!("text of {} characters", t.chars().count()),
            };
            cx.did(action, &what);
            Out::json(200, &json!({ "queued": true }))
        }
        Err(e) => {
            cx.log(action, &e, false);
            Out::err(429, &e)
        }
    }
}

fn config_route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    let Some(svc) = cx.svc else { return Out::err(503, "settings are not available here") };
    let (acct, who) = (cx.key.account.as_str(), cx.key.name.as_str());
    let json_body = || serde_json::from_slice::<Value>(body).map_err(|_| "the request body is not JSON".to_string());
    let reply = |r: Result<Value, String>, what: &dyn Fn(&Value) -> String, changes: bool| match r {
        Ok(v) => {
            if changes {
                cx.did(action, &what(&v));
            } else {
                cx.log(action, &what(&v), true);
            }
            Out::json(200, &v)
        }
        Err(e) => {
            cx.log(action, &format!("refused: {}", e.chars().take(150).collect::<String>()), false);
            Out::err(422, &e)
        }
    };
    let app = q("app");
    match path {
        "/v1/dev/config/list" => reply(Ok(super::config::list(svc, cx.ext, acct)), &|_| String::new(), false),
        "/v1/dev/config/get" => reply(super::config::get(svc, cx.ext, cx.policy, acct, &q("key")), &|_| q("key"), false),
        "/v1/dev/config/set" => {
            let r = json_body().and_then(|v| {
                let key = v["key"].as_str().unwrap_or("").to_string();
                super::config::set(svc, cx.ext, cx.policy, acct, who, &key, &v["value"]).map(|m| json!({ "message": m, "key": key }))
            });
            reply(r, &|v| v["message"].as_str().unwrap_or("").to_string(), true)
        }
        "/v1/dev/config/export" => reply(super::config::export(svc, cx.ext, cx.policy, acct), &|_| "profile exported".into(), false),
        "/v1/dev/config/import" => {
            let r = json_body().and_then(|v| super::config::import(svc, cx.ext, cx.policy, acct, who, &v));
            reply(r, &|v| format!("profile imported: {} settings applied, providers added {}", v["applied"], v["providers_added"]), true)
        }
        "/v1/dev/config/provider" => {
            // the key travels in the body and is never echoed or logged
            let r = json_body().and_then(|v| super::config::provider(svc, cx.policy, &v));
            reply(r, &|v| v.as_object().map(|o| o.iter().filter(|(k, _)| *k != "key").map(|(k, x)| format!("{k}={}", x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string()))).collect::<Vec<_>>().join(" ")).unwrap_or_default(), true)
        }
        "/v1/dev/config/call" => {
            let r = json_body().and_then(|v| {
                if !cx.policy.manage_settings {
                    return Err("this needs an administrator account".to_string());
                }
                cx.ext.and_then(|e| e.call(v["action"].as_str().unwrap_or(""), &v["args"], who)).unwrap_or_else(|| Err(format!("{:?} is not an action a computer may call", v["action"].as_str().unwrap_or(""))))
            });
            reply(r, &|v| v["message"].as_str().unwrap_or("done").to_string(), true)
        }
        "/v1/dev/app/info" => reply(super::app::info(svc, cx.policy, acct, &app), &|_| app.clone(), false),
        "/v1/dev/app/permissions" => reply(super::app::permissions(svc, cx.policy, acct, &app), &|_| app.clone(), false),
        "/v1/dev/app/data" => reply(super::app::data(svc, cx.policy, acct, &app, &q("key")), &|_| format!("{app} {}", q("key")), false),
        "/v1/dev/app/data/set" => {
            let r = String::from_utf8(body.to_vec()).map_err(|_| "the value is text".to_string()).and_then(|v| super::app::set_data(svc, cx.policy, acct, &app, &q("key"), &v));
            reply(r, &|v| format!("{app} set {} ({} bytes)", q("key"), v["bytes"]), true)
        }
        "/v1/dev/app/data/rm" => reply(super::app::remove_data(svc, cx.policy, acct, &app, &q("key")), &|_| format!("{app} removed {}", q("key")), true),
        "/v1/dev/app/clear" => reply(super::app::clear_data(svc, cx.policy, acct, &app), &|v| format!("{app} cleared {} keys", v["cleared_keys"]), true),
        "/v1/dev/app/reset" => reply(super::app::reset(svc, cx.dm, cx.policy, acct, &app), &|v| format!("{app} reset ({} keys cleared)", v["cleared_keys"]), true),
        "/v1/dev/app/grant" | "/v1/dev/app/revoke" => {
            cx.log(action, &format!("{app}: not available"), false);
            Out::err(501, "an app holds exactly the capabilities its manifest declares (see `app permissions`): change the manifest and install it again; there is no separate grant to switch")
        }
        _ => Out::err(404, "not found"),
    }
}

fn grant_json(st: &super::grants::ShellStatus) -> Value {
    use super::grants::ShellStatus as S;
    match st {
        S::None => json!({ "state": "none" }),
        S::Pending => json!({ "state": "pending" }),
        S::Denied => json!({ "state": "denied" }),
        S::Granted(g) => json!({ "state": "granted", "kind": g.kind, "as_root": g.as_root, "once": g.once, "expires": g.expires }),
    }
}

/// Ask for (or look at) the approval on the device's screen. Safe to call again and again while waiting.
fn grant_route(cx: &Ctx, action: &str, body: &[u8]) -> Out {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Out::err(400, "bad request") };
    let kind = v["kind"].as_str().unwrap_or("shell");
    let need = match kind {
        "shell" => Scope::Shell,
        "patch" => Scope::Update,
        _ => Scope::Power,
    };
    if let Err(e) = scopes::check(cx.key, cx.policy, need) {
        return cx.refuse(action, &e);
    }
    // a repeat of a request that is already waiting or granted just reports it
    let have = cx.dm.shell_status(&cx.key.name);
    let st = match have {
        super::grants::ShellStatus::Pending | super::grants::ShellStatus::Granted(_) => have,
        super::grants::ShellStatus::Denied if v["retry"].as_bool() != Some(true) => {
            // the denial is reported once; asking again after that makes a new request
            cx.dm.shell_clear_denied(&cx.key.name);
            super::grants::ShellStatus::Denied
        }
        _ => match cx.dm.grant_request(cx.key, kind, v["as_root"].as_bool().unwrap_or(false), v["minutes"].as_u64().unwrap_or(10) as u32, v["why"].as_str().unwrap_or("")) {
            Ok(s) => s,
            Err(e) => return Out::err(429, &e),
        },
    };
    Out::json(200, &grant_json(&st))
}

fn passwd_text(cx: &Ctx) -> String {
    std::fs::read_to_string(&cx.paths.sys.passwd).unwrap_or_default()
}

fn shell_route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    let comp = cx.key.name.as_str();
    let num = |n: &str| q(n).parse::<u64>().ok();
    let Some(sid) = (if path == "/v1/dev/shell/open" { Some(0) } else { num("sid") }) else { return Out::err(422, "sid: the session number") };
    let fail = |e: String| {
        cx.log(action, &format!("refused: {}", e.chars().take(150).collect::<String>()), false);
        Out::err(409, &e)
    };
    match path {
        "/v1/dev/shell/open" => {
            let euid = unsafe_euid();
            let home = cx.home.clone().unwrap_or_else(|| crate::scope::home_root(&cx.key.account, cx.dm.data_dir()));
            match super::shellexec::open(cx.dm, cx.key, cx.policy, &passwd_text(cx), euid, &home) {
                Ok(sid) => {
                    let cwd = super::shellexec::cwd(cx.dm, sid, comp).unwrap_or_default();
                    let g = cx.dm.shell_grant(comp);
                    cx.did(action, &format!("session {sid} opened{}", if g.as_ref().is_some_and(|g| g.as_root) { " AS ROOT" } else { "" }));
                    Out::json(200, &json!({ "sid": sid, "cwd": cwd.display().to_string(), "as_root": g.is_some_and(|g| g.as_root), "expires": cx.dm.shell_grant(comp).map(|g| g.expires) }))
                }
                Err(e) => fail(e),
            }
        }
        "/v1/dev/shell/exec" => {
            let line = String::from_utf8_lossy(body).to_string();
            match super::shellexec::exec(cx.dm, sid, comp, &line) {
                Ok((id, cwd)) => {
                    // the command line is the audit record; a credential in it is hidden
                    let shown = debug::scrub(&line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>());
                    cx.did(action, &format!("session {sid} in {}: {shown}", cwd.display()));
                    Out::json(200, &json!({ "id": id, "cwd": cwd.display().to_string() }))
                }
                Err(e) => fail(e),
            }
        }
        "/v1/dev/shell/read" => match super::shellexec::read(cx.dm, sid, comp, num("id").unwrap_or(0), num("since").unwrap_or(0), num("wait").unwrap_or(0)) {
            Ok((bytes, next, lost, done, code)) => Out::json(200, &json!({ "text": String::from_utf8_lossy(&bytes), "next": next, "lost": lost, "done": done, "code": code, "cwd": super::shellexec::cwd(cx.dm, sid, comp).map(|p| p.display().to_string()).unwrap_or_default() })),
            Err(e) => Out::err(409, &e),
        },
        "/v1/dev/shell/stdin" => match super::shellexec::write_stdin(cx.dm, sid, comp, num("id").unwrap_or(0), body, q("eof") == "1") {
            Ok(()) => Out::json(200, &json!({ "ok": true })),
            Err(e) => Out::err(409, &e),
        },
        "/v1/dev/shell/signal" => match super::shellexec::signal(cx.dm, sid, comp, num("id").unwrap_or(0), &q("sig")) {
            Ok(()) => {
                cx.log(action, &format!("session {sid} command {} <- {}", q("id"), q("sig")), true);
                Out::json(200, &json!({ "ok": true }))
            }
            Err(e) => Out::err(409, &e),
        },
        "/v1/dev/shell/close" => match super::shellexec::close(cx.dm, sid, comp) {
            Ok(()) => Out::json(200, &json!({ "ok": true })),
            Err(e) => Out::err(409, &e),
        },
        _ => Out::err(404, "not found"),
    }
}

fn unsafe_euid() -> u32 {
    // SAFETY: geteuid(2) takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

fn forward_route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    let comp = cx.key.name.as_str();
    let num = |n: &str| q(n).parse::<u64>().ok();
    if path == "/v1/dev/forward/start" {
        let Some(port) = q("port").parse::<u16>().ok() else { return Out::err(422, "port: 1 to 65535") };
        return match super::tunnel::start(cx.dm, cx.key, port, cx.own_port) {
            Ok(fid) => {
                cx.did(action, &format!("forward {port} opened (session {fid})"));
                Out::json(200, &json!({ "fid": fid, "port": port }))
            }
            Err(e) => {
                cx.log(action, &format!("refused: {e}"), false);
                Out::err(422, &e)
            }
        };
    }
    let Some(fid) = num("fid") else { return Out::err(422, "fid: the forward number") };
    let cid = num("cid").unwrap_or(0);
    match path {
        "/v1/dev/forward/connect" => match super::tunnel::connect(cx.dm, fid, comp) {
            Ok(cid) => {
                cx.log(action, &format!("forward {fid} connection {cid}"), true);
                Out::json(200, &json!({ "cid": cid }))
            }
            Err(e) => Out::err(502, &e),
        },
        "/v1/dev/forward/send" => match super::tunnel::send(cx.dm, fid, comp, cid, body) {
            Ok(()) => Out::json(200, &json!({ "ok": true })),
            Err(e) => Out::err(502, &e),
        },
        "/v1/dev/forward/recv" => match super::tunnel::recv(cx.dm, fid, comp, cid, num("wait").unwrap_or(0)) {
            Ok(Some(b)) => Out::bytes(b),
            Ok(None) => Out::err(410, "the connection is closed"),
            Err(e) => Out::err(502, &e),
        },
        "/v1/dev/forward/close" => match super::tunnel::close(cx.dm, fid, comp, cid) {
            Ok(()) => Out::json(200, &json!({ "ok": true })),
            Err(e) => Out::err(502, &e),
        },
        "/v1/dev/forward/stop" => match super::tunnel::stop(cx.dm, fid, comp) {
            Ok(()) => {
                cx.log(action, &format!("forward {fid} closed"), true);
                Out::json(200, &json!({ "ok": true }))
            }
            Err(e) => Out::err(502, &e),
        },
        _ => Out::err(404, "not found"),
    }
}

/// Restart or switch off. Needs an approved grant (any kind) unless... never: the device's screen is asked every time.
fn power_route(cx: &Ctx, action: &str, path: &str) -> Out {
    let Some(g) = cx.dm.shell_grant(&cx.key.name) else {
        return cx.refuse(action, "not approved: ask for the approval first (the administrator confirms on the device's screen)");
    };
    if !cx.policy.manage_settings {
        return cx.refuse(action, "restart and switch-off are for administrator accounts");
    }
    let reboot = path.ends_with("reboot");
    cx.did(action, &format!("{} requested (approved by {})", if reboot { "restart" } else { "switch off" }, g.by));
    cx.dm.power_used(&cx.key.name);
    (cx.power)(reboot);
    Out::json(200, &json!({ "message": if reboot { "the device is restarting" } else { "the device is switching off" } }))
}

fn update_route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    use super::update as up;
    let Some(boot) = cx.boot else { return Out::err(503, "this device has no boot partition to update") };
    let data = cx.dm.data_dir().to_path_buf();
    let free = || cx.data_free.unwrap_or_else(|| debug::statvfs(&data).1);
    let env = up::Env { dm: cx.dm, boot, release: cx.release.to_string(), volatile: cx.volatile, data_free: &free, fault_after: None };
    let stored = || -> Result<up::Manifest, String> { up::read_manifest(&data.join("boot-next")).map_err(|_| "no update is being prepared: send the plan first".to_string()) };
    let refuse = |why: String| {
        cx.log(action, &format!("refused: {}", why.chars().take(200).collect::<String>()), false);
        Out::err(409, &why)
    };
    match path {
        "/v1/dev/update/plan" => {
            let Ok(m) = serde_json::from_slice::<up::Manifest>(body) else { return Out::err(400, "the update description is not valid") };
            match up::plan(&env, &m, q("force") == "1") {
                Ok(p) => {
                    // a new plan starts from nothing staged
                    let st = data.join("boot-next");
                    if st.join(up::MANIFEST).exists() && up::read_manifest(&st).is_ok_and(|old| old != m) {
                        let _ = std::fs::remove_dir_all(&st);
                    }
                    if let Err(e) = std::fs::create_dir_all(&st).and_then(|_| std::fs::write(st.join(up::MANIFEST), body)) {
                        return Out::err(500, &e.to_string());
                    }
                    cx.did(action, &format!("{} -> {}: {} file(s) to send, {} the same", p.from, p.to, p.needs.len(), p.same));
                    Out::json(200, &serde_json::to_value(&p).unwrap_or_default())
                }
                Err(e) => refuse(e),
            }
        }
        "/v1/dev/update/staged" => match stored().and_then(|_| files::stat(&up_sandbox(&env)?, &q("path"), false)) {
            Ok(v) => Out::json(200, &v),
            Err(e) => refuse(e),
        },
        "/v1/dev/update/stage" => match stored() {
            Ok(m) => match up::stage_chunk(&env, &m, &q("path"), q("offset").parse().unwrap_or(0), body) {
                Ok(v) => {
                    if v["done"].as_bool() == Some(true) {
                        cx.log(action, &format!("staged {} ({} bytes)", q("path"), v["have"]), true);
                    }
                    Out::json(200, &v)
                }
                Err((code, v)) => {
                    cx.log(action, &format!("refused {}: {}", q("path"), v["error"].as_str().unwrap_or("")), false);
                    Out::json(code, &v)
                }
            },
            Err(e) => refuse(e),
        },
        "/v1/dev/update/apply" => match stored().and_then(|m| up::apply(&env, &m, q("force") == "1", &cx.key.name).map(|v| (m, v))) {
            Ok((m, v)) => {
                cx.did(action, &format!("{} (to {})", v["message"].as_str().unwrap_or("done"), m.version));
                if q("reboot") == "1" {
                    cx.did("dev.power.reboot", "restart requested by the update");
                    (cx.power)(true);
                }
                Out::json(200, &v)
            }
            Err(e) => {
                cx.log(action, &format!("FAILED: {}", e.chars().take(200).collect::<String>()), false);
                (cx.audit)("devmode.update.failed", &e.chars().take(200).collect::<String>());
                Out::err(409, &e)
            }
        },
        "/v1/dev/update/abort" => {
            if up::interrupted(&data).is_some() {
                return refuse("an update was interrupted: do not clear it until it has been repeated or the previous version is chosen in the boot menu".into());
            }
            let _ = std::fs::remove_dir_all(data.join("boot-next"));
            cx.did(action, "staged files removed");
            Out::json(200, &json!({ "ok": true }))
        }
        "/v1/dev/update/status" => Out::json(200, &up::status(&data)),
        _ => Out::err(404, "not found"),
    }
}

fn up_sandbox(env: &super::update::Env) -> Result<Sandbox, String> {
    let s = env.dm.data_dir().join("boot-next");
    std::fs::create_dir_all(&s).map_err(|e| e.to_string())?;
    let real = std::fs::canonicalize(&s).map_err(|e| e.to_string())?;
    Ok(Sandbox::with_roots(&real, &[real.clone()], &[], &[]))
}

/// Hot patches (`iyakki dev patch`, docs/development/REMOTE-PATCH.md). The `update` scope is checked by [`handle`]; on top of it the
/// computer needs the approval card once (kind `patch`), and the wide paths, free `shell` steps and setuid bits need the `shell` scope
/// and a valid shell approval. Every change is audited.
fn patch_route(cx: &Ctx, action: &str, path: &str, q: &dyn Fn(&str) -> String, body: &[u8]) -> Out {
    use super::patch as pt;
    let data = cx.dm.data_dir().to_path_buf();
    let free = || cx.data_free.unwrap_or_else(|| debug::statvfs(&data).1);
    let grant = cx.dm.shell_grant(&cx.key.name);
    let shell_ok = cx.key.has(Scope::Shell) && scopes::check(cx.key, cx.policy, Scope::Shell).is_ok() && grant.as_ref().is_some_and(|g| g.kind == "shell");
    let env = pt::Env { dm: cx.dm, live: &pt::SysLive, root: PathBuf::from("/"), release: cx.release.to_string(), wide: shell_ok, data_free: &free };
    let refuse = |why: String| {
        cx.log(action, &format!("refused: {}", why.chars().take(200).collect::<String>()), false);
        Out::err(409, &why)
    };
    if path == "/v1/dev/patch/list" {
        return Out::json(200, &pt::list(&env));
    }
    // the card: once per computer, then remembered; an approval of any kind that is valid now also counts
    if !cx.dm.patch_trusted(&cx.key.name) && !grant.as_ref().is_some_and(|g| matches!(g.kind.as_str(), "patch" | "shell")) {
        return cx.refuse(action, "not approved: an administrator confirms patching once on the device's screen (ask for the approval first)");
    }
    let id = q("id");
    let stored = || -> Result<pt::Manifest, String> { serde_json::from_slice(&std::fs::read(pt::staging_dir(&data, &id).join("manifest.json")).map_err(|_| "no patch is being prepared: send the plan first".to_string())?).map_err(|e| e.to_string()) };
    match path {
        "/v1/dev/patch/plan" => {
            let Ok(m) = serde_json::from_slice::<pt::Manifest>(body) else { return Out::err(400, "the patch description is not valid") };
            match pt::plan(&env, &m) {
                Ok(p) => {
                    let st = pt::staging_dir(&data, &m.id);
                    // a different plan under the same id starts from nothing staged
                    if st.join("manifest.json").exists() && std::fs::read(st.join("manifest.json")).ok().and_then(|b| serde_json::from_slice::<pt::Manifest>(&b).ok()).is_some_and(|old| old != m) {
                        let _ = std::fs::remove_dir_all(&st);
                    }
                    if let Err(e) = std::fs::create_dir_all(&st).and_then(|_| std::fs::write(st.join("manifest.json"), body)) {
                        return Out::err(500, &e.to_string());
                    }
                    cx.did(action, &format!("{}: {} file(s), {} bytes{}", p.id, p.files, p.bytes, if env.wide { " (wide)" } else { "" }));
                    Out::json(200, &serde_json::to_value(&p).unwrap_or_default())
                }
                Err(e) => refuse(e),
            }
        }
        "/v1/dev/patch/stage" => match stored() {
            Ok(m) => {
                let idx: usize = q("file").parse().unwrap_or(usize::MAX);
                let Some(o) = m.ops.get(idx).filter(|o| o.action != pt::Action::Delete && o.kind == pt::Kind::File) else { return Out::err(422, "file: the number of a file of the patch that has content") };
                let st = pt::staging_dir(&data, &m.id);
                let real = match std::fs::canonicalize(&st) {
                    Ok(r) => r,
                    Err(e) => return Out::err(500, &e.to_string()),
                };
                let sb = Sandbox::with_roots(&real, &[real.clone()], &[], &[]);
                match files::put(&sb, &pt::staged_name(idx), num(&q("offset")).unwrap_or(0), o.size, &o.sha256, true, None, body) {
                    Ok(v) => Out::json(200, &v),
                    Err((code, v)) => Out::json(code, &v),
                }
            }
            Err(e) => refuse(e),
        },
        "/v1/dev/patch/apply" => {
            let mode = if q("mode") == "next-boot" { pt::Mode::NextBoot } else { pt::Mode::Live };
            match stored().and_then(|m| pt::apply(&env, &m, mode, &cx.key.name).map(|v| (m, v))) {
                Ok((m, v)) => {
                    cx.did(action, &format!("{} {} ({} file(s){})", m.id, if mode == pt::Mode::Live { "live" } else { "next start" }, m.ops.len(), if env.wide { ", wide" } else { "" }));
                    Out::json(200, &v)
                }
                Err(e) => {
                    cx.log(action, &format!("FAILED: {}", e.chars().take(200).collect::<String>()), false);
                    (cx.audit)("devmode.patch.failed", &e.chars().take(200).collect::<String>());
                    Out::err(409, &e)
                }
            }
        }
        "/v1/dev/patch/remove" => match pt::remove(&env, &id, &cx.key.name) {
            Ok(v) => {
                cx.did(action, &format!("{id} removed"));
                Out::json(200, &v)
            }
            Err(e) => refuse(e),
        },
        _ => Out::err(404, "not found"),
    }
}

//! Chat tools for Settings -> Apps access (`apps_access_list` free, `apps_access_revoke` asks, administrators only) and for the switch of
//! Settings -> Connect a client (EXPERIMENTAL): `connect_client_status` (free) and `connect_client_enable` (asks). They use
//! `accessbroker` and `clientlink`, the same code as the pages. The devices, their permissions and revoking them are `client_devices`,
//! `client_device_permission` and `client_device_revoke`; a one-time link is shown by the page only (it is a bearer token).

use serde_json::json;

use crate::accessbroker as ab;
use crate::settingstools::{arg_bool, arg_str, no_args, Entry, Env};
pub use iyakki_shared::settingstools_access::{PROMPT_RULE};

pub const TOOLS: [&str; 7] = ["apps_access_list", "apps_access_revoke", "connect_client_status", "connect_client_enable", "app_sdk_status", "app_money_limit_status", "app_money_limit_set"];


const LOG_LINES: usize = 10;

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(
            Entry::read("AppAccess", "apps_access_list", "List what installed apps may use (device and system access), who approved it, whether it was taken away, and the latest lines of the access log.", no_args(), move |_| {
                let held = ab::held_list(&e.dir);
                let mut out = String::new();
                for h in &held {
                    for c in &h.holds {
                        out.push_str(&format!("{} {}: {c} (approved by {}) [installation {}]\n", h.app, h.version, h.who, h.installation));
                    }
                    for c in &h.revoked {
                        out.push_str(&format!("{} {}: {c} TAKEN AWAY [installation {}]\n", h.app, h.version, h.installation));
                    }
                }
                if out.is_empty() {
                    out.push_str("No installed app has device access yet (default apps are not listed).\n");
                }
                let zone = crate::clock::Zone::system_with(None);
                for l in ab::FileLog::open(&e.dir).recent(LOG_LINES) {
                    let when = crate::clock::format(&zone, l.at, "%d %b %H:%M");
                    out.push_str(&format!("log {when} {} {} {} {} {}\n", l.person, l.app, l.capability, l.target, if l.allowed { "allowed".to_string() } else { format!("refused ({})", l.reason) }));
                }
                Ok(out.trim_end().to_string())
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"installation": {"type": "string", "description": "The installation id from apps_access_list."}, "capability": {"type": "string"}}, "required": ["installation", "capability"]});
        v.push(
            Entry::change("AppAccess", "apps_access_revoke", "Take one capability away from one installed app. It applies at the app's next try. The person approves a card first.", schema, |a| format!("Take {} away from {}", a["capability"].as_str().unwrap_or("access"), a["installation"].as_str().unwrap_or("an app")), move |a| {
                let (inst, cap) = (arg_str(a, "installation")?, arg_str(a, "capability")?);
                let app = ab::held_list(&e.dir).into_iter().find(|h| h.installation == inst).map(|h| format!("{} {} (approved by {})", h.app, h.version, h.who)).unwrap_or_else(|| inst.clone());
                match crate::accessbroker_revoke::revoke(&e.dir, &inst, &cap)? {
                    true => {
                        e.audit("access.revoke", &format!("{app} {cap}"));
                        Ok(format!("{app} can no longer use {cap}. It applies at its next try."))
                    }
                    false => Ok(format!("{app} did not hold {cap} (already taken away).")),
                }
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(
            Entry::read("Remote", "connect_client_status", "Show whether browsers and other devices may connect to this desktop (EXPERIMENTAL), and how many devices and open one-time links there are.", no_args(), move |_| {
                let now = crate::clock::now_unix();
                Ok(format!(
                    "Connect a client (experimental) is {}. {} devices enrolled, {} one-time links open.",
                    if crate::clientlink::enabled(&e.dir) { "on" } else { "off" },
                    crate::clientlink::list_devices(&e.dir).len(),
                    crate::clientlink::open_tokens(&e.dir, now)
                ))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(
            Entry::change("Remote", "connect_client_enable", "Switch Connect a client (EXPERIMENTAL) on or off. Off stops new connections. The person approves a card first.", schema, |a| format!("Turn Connect a client {}", if a["on"].as_bool() == Some(false) { "off" } else { "on" }), move |a| {
                let on = arg_bool(a, "on")?;
                crate::clientlink::set_enabled(&e.dir, on)?;
                e.audit("settings.connect", if on { "on" } else { "off" });
                Ok(format!("Connect a client is now {}.", if on { "on" } else { "off" }))
            })
            .admin(),
        );
    }
    {
        v.push(Entry::read("AppAccess", "app_sdk_status", "List the features installed apps can ask for (the SDK version, each feature with the version it appeared in and the approval it needs: none, once, every time, or a money limit).", no_args(), |_| {
            use iyakki_core_api::sdkbridge as b;
            let mut out = format!("App features (SDK {}). A feature only works when an app declared it and the person allowed it where it asks.\n", b::SDK_VERSION);
            for n in b::NAMESPACES {
                let ops: Vec<String> = n.ops.iter().map(|o| format!("{} [{}]", o.name, o.approval.name())).collect();
                out.push_str(&format!("{} (since {}): {}\n", n.name, n.since, ops.join(", ")));
            }
            Ok(out.trim_end().to_string())
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"app": {"type": "string", "description": "The app's id, like meeting-notes."}}, "required": ["app"]});
        v.push(Entry::read("AppAccess", "app_money_limit_status", "Show how much money one of your apps may spend a day (it starts at nothing), what it spent today, and whether each payment asks you.", schema, move |a| {
            let app = arg_str(a, "app")?;
            if !crate::skills::valid_skill_name(&app) {
                return Err("an app id is lowercase letters, digits and dashes".into());
            }
            let s = crate::store::sdkv2_money::status(&e.dir, &e.user, &app, &crate::clock::Zone::system(), crate::clock::now_unix());
            Ok(format!("{app} {}", s.words()))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "app": {"type": "string", "description": "The app's id, like meeting-notes."},
            "currency": {"type": "string", "description": "Three capital letters, like USD."},
            "per_day_minor": {"type": "integer", "description": "The most it may spend a day, in the smallest unit (cents). 0 takes the permission away."},
            "confirm_each": {"type": "boolean", "description": "true (the default) keeps a card on every payment; false lets payments inside the limit go without one."}
        }, "required": ["app", "currency", "per_day_minor"]});
        v.push(Entry::change("AppAccess", "app_money_limit_set", "Set how much money one of your apps may spend a day, in the smallest unit of a currency. The person approves a card first, every time.", schema, |a| {
            let cur = a["currency"].as_str().unwrap_or("?");
            let per = a["per_day_minor"].as_u64().unwrap_or(0);
            format!("Let {} spend up to {} a day{}", a["app"].as_str().unwrap_or("an app"), iyakki_core_api::sdkbridge::money::format_minor(per, cur), if a["confirm_each"].as_bool() == Some(false) { " WITHOUT asking you for each payment inside the limit" } else { " (every payment still asks you)" })
        }, move |a| {
            let app = arg_str(a, "app")?;
            if !crate::skills::valid_skill_name(&app) {
                return Err("an app id is lowercase letters, digits and dashes".into());
            }
            let cur = arg_str(a, "currency")?;
            let per = a.get("per_day_minor").and_then(serde_json::Value::as_u64).ok_or("say `per_day_minor`: a whole number of the smallest unit")?;
            let confirm = a.get("confirm_each").and_then(serde_json::Value::as_bool);
            crate::store::sdkv2_money::set_limit(&e.dir, &e.user, &app, &cur, per, confirm)?;
            e.audit("app.money.limit", &format!("{app} limit set to {}", iyakki_core_api::sdkbridge::money::format_minor(per, &cur)));
            Ok(format!("{app} may now spend up to {} a day.", iyakki_core_api::sdkbridge::money::format_minor(per, &cur)))
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn connect_switch_and_the_access_list_are_for_administrators_and_ask_for_changes() {
        let r = rig("access", true);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["apps_access_list", "connect_client_status"], &["apps_access_revoke", "connect_client_enable"]);
        assert!(t.call("apps_access_list", &json!({})).unwrap().contains("No installed app"));
        assert!(t.call("apps_access_revoke", &json!({"installation": "nope", "capability": "camera.view"})).is_err());
        assert!(t.call("connect_client_status", &json!({})).unwrap().contains("off"));
        t.call("connect_client_enable", &json!({"on": true})).unwrap();
        assert!(crate::clientlink::enabled(&r.dir));
        assert!(t.call("connect_client_status", &json!({})).unwrap().contains("is on"));
        assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.connect on"));
        let _ = std::fs::remove_dir_all(&r.dir);
        // a person who is not an administrator does not even see them
        let r2 = rig("access2", false);
        let t2 = tools(&r2, entries(&r2.env));
        // only the person's own app tools (the SDK and money limits) are theirs; the access list and the connect switch are for administrators
        assert!(t2.defs().iter().all(|d| d.spec.name.starts_with("app_")) && t2.defs().len() == 3);
        assert!(t2.call("connect_client_enable", &json!({"on": true})).is_err());
        assert!(t2.call("apps_access_list", &json!({})).is_err());
        let _ = std::fs::remove_dir_all(&r2.dir);
    }

    #[test]
    fn the_money_limit_tools_start_at_nothing_ask_to_change_it_and_never_move_a_limit_without_a_person() {
        let r = rig("access-money", false);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["app_sdk_status", "app_money_limit_status"], &["app_money_limit_set"]);
        assert!(t.call("app_sdk_status", &json!({})).unwrap().contains("money (since 1.2): limits [none], spend [limit], requestLimit [each-call]"));
        assert!(t.call("app_money_limit_status", &json!({"app": "demo"})).unwrap().contains("may not spend money"));
        assert!(t.call("app_money_limit_status", &json!({"app": "Bad App"})).is_err());
        assert!(t.call("app_money_limit_set", &json!({"app": "demo", "currency": "usd", "per_day_minor": 100})).is_err());
        assert!(t.call("app_money_limit_set", &json!({"app": "demo", "currency": "USD"})).is_err());
        t.call("app_money_limit_set", &json!({"app": "demo", "currency": "USD", "per_day_minor": 1000})).unwrap();
        let s = t.call("app_money_limit_status", &json!({"app": "demo"})).unwrap();
        assert!(s.contains("10.00 USD a day") && s.contains("every payment asks you first"), "{s}");
        t.call("app_money_limit_set", &json!({"app": "demo", "currency": "USD", "per_day_minor": 1000, "confirm_each": false})).unwrap();
        assert!(t.call("app_money_limit_status", &json!({"app": "demo"})).unwrap().contains("do not ask"));
        assert!(r.audits.lock().unwrap().iter().any(|a| a.starts_with("app.money.limit demo limit set to 10.00 USD")));
        assert!(crate::agent::hard_ask("app_money_limit_set", &json!({})).is_some(), "even Auto mode asks");
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}

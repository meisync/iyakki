//! Chat tools for the events apps hear (F-SDK-188, Settings -> App events): `app_events_list` (free), `app_events_register`,
//! `app_events_unregister`, `app_events_switch` and `app_events_replay_missed` (each asks the person on a card). All of them go through the
//! core (`cx.core.app_events()`): the same call the page makes, with the same scope and file-policy checks, and the audit lines name events
//! and numbers, never a payload value. A tool never reads or shows a payload.

use super::chat_settings::{arg_bool, arg_str, no_args, Entry, Env};
use iyakki_shared::agent::Tools;
use serde_json::json;
use std::sync::Arc;

use super::chat_port::ChatPort;

pub const TOOLS: [&str; 10] = ["app_events_list", "app_events_catalogue", "app_events_register", "app_events_unregister", "app_events_switch", "app_events_replay_missed", "app_actions_pending", "app_actions_decide", "app_permissions_status", "app_permissions_set"];

pub const PROMPT_RULE: &str = "\nAPP EVENTS (Settings -> App events): app_events_list says which events each of this person's apps listens to, what each lets the app hear, how many wait for it and whether the person switched it off (free). app_events_catalogue lists the events that exist and the permission each needs (free). app_events_register, app_events_unregister, app_events_switch (true or false) and app_events_replay_missed (hands the app the events it was already given, again, marked late) each ask the person first. An app hears an event only if it registered for it, holds the permission and the person has not switched it off; an app that does not run gets what happened at its next start, marked late, and one `missed` count if the queue dropped events. Events never carry secrets or the values of settings; the tools never show a payload. APP ACTIONS: an app that heard an event may ask for a chat tool to run; nothing runs until the person answers. app_actions_pending lists the requests waiting (app, tool; free). app_actions_decide (asks; the card names the app, the tool and its arguments) runs the tool when approve is true and drops the request when false. Never approve for the person: they answer the card. app_permissions_status (free) says what each app holds now and the last change of its permissions that was told to it (who changed it, what was granted and taken away); names only. app_permissions_set (asks) takes one permission away from an app, or gives back one the person took away (allow true or false): the app is told at once and the events registrations that needed it end; it cannot give an app more than it was approved for.";

fn app_arg(a: &serde_json::Value) -> Result<String, String> {
    arg_str(a, "app")
}

pub fn entries(cx: &dyn ChatPort, _env: &Env, inner: Arc<dyn Tools>) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        v.push(Entry::read("App events", "app_events_list", "List which events each app listens to: the app, the event, what it lets the app hear, how many wait, and whether it is switched off. Read only; no payloads.", no_args(), move |_| {
            let apps = core.app_events().apps(&user);
            if apps.is_empty() {
                return Ok("No app listens to events yet.".into());
            }
            let mut out = String::new();
            for (app, regs) in apps {
                for r in regs {
                    let what = if r.filter.is_empty() { r.event } else { format!("{} ({})", r.event, r.filter) };
                    out.push_str(&format!("{app}: {what} - {} - {}\n", r.gives, if r.enabled { format!("{} waiting", r.pending) } else { "switched off".into() }));
                }
            }
            Ok(out)
        }));
    }
    {
        let core = cx.core();
        v.push(Entry::read("App events", "app_events_catalogue", "List the events an app can register for, with the permission each needs and its fields (never values). Read only.", no_args(), move |_| {
            let c = core.app_events().catalogue();
            if c.is_empty() {
                return Ok("This system has no app events.".into());
            }
            Ok(c.iter().map(|e| format!("{} v{} [{}]: {}{}", e.name, e.version, e.scope, e.gives, if e.timer { " (timer: runs only while an app listens)" } else { "" })).collect::<Vec<_>>().join("\n"))
        }));
    }
    let schema_reg = json!({"type": "object", "properties": {"app": {"type": "string"}, "event": {"type": "string"}, "filter": {"type": "string", "description": "a settings key prefix or a folder in the home, for the events that take one"}}, "required": ["app", "event"]});
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        v.push(Entry::change("App events", "app_events_register", "Let an app hear an event. It must hold the event's permission. The person approves a card first.", schema_reg.clone(), |a| format!("Let {} hear {}", a["app"].as_str().unwrap_or("an app"), a["event"].as_str().unwrap_or("an event")), move |a| {
            let (app, ev) = (app_arg(a)?, arg_str(a, "event")?);
            let filter = a.get("filter").and_then(|f| f.as_str()).unwrap_or("");
            core.app_events().register(&user, &app, &ev, filter)?;
            Ok(format!("{app} now hears {ev}."))
        }));
    }
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        v.push(Entry::change("App events", "app_events_unregister", "Stop an app hearing an event; what waited for it is dropped. The person approves a card first.", schema_reg, |a| format!("Stop {} hearing {}", a["app"].as_str().unwrap_or("an app"), a["event"].as_str().unwrap_or("an event")), move |a| {
            let (app, ev) = (app_arg(a)?, arg_str(a, "event")?);
            core.app_events().unregister(&user, &app, &ev, a.get("filter").and_then(|f| f.as_str()).unwrap_or(""))?;
            Ok(format!("{app} no longer hears {ev}."))
        }));
    }
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        let schema = json!({"type": "object", "properties": {"app": {"type": "string"}, "event": {"type": "string"}, "filter": {"type": "string"}, "on": {"type": "boolean"}}, "required": ["app", "event", "on"]});
        v.push(Entry::change("App events", "app_events_switch", "Switch one event of one app off or on without removing the app's registration (the switch on Settings -> App events). The person approves a card first.", schema, |a| format!("Turn {} for {} {}", a["event"].as_str().unwrap_or("an event"), a["app"].as_str().unwrap_or("an app"), if a["on"].as_bool().unwrap_or(true) { "on" } else { "off" }), move |a| {
            let (app, ev, on) = (app_arg(a)?, arg_str(a, "event")?, arg_bool(a, "on")?);
            core.app_events().set_enabled(&user, &app, &ev, a.get("filter").and_then(|f| f.as_str()).unwrap_or(""), on)?;
            Ok(format!("{ev} for {app} is {}.", if on { "on" } else { "off, and nothing waits for it" }))
        }));
    }
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        let schema = json!({"type": "object", "properties": {"app": {"type": "string"}, "after": {"type": "integer", "description": "hand over the events after this sequence number again (0 = all that are kept)"}}, "required": ["app"]});
        v.push(Entry::change("App events", "app_events_replay_missed", "Give an app the events it was already delivered again (up to the last 50), marked late, for an app that crashed before it handled them. Dropped events cannot come back. The person approves a card first.", schema, |a| format!("Give {} its recent events again", a["app"].as_str().unwrap_or("an app")), move |a| {
            let app = app_arg(a)?;
            let after = a.get("after").and_then(|n| n.as_u64()).unwrap_or(0);
            let n = core.app_events().requeue(&user, &app, after)?;
            Ok(format!("{n} events wait for {app} again; it gets them with its next poll, marked late."))
        }));
    }
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        let schema = json!({"type": "object", "properties": {"app": {"type": "string", "description": "one app; leave out for every app that has registrations or a permission change"}}});
        v.push(Entry::read("App events", "app_permissions_status", "List what an app may do now (the names of its capabilities and scopes) and the last change of that told to it: when, who changed it, what was granted and taken away, which registrations ended, whether it must restart. Names only, no payload values. Read only.", schema, move |a| {
            let ev = core.app_events();
            let apps: Vec<String> = match a.get("app").and_then(|x| x.as_str()) {
                Some(one) => vec![one.to_string()],
                None => ev.known_apps(&user),
            };
            if apps.is_empty() {
                return Ok("No app has registrations or a permission change yet.".into());
            }
            Ok(apps.iter().map(|app| {
                let (held, last) = ev.permissions(&user, app);
                format!("{app}: holds [{}]; last change: {}", held.join(", "), last.unwrap_or_else(|| "none told yet".into()))
            }).collect::<Vec<_>>().join("\n"))
        }));
    }
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        let schema = json!({"type": "object", "properties": {"app": {"type": "string"}, "permission": {"type": "string", "description": "the permission's name, for example events.network or files.read"}, "allow": {"type": "boolean", "description": "false takes it away, true gives back one that was taken away"}}, "required": ["app", "permission", "allow"]});
        v.push(Entry::change("App events", "app_permissions_set", "Take one permission away from an app, or give back one the person took away. The app is told at once. It cannot give an app more than it was approved for. The person approves a card first.", schema, |a| format!("{} {} {}", if a["allow"].as_bool().unwrap_or(false) { "Give back" } else { "Take away" }, a["permission"].as_str().unwrap_or("a permission"), if a["allow"].as_bool().unwrap_or(false) { format!("to {}", a["app"].as_str().unwrap_or("an app")) } else { format!("from {}", a["app"].as_str().unwrap_or("an app")) }), move |a| {
            let (app, scope, allow) = (app_arg(a)?, arg_str(a, "permission")?, arg_bool(a, "allow")?);
            core.app_events().set_permission(&user, &app, &scope, allow, "person")?;
            Ok(format!("{app} {} {scope}.", if allow { "may use" } else { "may no longer use" }))
        }));
    }
    {
        let (core, user) = (cx.core(), cx.user().to_string());
        v.push(Entry::read("App events", "app_actions_pending", "List the chat tool requests apps made after hearing an event and that wait for the person: the id, the app and the tool. Read only; arguments are shown on the card of app_actions_decide.", no_args(), move |_| {
            let p = core.app_actions().pending(&user);
            if p.is_empty() {
                return Ok("No app request is waiting.".into());
            }
            Ok(p.iter().map(|x| format!("{}: {}", x.id, x.summary())).collect::<Vec<_>>().join("\n"))
        }));
    }
    {
        let (core, user, tools) = (cx.core(), cx.user().to_string(), inner);
        let (core2, user2) = (cx.core(), cx.user().to_string());
        let schema = json!({"type": "object", "properties": {"id": {"type": "string"}, "approve": {"type": "boolean"}}, "required": ["id", "approve"]});
        v.push(Entry::change("App events", "app_actions_decide", "Answer a chat tool request an app made after hearing an event: approve runs the tool with the arguments the app gave, decline drops the request. The person approves a card that names the app, the tool and its arguments.", schema, move |a| {
            let id = a["id"].as_str().unwrap_or("");
            match core2.app_actions().pending(&user2).into_iter().find(|p| p.id == id) {
                Some(p) => format!("{} {} for {}{}", if a["approve"].as_bool().unwrap_or(false) { "Run" } else { "Drop the request for" }, p.tool, p.app, if p.args.is_empty() { String::new() } else { format!(" with {}", p.args.chars().take(200).collect::<String>()) }),
                None => "Answer a request that is no longer waiting".into(),
            }
        }, move |a| {
            let (id, approve) = (arg_str(a, "id")?, arg_bool(a, "approve")?);
            let p = core.app_actions().take(&user, &id).ok_or("no such request is waiting")?;
            if !approve {
                return Ok(format!("{}'s request to run {} was dropped.", p.app, p.tool));
            }
            if !tools.defs().iter().any(|d| d.spec.name == p.tool) {
                return Err(format!("there is no tool called {}", p.tool));
            }
            let args: serde_json::Value = if p.args.is_empty() { json!({}) } else { serde_json::from_str(&p.args).map_err(|e| e.to_string())? };
            let out = tools.call(&p.tool, &args)?;
            Ok(format!("{} ran for {}: {}", p.tool, p.app, out.chars().take(600).collect::<String>()))
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::agent::{Risk, ToolDef};
    use iyakki_core_api::appevents::{payload, AppEvents, Val};
    use iyakki_shared::agent::Tools;
    use iyakki_shared::settingstools::SettingsTools;
    use iyakki_core_fake::FakeCore;
    use std::sync::Arc;

    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
            name.to_string()
        }
    }

    #[test]
    fn the_list_is_free_the_changes_ask_and_everything_goes_through_the_core() {
        let mut cx = crate::apps::tests::cx_with_admin_store("appev-chat");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.settings(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&crate::apps::chat_port::CxChat(&cx), &env, Arc::new(NoTools)));
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
        for n in ["app_events_list", "app_events_catalogue"] {
            assert_eq!(risk(n), Risk::Safe, "{n}");
        }
        for n in ["app_events_register", "app_events_unregister", "app_events_switch", "app_events_replay_missed"] {
            assert_eq!(risk(n), Risk::Ask, "{n}");
        }
        assert_eq!(defs.iter().filter(|d| d.spec.name.starts_with("app_events_")).count(), TOOLS.len() - 4);
        let f = core.app_events_fake();
        assert!(t.call("app_events_list", &json!({})).unwrap().contains("No app"));
        assert!(t.call("app_events_register", &json!({"app": "notes", "event": "power.source"})).unwrap_err().contains("events.power"), "the scope is the core's rule");
        f.grant("notes", "events.power");
        t.call("app_events_register", &json!({"app": "notes", "event": "power.source"})).unwrap();
        let l = t.call("app_events_list", &json!({})).unwrap();
        assert!(l.contains("notes: power.source") && l.contains("0 waiting"), "{l}");
        f.emit("power.source", Some(&user), payload(&[("source", Val::Text("battery".into()))])).unwrap();
        assert!(t.call("app_events_list", &json!({})).unwrap().contains("1 waiting"));
        t.call("app_events_switch", &json!({"app": "notes", "event": "power.source", "on": false})).unwrap();
        assert!(t.call("app_events_list", &json!({})).unwrap().contains("switched off"));
        assert!(t.call("app_events_switch", &json!({"app": "notes", "event": "power.source", "on": "no"})).is_err());
        t.call("app_events_switch", &json!({"app": "notes", "event": "power.source", "on": true})).unwrap();
        f.start(&user, "notes");
        f.emit("power.source", Some(&user), payload(&[("source", Val::Text("ac".into()))])).unwrap();
        f.poll(&user, "notes", 9);
        let r = t.call("app_events_replay_missed", &json!({"app": "notes"})).unwrap();
        assert!(r.contains("1 events wait"), "{r}");
        assert_eq!(f.poll(&user, "notes", 9)[0].late, true);
        assert!(t.call("app_events_catalogue", &json!({})).unwrap().contains("power.source v1 [events.power]"));
        t.call("app_events_unregister", &json!({"app": "notes", "event": "power.source"})).unwrap();
        assert!(t.call("app_events_unregister", &json!({"app": "notes", "event": "power.source"})).is_err());
    }

    struct One;
    impl Tools for One {
        fn defs(&self) -> Vec<ToolDef> {
            vec![ToolDef { spec: iyakki_agent::chat::ToolSpec { name: "wifi_status".into(), description: "x".into(), schema: json!({"type": "object"}) }, risk: Risk::Safe, untrusted: false }]
        }
        fn call(&self, name: &str, a: &serde_json::Value) -> Result<String, String> {
            if name == "wifi_status" { Ok(format!("connected {a}")) } else { Err(format!("no tool {name}")) }
        }
        fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
            name.to_string()
        }
    }

    #[test]
    fn a_tool_an_app_asked_for_runs_only_when_the_person_approves_the_card_and_never_by_the_app() {
        use iyakki_core_api::AppActions;
        let mut cx = crate::apps::tests::cx_with_admin_store("appact-chat");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.settings(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(One), true, entries(&crate::apps::chat_port::CxChat(&cx), &env, Arc::new(One)));
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk;
        assert_eq!((risk("app_actions_pending"), risk("app_actions_decide")), (Risk::Safe, Risk::Ask));
        assert_eq!((risk("app_permissions_status"), risk("app_permissions_set")), (Risk::Safe, Risk::Ask));
        let f = core.app_actions_fake();
        assert!(t.call("app_actions_pending", &json!({})).unwrap().contains("No app request"));
        f.grant("notes", "tools.request");
        let id = f.request_tool(&user, "notes", "wifi_status", r#"{"x":1}"#).unwrap();
        let l = t.call("app_actions_pending", &json!({})).unwrap();
        assert!(l.contains("notes asks to run wifi_status") && !l.contains("\"x\""), "arguments are on the card, not in the list: {l}");
        let card = t.describe("app_actions_decide", &json!({"id": id, "approve": true}));
        assert!(card.contains("Run wifi_status for notes") && card.contains("{\"x\":1}"), "{card}");
        let r = t.call("app_actions_decide", &json!({"id": id, "approve": true})).unwrap();
        assert!(r.contains("connected") && r.contains("for notes"), "{r}");
        assert!(t.call("app_actions_decide", &json!({"id": id, "approve": true})).is_err(), "once");
        // a decline drops it and runs nothing; a request for a tool that does not exist fails and is gone
        let id = f.request_tool(&user, "notes", "wifi_status", "").unwrap();
        assert!(t.call("app_actions_decide", &json!({"id": id, "approve": false})).unwrap().contains("dropped"));
        let id = f.request_tool(&user, "notes", "rm_everything", "").unwrap();
        assert!(t.call("app_actions_decide", &json!({"id": id, "approve": true})).unwrap_err().contains("no tool"));
        assert!(f.pending(&user).is_empty());
    }

    #[test]
    fn the_permission_status_lists_what_an_app_holds_and_the_last_change_without_values() {
        let mut cx = crate::apps::tests::cx_with_admin_store("appperm-chat");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.settings(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(One), true, entries(&crate::apps::chat_port::CxChat(&cx), &env, Arc::new(One)));
        assert!(t.call("app_permissions_status", &json!({})).unwrap().contains("No app has"));
        let f = core.app_events_fake();
        f.grant("notes", "events.network");
        f.permissions_changed(&user, "notes", &["events.network".to_string()], &[], "administrator", true).unwrap();
        let r = t.call("app_permissions_status", &json!({"app": "notes"})).unwrap();
        assert!(r.contains("notes: holds [events.network]") && r.contains("by administrator: granted [events.network]") && r.contains("restart needed"), "{r}");
        assert!(t.call("app_permissions_status", &json!({})).unwrap().contains("notes:"));
        assert!(t.call("app_permissions_status", &json!({"app": "ghost"})).unwrap().contains("none told yet"));
    }

    #[test]
    fn the_person_takes_a_permission_away_by_chat_and_the_app_hears_it_and_gets_it_back() {
        let mut cx = crate::apps::tests::cx_with_admin_store("appperm-set");
        let core = Arc::new(FakeCore::new());
        cx.core = core.clone();
        let user = cx.session.user.clone();
        let env = Env { dir: cx.data_dir().to_path_buf(), user: user.clone(), admin: true, settings: cx.services.settings(), audit: Arc::new(|_, _| {}) };
        let t = SettingsTools::new(Arc::new(One), true, entries(&crate::apps::chat_port::CxChat(&cx), &env, Arc::new(One)));
        let f = core.app_events_fake();
        f.grant("sample", "events.network");
        f.register(&user, "sample", "connectivity.changed", "").unwrap();
        f.start(&user, "sample");
        assert!(t.describe("app_permissions_set", &json!({"app": "sample", "permission": "events.network", "allow": false})).contains("Take away events.network from sample"));
        t.call("app_permissions_set", &json!({"app": "sample", "permission": "events.network", "allow": false})).unwrap();
        let got = f.poll(&user, "sample", 5);
        assert!(got[0].payload["revoked"] == iyakki_core_api::appevents::Val::Text("events.network".into()));
        assert!(f.list(&user, "sample").is_empty());
        assert!(t.call("app_permissions_status", &json!({"app": "sample"})).unwrap().contains("holds []"));
        t.call("app_permissions_set", &json!({"app": "sample", "permission": "events.network", "allow": true})).unwrap();
        assert!(t.call("app_permissions_status", &json!({"app": "sample"})).unwrap().contains("events.network"));
        assert!(t.call("app_permissions_set", &json!({"app": "sample", "permission": "bogus", "allow": false})).is_err());
    }
}

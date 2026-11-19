//! Chat tools for Settings -> System -> Host name (F-NAME): `host_name_status` (free) says the name now, the name kept for the next starts and whether a
//! boot option fixes it; `host_name_set` (asks every time, administrators) changes it, the same way the page's Apply now does: the name is checked, kept in
//! `system.conf` and applied to the running system. A name fixed by `iyakki.hostname=` on the kernel command line is not changed.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_shared::settingstools::{Entry};
use iyakki_shared::settingstools::{no_args};
use super::chat_settings::Env;
use serde_json::{json, Value};

pub const TOOLS: [&str; 2] = ["host_name_status", "host_name_set"];

pub const PROMPT_RULE: &str = "\nCOMPUTER NAME (Settings -> System -> Host name): host_name_status (free) says the name the computer has now, the name kept for the next starts and whether a boot option fixes it; host_name_set (administrators; asks every time) changes it: lower-case letters a-z, digits and '-', up to 63 characters, not starting or ending with '-'. It is kept for the next starts and used at once. A name fixed by the boot option iyakki.hostname cannot be changed here.";

/// Where the name lives and how it is applied: the real machine, or a fake in a test.
#[derive(Clone)]
pub struct Panel {
    pub conf: PathBuf,
    pub etc: PathBuf,
    pub cmdline: Arc<dyn Fn() -> String + Send + Sync>,
    pub current: Arc<dyn Fn() -> String + Send + Sync>,
    pub apply: Arc<dyn Fn(&str, &std::path::Path) -> Result<(), String> + Send + Sync>,
}

impl Panel {
    pub fn real() -> Panel {
        Panel {
            conf: crate::hostname::CONF_PATH.into(),
            etc: "/etc/hostname".into(),
            cmdline: Arc::new(|| crate::host().cmdline()),
            current: Arc::new(|| crate::hostname::current("iyakki")),
            apply: Arc::new(|n, p| crate::hostname::apply(n, p)),
        }
    }
}

fn describe(p: &Panel) -> String {
    let now = (p.current)();
    let mut s = format!("The computer is called {now}.");
    match crate::hostname::saved(&p.conf) {
        Some(n) if n != now => s += &format!(" Kept for the next starts: {n}."),
        Some(_) => s += " It is kept for the next starts.",
        None => s += " No name is kept for the next starts (the installer's name or the default is used).",
    }
    if let Some(n) = crate::hostname::locked_by(&(p.cmdline)()) {
        s += &format!(" The boot option iyakki.hostname={n} fixes the name: it cannot be changed here.");
    }
    s
}

fn wanted(a: &Value) -> Result<String, String> {
    crate::hostname::check(a.get("name").and_then(Value::as_str).ok_or("say `name`, the new host name")?)
}

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, Panel::real())
}

pub fn entries_with(env: &Env, panel: Panel) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let p = panel.clone();
        v.push(Entry::read("HostName", "host_name_status", "Show the host name of this computer now, the name kept for the next starts, and whether a boot option fixes it.", no_args(), move |_| Ok(describe(&p))));
    }
    {
        let (p, p2, p3, e) = (panel.clone(), panel.clone(), panel, env.clone());
        let schema = json!({"type": "object", "required": ["name"], "properties": {"name": {"type": "string", "description": "The new host name: lower-case letters a-z, digits and '-', up to 63 characters."}}});
        v.push(
            Entry::change("HostName", "host_name_set", "Change the host name of this computer (kept for the next starts and used at once). The person approves a card first.", schema, |a| format!("Rename this computer to {}", a.get("name").and_then(Value::as_str).unwrap_or("?")), move |a| {
                let name = wanted(a)?;
                if let Some(n) = crate::hostname::locked_by(&(p.cmdline)()) {
                    return Err(format!("the name is fixed by the boot option iyakki.hostname={n}; edit the boot entry to change it"));
                }
                crate::hostname::save(&p.conf, &name).map_err(|e| format!("could not keep the name: {e}"))?;
                e.audit("hostname.apply", &name);
                (p.apply)(&name, &p.etc).map_err(|e| format!("kept for the next starts, but not applied now: {e}"))?;
                Ok(format!("The computer is now called {name}, and the name is kept for the next starts."))
            })
            .admin()
            .with_preview(move |a| format!("Now: {}\nAfter: {}", (p2.current)(), wanted(a).unwrap_or_default()))
            .with_refusal(move |a| wanted(a).err().or_else(|| crate::hostname::locked_by(&(p3.cmdline)()).map(|n| format!("the name is fixed by the boot option iyakki.hostname={n}")))),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::agent::Tools;
    use std::sync::Mutex;

    fn panel(tag: &str, cmdline: &'static str) -> (Panel, Arc<Mutex<Vec<String>>>, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-hosttool-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let applied = Arc::new(Mutex::new(Vec::new()));
        let a2 = applied.clone();
        let p = Panel { conf: d.join("system.conf"), etc: d.join("hostname"), cmdline: Arc::new(move || cmdline.to_string()), current: Arc::new(|| "old-name".to_string()), apply: Arc::new(move |n, f| {
            a2.lock().unwrap().push(n.to_string());
            std::fs::write(f, format!("{n}\n")).map_err(|e| e.to_string())
        }) };
        (p, applied, d)
    }

    fn env(d: &std::path::Path, admin: bool) -> (Env, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&d.join("settings.json")).unwrap()));
        (Env { dir: d.to_path_buf(), user: "admin".into(), admin, settings, audit: Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}"))) }, log)
    }

    #[test]
    fn the_status_is_free_the_change_asks_and_is_for_administrators_only() {
        let (p, _, d) = panel("defs", "quiet");
        let (e, _) = env(&d, true);
        let ents = entries_with(&e, p.clone());
        let names: Vec<&str> = ents.iter().map(|x| x.name).collect();
        assert_eq!(names, TOOLS);
        let st = ents.iter().find(|x| x.name == "host_name_status").unwrap();
        let set = ents.iter().find(|x| x.name == "host_name_set").unwrap();
        assert_eq!(st.risk, iyakki_shared::agent::Risk::Safe);
        assert_eq!(set.risk, iyakki_shared::agent::Risk::Ask);
        assert!(set.admin_only && !st.admin_only);
        let said = (st.call)(&json!({})).unwrap();
        assert!(said.contains("old-name") && said.contains("No name is kept"), "{said}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_rename_is_checked_kept_applied_and_audited_and_a_refusal_changes_nothing() {
        let (p, applied, d) = panel("set", "quiet");
        let (e, log) = env(&d, true);
        let ents = entries_with(&e, p.clone());
        let set = ents.iter().find(|x| x.name == "host_name_set").unwrap();
        for bad in [json!({}), json!({"name": ""}), json!({"name": "Kitchen"}), json!({"name": "-x"}), json!({"name": "a b"}), json!({"name": "x".repeat(64)})] {
            assert!((set.refusal.as_ref().unwrap())(&bad).is_some(), "{bad}");
            assert!((set.call)(&bad).is_err(), "{bad}");
        }
        assert!(applied.lock().unwrap().is_empty() && !p.conf.exists(), "nothing was written by a refusal");
        assert_eq!((set.preview.as_ref().unwrap())(&json!({"name": "kitchen-mac"})), "Now: old-name\nAfter: kitchen-mac");
        let said = (set.call)(&json!({"name": "kitchen-mac"})).unwrap();
        assert!(said.contains("kitchen-mac"), "{said}");
        assert_eq!(applied.lock().unwrap().as_slice(), ["kitchen-mac"]);
        assert_eq!(std::fs::read_to_string(&p.etc).unwrap(), "kitchen-mac\n");
        assert_eq!(crate::hostname::saved(&p.conf).as_deref(), Some("kitchen-mac"));
        assert_eq!(log.lock().unwrap().as_slice(), ["hostname.apply kitchen-mac"]);
        // the status now says what is kept
        let st = ents.iter().find(|x| x.name == "host_name_status").unwrap();
        assert!((st.call)(&json!({})).unwrap().contains("Kept for the next starts: kitchen-mac"));
        // an apply that fails says the name was kept anyway
        let mut failing = p.clone();
        failing.apply = Arc::new(|_, _| Err("not allowed".into()));
        let ents = entries_with(&e, failing);
        let set = ents.iter().find(|x| x.name == "host_name_set").unwrap();
        assert!((set.call)(&json!({"name": "second-name"})).unwrap_err().contains("kept for the next starts, but not applied now"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_name_fixed_by_the_boot_option_is_said_and_never_changed() {
        let (p, applied, d) = panel("locked", "quiet iyakki.hostname=lab-pc");
        let (e, _) = env(&d, true);
        let ents = entries_with(&e, p.clone());
        let set = ents.iter().find(|x| x.name == "host_name_set").unwrap();
        assert!((set.refusal.as_ref().unwrap())(&json!({"name": "other"})).unwrap().contains("iyakki.hostname=lab-pc"));
        assert!((set.call)(&json!({"name": "other"})).unwrap_err().contains("fixed by the boot option"));
        assert!(applied.lock().unwrap().is_empty() && !p.conf.exists());
        let st = ents.iter().find(|x| x.name == "host_name_status").unwrap();
        assert!((st.call)(&json!({})).unwrap().contains("cannot be changed here"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_tools_are_in_the_prompt_and_the_frame_hides_the_change_from_an_ordinary_account() {
        for n in TOOLS {
            assert!(PROMPT_RULE.contains(n));
        }
        let (p, _, d) = panel("frame", "quiet");
        let (e, _) = env(&d, false);
        let frame = iyakki_shared::settingstools::SettingsTools::new(Arc::new(NoInner), false, entries_with(&e, p));
        let defs = frame.defs();
        assert!(defs.iter().any(|x| x.spec.name == "host_name_status") && !defs.iter().any(|x| x.spec.name == "host_name_set"), "{:?}", defs.iter().map(|x| &x.spec.name).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(d);
    }

    struct NoInner;
    impl Tools for NoInner {
        fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
            vec![]
        }
        fn call(&self, n: &str, _: &Value) -> Result<String, String> {
            Err(format!("unknown {n}"))
        }
        fn describe(&self, n: &str, _: &Value) -> String {
            n.into()
        }
    }
}

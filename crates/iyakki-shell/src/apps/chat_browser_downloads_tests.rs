//! The assistant's tools for saved downloads and site reports: reads are free, changes ask every time, a download opens through the
//! same check as the Downloads list, and the request waits for the shell (F-BROX-01, F-BROW-41).

use std::sync::{Arc, Mutex};

use iyakki_agent::agent::{Risk, Tools};
use iyakki_agent::settingstools::{Env, SettingsTools};
use serde_json::json;

use super::*;

struct NoTools;
impl Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _args: &serde_json::Value) -> Result<String, String> {
        Err(format!("no tool {name}"))
    }
    fn describe(&self, name: &str, _args: &serde_json::Value) -> String {
        name.to_string()
    }
}

fn setup(tag: &str) -> (AppCx, SettingsTools, Arc<Mutex<Vec<String>>>) {
    let cx = crate::apps::tests::cx_with_admin_store(tag);
    let audits = Arc::new(Mutex::new(Vec::new()));
    let a2 = audits.clone();
    let settings = Arc::new(Mutex::new(iyakki_agent::settings::SettingsStore::open(&cx.services.dir().join("settings.json")).unwrap()));
    let e = Env { dir: cx.services.dir().to_path_buf(), user: cx.session.user.clone(), admin: true, settings, audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
    let t = SettingsTools::new(Arc::new(NoTools), true, entries(&cx, &e));
    (cx, t, audits)
}

#[test]
fn downloads_are_listed_and_opened_by_chat_through_the_same_check_as_the_list_and_the_shell_takes_the_request() {
    let (cx, t, audits) = setup("chatdl1");
    let defs = t.defs();
    for (n, risk) in [("browser_downloads_list", Risk::Safe), ("browser_download_open", Risk::Ask), ("browser_reports_list", Risk::Safe), ("browser_reports_clear", Risk::Ask)] {
        assert_eq!(defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n}")).risk, risk, "{n}");
    }
    assert_eq!(t.call("browser_downloads_list", &json!({})).unwrap(), "No saved downloads.");
    let dir = super::super::browser::BrowserApp::downloads_dir(&cx).unwrap();
    let mut d = bd::load(&cx);
    for (name, body) in [("notes.txt", "hello"), ("pic.png", "x")] {
        std::fs::write(dir.join(name), body).unwrap();
        d.add_download(bd::Download { url: format!("https://x.org/{name}"), name: name.into(), bytes: body.len() as u64, at: 1, status: "saved".into() });
    }
    bd::save(&cx, &d);
    let list = t.call("browser_downloads_list", &json!({})).unwrap();
    assert!(list.contains("notes.txt") && list.contains("tile edit") && list.contains("pic.png (1 bytes, no tile opens it)"), "{list}");
    // the same refusals as a click in the list: a name with a folder, a hidden name, a kind no tile opens, a file that is gone
    for bad in ["../notes.txt", ".hidden.txt", "sub/notes.txt", ""] {
        assert!(t.call("browser_download_open", &json!({"name": bad})).is_err(), "{bad}");
    }
    assert!(t.call("browser_download_open", &json!({"name": "pic.png"})).unwrap_err().contains("no tile"));
    assert!(t.call("browser_download_open", &json!({"name": "gone.txt"})).unwrap_err().contains("not in Downloads"));
    assert!(super::super::download_open::take_chat(&cx.session.user).is_none(), "refused calls queue nothing");
    // an approved call queues the request for this person only; the shell takes it once
    let r = t.call("browser_download_open", &json!({"name": "notes.txt"})).unwrap();
    assert!(r.contains("EDIT"), "{r}");
    assert!(super::super::download_open::take_chat("someone-else").is_none());
    let (tile, path) = super::super::download_open::take_chat(&cx.session.user).expect("queued");
    assert_eq!((tile.as_str(), path.ends_with("notes.txt")), ("edit", true));
    assert!(super::super::download_open::take_chat(&cx.session.user).is_none(), "taken once");
    assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("browser.download.open edit")));
    // the person's card names the file
    assert!(t.describe("browser_download_open", &json!({"name": "notes.txt"})).contains("notes.txt"));
}

#[test]
fn site_reports_are_listed_without_paths_and_deleted_only_when_asked() {
    let (cx, t, audits) = setup("chatdl2");
    assert_eq!(t.call("browser_reports_list", &json!({})).unwrap(), "No reports saved.");
    let l = super::super::browser_report::line(1700000000, "shop.example.org", 200, true, "reader", &["script error in https://cdn.example.org/a.js?token=SECRET: x".to_string()]);
    super::super::browser_report::append(&super::super::browser_report::file_for(cx.services.dir(), &cx.session.user), &l).unwrap();
    let list = t.call("browser_reports_list", &json!({})).unwrap();
    assert!(list.contains("shop.example.org") && list.contains("status 200") && list.contains("1 notes") && !list.contains("SECRET"), "{list}");
    t.call("browser_reports_clear", &json!({})).unwrap();
    assert_eq!(t.call("browser_reports_list", &json!({})).unwrap(), "No reports saved.");
    assert!(audits.lock().unwrap().iter().any(|a| a.starts_with("browser.report.clear")));
    assert_eq!(t.call("browser_reports_clear", &json!({})).unwrap(), "There were no reports.");
}

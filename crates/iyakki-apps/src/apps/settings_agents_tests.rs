//! Settings → Agents: geometry and click paths, with a fake `claude` (no vendor is ever called).
use iyakki_agent::facets_h::RealServices as _;

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

struct Offline;
impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    key(a, cx, Key::Char(c));
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    s.chars().for_each(|c| press(a, cx, c));
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn goto_agents(a: &mut SettingsApp, _cx: &AppCx) {
    a.go_to(Tab::Agents);
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

/// A `claude` that answers `--version` and `auth status`, and logs every run of a prompt in `runs.txt`.
fn fake(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("claude");
    std::fs::write(
        &p,
        format!(
            r#"#!/bin/sh
case "$1" in
  --version) echo "2.1.0 (Claude Code)"; exit 0;;
  auth) echo '{{"loggedIn":true,"authMethod":"claude.ai"}}'; exit 0;;
esac
echo run >> {dir}/runs.txt
cat <<'JSONL'
{{"type":"result","subtype":"success","is_error":false,"result":"OK","session_id":"s1","usage":{{"input_tokens":3,"output_tokens":1}}}}
JSONL
"#,
            dir = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn cfg(cx: &AppCx) -> iyakki_agent::extagent::AgentsCfg {
    cx.settings().lock().unwrap().get().agents.clone()
}

#[test]
fn the_agents_page_is_for_administrators_and_sits_next_to_models() {
    let cx = admin("agp1");
    let a = SettingsApp::new(&cx);
    let m = a.tabs().iter().position(|t| *t == Tab::Models).unwrap();
    assert_eq!(a.tabs()[m + 1], Tab::Agents, "Agents sits right after Models");
    assert_eq!(Tab::Agents.group(), "Apps & AI");
    let mut user = admin("agp1b");
    user.session.user = "amy".into();
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&user).tabs().contains(&Tab::Agents));
    // found by searching for the vendors' names
    let mut a = SettingsApp::new(&cx);
    key(&mut a, &cx, Key::Ctrl('f'));
    typ(&mut a, &cx, "copilot");
    assert_eq!(a.tab(), Tab::Agents);
}

#[test]
fn the_page_lists_the_agents_with_status_chips_and_every_button_is_on_screen() {
    let cx = admin("agp2");
    let bin = temp_dir("agp2-bin");
    let claude = fake(&bin);
    cx.settings().lock().unwrap().update(|s| s.agents.claude.command = claude.display().to_string()).unwrap();
    cx.settings().lock().unwrap().update(|s| s.agents.copilot.command = "/nonexistent/copilot-xyz".into()).unwrap();
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    for (w, h) in [(1280u32, 720u32), (800, 500), (1920, 1080)] {
        let g = draw(&mut a, &cx, w, h);
        let rows: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).collect();
        assert_eq!(rows.len(), 2, "Claude Code and GitHub Copilot at {w}x{h}");
        assert!(rows[0].rect.y < rows[1].rect.y);
        let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_)))).collect();
        assert!(buttons.len() >= 12, "{} buttons at {w}x{h}", buttons.len());
        for b in &buttons {
            assert!(b.rect.x >= 0.0 && b.rect.y >= 0.0 && b.rect.x + b.rect.w <= w as f32 + 0.5 && b.rect.y + b.rect.h <= h as f32 + 0.5, "{:?} at {w}x{h}", b.rect);
        }
    }
    // Detect (d) looks at the programs: --version and the sign-in status only
    press(&mut a, &cx, 'd');
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if cx.ext_status().get("claude-code").is_some_and(|c| c.installed) && cx.ext_status().get("copilot").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let c = cx.ext_status().get("claude-code").unwrap();
    assert!(c.ready() && c.version.contains("2.1.0"), "{c:?}");
    let p = cx.ext_status().get("copilot").unwrap();
    assert!(!p.installed && p.chip == "not installed", "{p:?}");
    assert!(!bin.join("runs.txt").exists(), "detecting never sends a request");
    draw(&mut a, &cx, 1280, 720);
}

#[test]
fn model_key_auto_default_and_hiding_are_set_with_keys_and_clicks() {
    let cx = admin("agp3");
    cx.vault().system_set("anthropic-key", "sk-ant-NOT-A-REAL-KEY").unwrap();
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    // model: a list of the agent's models (the built-in list), filtered by typing
    press(&mut a, &cx, 'm');
    assert!(a.has_modal());
    typ(&mut a, &cx, "sonnet");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    assert_eq!(cfg(&cx).claude.model, "sonnet");
    // Default clears it
    press(&mut a, &cx, 'm');
    typ(&mut a, &cx, "default");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cfg(&cx).claude.model, "");
    // a custom id: asked in a form, validated, remembered for the agent
    press(&mut a, &cx, 'm');
    typ(&mut a, &cx, "custom");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal());
    typ(&mut a, &cx, "bad id; x");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "shell characters are refused and the form stays open");
    key(&mut a, &cx, Key::Esc);
    press(&mut a, &cx, 'm');
    typ(&mut a, &cx, "custom");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "claude-sonnet-5");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    assert_eq!((cfg(&cx).claude.model.as_str(), cfg(&cx).claude.custom_models.clone()), ("claude-sonnet-5", vec!["claude-sonnet-5".to_string()]));
    press(&mut a, &cx, 'm');
    typ(&mut a, &cx, "sonnet");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cfg(&cx).claude.model, "sonnet");
    // API key from the vault: only a vault reference is stored, never the value
    press(&mut a, &cx, 'k');
    assert!(a.has_modal());
    typ(&mut a, &cx, "anthropic");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cfg(&cx).claude.key_ref, "vault:anthropic-key");
    let saved = std::fs::read_to_string(cx.data_dir().join("settings.json")).expect("the settings file");
    assert!(saved.contains("vault:anthropic-key"));
    assert!(!saved.contains("NOT-A-REAL-KEY"), "the key itself is not in the settings file");
    // Auto: off by default, asks before it is turned on, one key turns it off
    assert!(!cfg(&cx).claude.allow_auto);
    press(&mut a, &cx, 'a');
    assert!(a.has_modal(), "turning Auto on asks first");
    press(&mut a, &cx, 'n');
    assert!(!cfg(&cx).claude.allow_auto);
    press(&mut a, &cx, 'a');
    press(&mut a, &cx, 'y');
    assert!(cfg(&cx).claude.allow_auto && status(&a).contains("Auto is allowed"), "{}", status(&a));
    press(&mut a, &cx, 'a');
    assert!(!cfg(&cx).claude.allow_auto);
    // second row: Copilot; use for new chats; hide
    key(&mut a, &cx, Key::Down);
    press(&mut a, &cx, 'u');
    assert_eq!(cfg(&cx).default.as_deref(), Some("copilot"));
    press(&mut a, &cx, 'x');
    assert!(!cfg(&cx).copilot.enabled);
    let g = {
        // a hidden agent is not offered in the chat's list
        let entries = iyakki_shared::extagent::entries_with(&cfg(&cx), &|id| cx.ext_status().get(id));
        entries.iter().map(|e| e.id.clone()).collect::<Vec<_>>()
    };
    assert_eq!(g, ["claude-code"]);
    press(&mut a, &cx, 'U');
    assert_eq!(cfg(&cx).default, None);
    // a click on a row selects it
    draw(&mut a, &cx, 1280, 720);
    a.handle(&Event::Row(0), &cx);
    assert_eq!(a.sel(), 0);
}

#[test]
fn key_v_on_the_copilot_row_switches_it_between_working_through_the_tools_and_a_read_only_advisor_and_only_there() {
    let cx = admin("ag-advisor");
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    assert!(!cfg(&cx).copilot.advisor_only, "through IYAKKI's tools and approvals by default");
    // on the first row (Claude Code) the key does nothing to Copilot
    press(&mut a, &cx, 'v');
    assert!(!cfg(&cx).copilot.advisor_only);
    key(&mut a, &cx, Key::Down);
    press(&mut a, &cx, 'v');
    assert!(cfg(&cx).copilot.advisor_only);
    assert!(status(&a).contains("is an advisor now: read-only, no IYAKKI tools; you apply its edits."), "{}", status(&a));
    // the agent's own configuration follows: the advisor has no write or shell tool offered
    let entries = iyakki_shared::extagent::entries_with(&cfg(&cx), &|id| cx.ext_status().get(id));
    assert!(entries.iter().any(|e| e.id == "copilot"));
    press(&mut a, &cx, 'v');
    assert!(!cfg(&cx).copilot.advisor_only);
    assert!(status(&a).contains("works through IYAKKI's tools and approvals again"), "{}", status(&a));
}

#[test]
fn install_shows_the_exact_command_and_asks_and_login_names_the_vendors_command() {
    let cx = admin("agp4");
    cx.settings().lock().unwrap().update(|s| s.agents.claude.command = "/nonexistent/claude-xyz".into()).unwrap();
    cx.ext_status().set("claude-code", iyakki_agent::extagent::detect(cfg(&cx).agent("claude-code").unwrap().as_ref(), &iyakki_shared::wsexec::System, &std::env::temp_dir()));
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    cx.services.real().clis.set_npm("/nonexistent/npm", "/nonexistent/node");
    press(&mut a, &cx, 'i');
    // the same plan as the CLI tile shows: the exact command, the folder, and (here) that Node.js is missing
    match &a.modal {
        Some(Modal::Confirm(ConfirmKind::AgentInstall(id), text)) => {
            assert_eq!(id, "claude-code");
            assert!(text.contains("downloads.claude.ai") && text.contains("no Node.js") && text.contains("`npm install -g @anthropic-ai/claude-code`"), "{text}");
        }
        _ => panic!("expected a confirmation, got {}", status(&a)),
    }
    // anything but y cancels; nothing was installed
    press(&mut a, &cx, 'n');
    assert!(!a.has_modal() && !a.is_busy() && cx.services.clis().installed().is_empty());
    // yes with no Node.js: it is installed first from the catalogue; here there is no network, so that is what the status says
    press(&mut a, &cx, 'i');
    press(&mut a, &cx, 'y');
    assert!(status(&a).contains("network") || status(&a).contains("Node.js"), "{}", status(&a));
    assert!(cx.services.clis().job("claude-code").is_none());
    // Login before it is installed says to install it first; once it is there the shell is asked for the sign-in dialog
    press(&mut a, &cx, 'l');
    assert!(status(&a).contains("not installed yet"), "{}", status(&a));
    assert_eq!(crate::agent_login::take_request(), None);
}

#[test]
fn the_test_button_sends_one_request_only_after_the_person_says_yes() {
    let cx = admin("agp5");
    let bin = temp_dir("agp5-bin");
    let claude = fake(&bin);
    cx.settings().lock().unwrap().update(|s| s.agents.claude.command = claude.display().to_string()).unwrap();
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    press(&mut a, &cx, 't');
    assert!(a.has_modal(), "it asks first");
    press(&mut a, &cx, 'n');
    assert!(!bin.join("runs.txt").exists(), "no request was made");
    press(&mut a, &cx, 't');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(status(&a).contains("answered") && status(&a).contains("OK"), "{}", status(&a));
    assert_eq!(std::fs::read_to_string(bin.join("runs.txt")).unwrap().lines().count(), 1, "exactly one request");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("agents.test")));
}

#[test]
fn a_custom_agent_is_added_edited_and_removed_and_a_bad_one_is_refused_in_the_form() {
    let cx = admin("agp6");
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    press(&mut a, &cx, 'n');
    typ(&mut a, &cx, "aider");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "Aider");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "aider --yes --message {prompt}");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "bad key");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "the key variable must look like OPENAI_API_KEY: the form stays open");
    key(&mut a, &cx, Key::Esc);
    press(&mut a, &cx, 'n');
    typ(&mut a, &cx, "aider");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "Aider");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "aider --yes --message {prompt}");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{}", status(&a));
    let c = cfg(&cx).custom;
    assert_eq!((c.len(), c[0].id.as_str(), c[0].name.as_str()), (1, "aider", "Aider"));
    draw(&mut a, &cx, 1280, 720);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    assert_eq!(a.sel(), 2, "the custom agent is the third row");
    press(&mut a, &cx, 'R');
    press(&mut a, &cx, 'y');
    assert!(cfg(&cx).custom.is_empty());
}

#[test]
fn the_project_context_switches_are_in_settings_agents_for_the_workspace_last_opened_and_say_they_are_shared() {
    let cx = admin("ctxsettings");
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    // no workspace: they say so and change nothing
    press(&mut a, &cx, 'c');
    assert!(status(&a).contains("Open a workspace"), "{}", status(&a));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.width() > 0);
    // a workspace
    let root = temp_dir("ctxsettings-ws");
    std::fs::write(root.join("CLAUDE.md"), "# Mine\n\nKeep this.\n").unwrap();
    let mut reg = iyakki_shared::workspace::Registry::load(cx.data_dir(), &cx.session.user);
    let w = reg.add("Shop", &root).unwrap();
    reg.last = Some(w.id.clone());
    reg.save().unwrap();
    press(&mut a, &cx, 'c');
    assert!(status(&a).contains("Shop") && status(&a).contains("clones the project"), "{}", status(&a));
    assert_eq!(iyakki_shared::notes::load(&root).0.settings.agents_may_add, iyakki_shared::notes::AgentsMayAdd::Ask);
    press(&mut a, &cx, 'c');
    press(&mut a, &cx, 'c');
    assert_eq!(iyakki_shared::notes::load(&root).0.settings.agents_may_add, iyakki_shared::notes::AgentsMayAdd::On);
    press(&mut a, &cx, 'g');
    assert!(iyakki_shared::notes::load(&root).0.settings.share_external);
    let t = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
    assert!(t.starts_with("# Mine\n\nKeep this.") && t.contains(iyakki_shared::notes::BEGIN), "{t}");
    press(&mut a, &cx, 'g');
    assert!(!std::fs::read_to_string(root.join("CLAUDE.md")).unwrap().contains(iyakki_shared::notes::BEGIN));
    // it fits at the four sizes
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        draw(&mut a, &cx, w, h);
    }
}

#[test]
fn login_and_logout_ask_the_shell_for_an_installed_agent_and_the_page_shows_who_is_signed_in() {
    let cx = admin("agp-login");
    let bin = temp_dir("agp-login-bin");
    let claude = fake(&bin);
    cx.settings().lock().unwrap().update(|s| s.agents.claude.command = claude.display().to_string()).unwrap();
    cx.settings().lock().unwrap().update(|s| s.agents.copilot.command = "/nonexistent/copilot-xyz".into()).unwrap();
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    press(&mut a, &cx, 'd');
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        if cx.ext_status().get("claude-code").is_some() && cx.ext_status().get("copilot").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Claude Code (selected) is installed and signed in: Login and Logout are drawn and the keys ask the shell
    // the status is looked up on a thread of its own and a busy machine can answer late: draw again until Logout shows (up to 20 s)
    let mut g = draw(&mut a, &cx, 1280, 720);
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(60) {
        if g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGOUT_TAB)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        g = draw(&mut a, &cx, 1280, 720);
    }
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB)) && g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGOUT_TAB)));
    press(&mut a, &cx, 'l');
    assert_eq!(crate::agent_login::take_request(), Some(("claude-code", false)));
    press(&mut a, &cx, 'o');
    assert_eq!(crate::agent_login::take_request(), Some(("claude-code", true)));
    // Copilot is not installed: no buttons, and Login says to install it first
    key(&mut a, &cx, Key::Down);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if crate::agent_login::agent_of_tab(n).is_some())));
    press(&mut a, &cx, 'l');
    assert_eq!(crate::agent_login::take_request(), None);
    assert!(status(&a).contains("not installed"), "{}", status(&a));
}

// ---------------------------------------------------------------------------------------------------- native_pdf_output (F-PDF-14)

struct NoTools;
impl iyakki_shared::agent::Tools for NoTools {
    fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &serde_json::Value) -> Result<String, String> {
        Err(format!("unknown {name}"))
    }
    fn describe(&self, name: &str, _: &serde_json::Value) -> String {
        name.into()
    }
}

#[test]
fn the_pdf_output_switch_is_a_key_on_the_agents_and_models_pages_and_the_chat_tool_flips_the_same_setting() {
    use iyakki_shared::agent::Tools;
    use iyakki_agent::pdfnativetools::{is_native, PdfOutputTools, RealBackend};
    let cx = admin("pdfout");
    let mut a = SettingsApp::new(&cx);
    let tool = PdfOutputTools::new(Arc::new(NoTools), Arc::new(RealBackend { svc: iyakki_agent::services::Services::from_handle(&cx.services.clone() )}));
    let texts = |a: &mut SettingsApp| draw(a, &cx, 1280, 720).text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n");
    // Agents: key p on the chosen agent (Claude Code is the first row)
    goto_agents(&mut a, &cx);
    assert!(!cfg(&cx).claude.native_pdf_output && !texts(&mut a).contains("writes PDFs"));
    press(&mut a, &cx, 'p');
    assert!(cfg(&cx).claude.native_pdf_output && status(&a).contains("pdf_create is not offered"), "{}", status(&a));
    assert!(texts(&mut a).contains("writes PDFs"), "the row says so");
    let agents = cx.agents_cfg();
    assert!(is_native(SettingsApp::svc(&cx).settings().lock().unwrap().get(), &agents, Some(iyakki_agent::extagent::CLAUDE_CODE)), "and the chat reads the same switch");
    press(&mut a, &cx, 'p');
    assert!(!cfg(&cx).claude.native_pdf_output && status(&a).contains("offered the assistant's pdf_create again"));
    // the chat tool: status is free, the change asks, and it lands in the same setting the page shows
    assert!(tool.call("pdf_output_status", &serde_json::json!({})).unwrap().contains("agent:claude (Claude Code): pdf_create offered"));
    tool.call("pdf_output_set", &serde_json::json!({"target": "agent:claude", "native": true})).unwrap();
    assert!(cfg(&cx).claude.native_pdf_output && texts(&mut a).contains("writes PDFs"));
    tool.call("pdf_output_set", &serde_json::json!({"target": "agent:claude", "native": false})).unwrap();
    assert!(!cfg(&cx).claude.native_pdf_output);
    // Models: key P on the chosen provider
    SettingsApp::svc(&cx).settings().lock().unwrap().update(|s| {
        let mut p = ProviderCfg::new("local", Kind::Ollama);
        p.models = vec!["m".into()];
        s.providers.push(p);
        s.chat = Some(ModelChoice { provider: "local".into(), model: "m".into() });
    }).unwrap();
    a.go_to(Tab::Models);
    let native = || SettingsApp::svc(&cx).settings().lock().unwrap().get().providers[0].native_pdf_output;
    let rule = || {
        let agents = cx.agents_cfg();
        let st = SettingsApp::svc(&cx).settings().lock().unwrap();
        is_native(st.get(), &agents, None)
    };
    assert!(!native() && !rule());
    press(&mut a, &cx, 'P');
    assert!(native() && rule() && status(&a).contains("local is marked as writing PDF files itself"), "{}", status(&a));
    assert!(texts(&mut a).contains("writes PDFs"));
    press(&mut a, &cx, 'P');
    assert!(!native() && !rule());
    tool.call("pdf_output_set", &serde_json::json!({"target": "local", "native": true})).unwrap();
    assert!(native() && rule());
    assert!(tool.call("pdf_output_status", &serde_json::json!({})).unwrap().contains("local (Ollama): writes PDF files itself"));
    // a person who may not change settings cannot flip it from the page
    let mut ucx = cx_with_admin_store("pdfout-user");
    ucx.session.policy.manage_settings = false;
    ucx.services = cx.services.clone();
    let mut u = SettingsApp::new(&ucx);
    u.go_to(Tab::Models);
    press(&mut u, &ucx, 'P');
    assert!(native(), "the page ignores P for a non-administrator");
}

#[test]
fn key_v_on_the_copilot_row_switches_it_between_the_iyakkis_tools_and_a_read_only_advisor_and_does_nothing_on_the_others() {
    let cx = admin("agp-v");
    let mut a = SettingsApp::new(&cx);
    goto_agents(&mut a, &cx);
    // the first row (Claude Code): v is not for it
    press(&mut a, &cx, 'v');
    assert!(!cfg(&cx).copilot.advisor_only && !status(&a).contains("advisor"), "{}", status(&a));
    // the Copilot row: v makes it an advisor, and again takes it back to IYAKKI's tools
    key(&mut a, &cx, Key::Down);
    assert!(!cfg(&cx).copilot.advisor_only);
    press(&mut a, &cx, 'v');
    assert!(cfg(&cx).copilot.advisor_only, "{}", status(&a));
    assert!(status(&a).starts_with("GitHub Copilot is an advisor now: read-only, no IYAKKI tools"), "{}", status(&a));
    // the row says which mode it is in and offers the other
    press(&mut a, &cx, 'v');
    assert!(!cfg(&cx).copilot.advisor_only);
    assert!(status(&a).starts_with("GitHub Copilot works through IYAKKI's tools and approvals again"), "{}", status(&a));
    // the choice is audited like every setting of this page
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("agents")), "{:?}", cx.audit.recent(20));
}

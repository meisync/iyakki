//! The model list with external coding agents in it, and a chat answered by one (a fake CLI; no vendor is ever called).

use std::os::unix::fs::PermissionsExt;

use super::tests::render_with;
use super::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::model_picker::MRow;
use crate::apps::App;
use iyakki_agent::extagent::{self, Auth, Mode, Status};

fn fake_claude(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("claude");
    std::fs::write(
        &p,
        r#"#!/bin/sh
case "$1" in
  --version) echo "2.1.0 (Claude Code)"; exit 0;;
  auth) echo '{"loggedIn":true,"authMethod":"claude.ai"}'; exit 0;;
esac
cat <<'JSONL'
{"type":"system","subtype":"init","session_id":"sess-77","model":"fake-model"}
{"type":"assistant","message":{"content":[{"type":"text","text":"All done here."}]}}
{"type":"result","subtype":"success","is_error":false,"result":"All done here.","session_id":"sess-77","total_cost_usd":0.01,"usage":{"input_tokens":12,"output_tokens":5}}
JSONL
"#,
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn status(installed: bool, auth: Auth) -> Status {
    let (chip, fix) = if !installed { ("not installed".to_string(), "Claude Code is not installed. Install it with `npm install -g @anthropic-ai/claude-code`".to_string()) } else if auth == Auth::SignedOut { ("sign in".to_string(), "Open the terminal and run `claude auth login`".to_string()) } else { ("ready".to_string(), String::new()) };
    Status { installed, version: if installed { "2.1.0".into() } else { String::new() }, auth, mode: Mode::Bridged, chip, fix }
}

fn with_models(tag: &str) -> (ChatApp, AppCx) {
    let (mut app, mut cx, _h) = {
        let (cx, d) = cx_with_backend(tag, iyakki_agent::chat::Unconfigured);
        (ChatApp::new(), cx, d)
    };
    cx.session.policy.manage_settings = true;
    app.msgs.push(Message::user("hi"));
    app.msgs.push(Message::assistant("hello".to_string()));
    let mut a = iyakki_agent::providers::ProviderCfg::new("nvidia_ollama", iyakki_agent::providers::Kind::Ollama);
    a.models = vec!["qwen3:8b".into(), "qwen2.5-coder:14b".into()];
    cx.services.settings.lock().unwrap().upsert_provider(a).unwrap();
    (app, cx)
}

#[test]
fn the_model_list_shows_models_then_agents_with_their_status_and_the_fix_when_unavailable() {
    let (mut app, cx) = with_models("chatext1");
    cx.services.ext_status.set("claude-code", status(false, Auth::Unknown));
    cx.services.ext_status.set("copilot", status(true, Auth::Unknown));
    cx.services.settings.lock().unwrap().update(|s| s.agents.custom.push(extagent::CustomCfg { id: "aider".into(), name: "Aider".into(), command: "aider --yes".into(), ..Default::default() })).unwrap();
    cx.services.ext_status.set("custom:aider", status(true, Auth::Unknown));
    app.handle(&Event::Key(Key::Ctrl('p')), &cx);
    let items = app.models.as_ref().unwrap().items.clone();
    let names: Vec<String> = items.iter().map(|(p, m)| format!("{p}/{m}")).collect();
    let expect: Vec<&str> = vec![
        "nvidia_ollama/qwen3:8b", "nvidia_ollama/qwen2.5-coder:14b",
        "agent:claude-code/", "agent:claude-code/default", "agent:claude-code/sonnet", "agent:claude-code/opus", "agent:claude-code/haiku", "agent:claude-code/fable", "agent:claude-code/opusplan", "agent:claude-code/custom model id",
        "agent:copilot/", "agent:copilot/default", "agent:copilot/custom model id",
        "agent:custom:aider/", "agent:custom:aider/default", "agent:custom:aider/custom model id",
    ];
    assert_eq!(names, expect, "one flat list: the models, then each agent with its models under it");
    let g = render_with(&mut app, &cx, 1100, 650);
    let rows: Vec<_> = g.hits.iter().filter(|h| h.label == "model").collect();
    assert!(rows.len() >= 12 && rows.len() <= 16, "every visible entry is a click target, unavailable agents too: {}", rows.len());
    for h in &rows {
        assert!(h.rect.x >= 0.0 && h.rect.y >= 0.0 && h.rect.x + h.rect.w <= 1100.0 && h.rect.y + h.rect.h <= 650.0, "{:?} is on screen", h.rect);
    }
    assert!(rows.windows(2).all(|w| w[0].rect.y < w[1].rect.y), "rows do not overlap");
    // choosing the one that is not installed selects it and shows the exact fix instead of failing silently
    app.handle(&Event::Row(2), &cx);
    assert!(app.models.is_none());
    assert_eq!(app.ext_id(&cx).as_deref(), Some("claude-code"));
    assert_eq!(app.ext_model_of(&cx, "claude-code"), "", "the agent line keeps the agent's own model");
    assert!(app.error.as_deref().is_some_and(|e| e.contains("npm install -g @anthropic-ai/claude-code")), "{:?}", app.error);
    assert!(app.ext_footer(&cx).unwrap().starts_with("agent: Claude Code · Default · new session"), "{:?}", app.ext_footer(&cx));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("chat.agent") && l.contains("claude-code")));
    // the list marks the chosen agent and the built-in model is no longer the chosen one
    app.handle(&Event::Key(Key::Ctrl('p')), &cx);
    assert_eq!(app.models.as_ref().unwrap().sel, 3, "the list opens on the model in use: Default under Claude Code");
    render_with(&mut app, &cx, 1100, 650);
    // back to a built-in model: the agent is dropped for this chat
    app.handle(&Event::Row(0), &cx);
    assert_eq!(app.ext_id(&cx), None);
    assert_eq!(cx.services.settings.lock().unwrap().get().chat.clone().map(|c| c.model), Some("qwen3:8b".to_string()));
}

#[test]
fn an_agent_choice_is_remembered_for_the_workspace_and_an_account_without_commands_cannot_pick_one() {
    let (mut app, mut cx) = with_models("chatext2");
    cx.services.ext_status.set("copilot", status(true, Auth::Unknown));
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Off;
    app.handle(&Event::Key(Key::Ctrl('p')), &cx);
    let copilot = app.models.as_ref().unwrap().items.iter().position(|(p, m)| p == "agent:copilot" && m.is_empty()).unwrap();
    app.handle(&Event::Row(copilot), &cx);
    assert_eq!(app.ext_id(&cx), None);
    assert!(app.error.as_deref().is_some_and(|e| e.contains("may not run commands")), "{:?}", app.error);
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Full;
    // an administrator's choice outside a workspace becomes the default of new chats
    app.handle(&Event::Key(Key::Ctrl('p')), &cx);
    app.handle(&Event::Row(copilot), &cx);
    assert_eq!(cx.services.settings.lock().unwrap().get().agents.default.as_deref(), Some("copilot"));
    let mut fresh = ChatApp::new();
    assert_eq!(fresh.ext_id(&cx).as_deref(), Some("copilot"), "a new chat starts with it");
    // a default that no longer exists is ignored
    cx.services.settings.lock().unwrap().update(|s| s.agents.default = Some("custom:gone".into())).unwrap();
    assert_eq!(ChatApp::new().ext_id(&cx), None);
}

#[test]
fn a_message_to_an_agent_runs_it_in_the_chats_folder_and_shows_the_answer_with_vendor_usage() {
    let (mut app, cx) = with_models("chatext3");
    let bin = std::env::temp_dir().join(format!("iyakki-chatext3-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap();
    let claude = fake_claude(&bin);
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.command = claude.display().to_string()).unwrap();
    app.choose_agent(&cx, "claude-code".into());
    app.agent = true;
    app.start_run(&cx, "say something".into());
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(app.run.is_some(), "the agent run started");
    for _ in 0..600 {
        app.poll_run(&cx);
        if app.run.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.run.is_none(), "the run ended");
    assert!(app.messages().iter().any(|m| m.role == Role::Assistant && m.content.contains("All done here.")), "{:?}", app.messages().iter().map(|m| m.content.clone()).collect::<Vec<_>>());
    // the vendor's numbers are in USAGE and the next message will resume the vendor session
    let u = cx.services.usage.entries(Some("tester"));
    assert!(u.iter().any(|e| e.model == "fake-model" && e.input == 12 && e.output == 5), "{u:?}");
    let sid = app.session_id().unwrap().to_string();
    assert_eq!(cx.services.ext_sessions.get(&sid, "claude-code").as_deref(), Some("sess-77"));
    assert!(app.ext_footer(&cx).unwrap().contains("session sess-77"), "{:?}", app.ext_footer(&cx));
    assert!(app.usage_text().contains("17"), "the footer's last-run numbers come from the vendor: {}", app.usage_text());
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
fn a_missing_agent_says_how_to_fix_it_in_the_chat_and_autopilot_is_not_pretended() {
    let (mut app, cx) = with_models("chatext4");
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.command = "/nonexistent/claude-xyz".into()).unwrap();
    app.choose_agent(&cx, "claude-code".into());
    app.agent = true;
    app.start_run(&cx, "hello".into());
    for _ in 0..300 {
        app.poll_run(&cx);
        if app.run.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let err = app.error.clone().unwrap_or_default() + &app.messages().iter().map(|m| m.content.clone()).collect::<String>();
    assert!(err.contains("not installed") && err.contains("npm install -g @anthropic-ai/claude-code"), "{err}");
    // Autopilot needs an agent that can use IYAKKI's tools: Copilot set to advisor only is read-only, so it is refused with the way out
    cx.services.settings.lock().unwrap().update(|s| s.agents.copilot.advisor_only = true).unwrap();
    cx.services.ext_status.set("copilot", status(true, Auth::Unknown));
    app.choose_agent(&cx, "copilot".into());
    app.autopilot = true;
    app.error = None;
    app.start_run(&cx, "build it".into());
    assert!(app.error.as_deref().is_some_and(|e| e.contains("Autopilot needs an agent that works through IYAKKI's tools") && e.contains("advisor only") && e.contains("Settings → Agents") && e.contains("Ctrl+P") && !e.contains("cannot use")), "{:?}", app.error);
    assert!(app.run.is_none());
}

#[test]
fn the_mode_chip_says_what_an_external_agent_really_does_with_the_mode() {
    let (mut app, cx) = with_models("chatext-chip");
    cx.services.ext_status.set("claude-code", status(true, Auth::Unknown));
    app.choose_agent(&cx, "claude-code".into());
    app.agent = true;
    app.autopilot = true;
    // Auto not allowed for Claude Code (the default): the chip does not pretend
    let label = app.mode_label_in(&cx);
    assert!(label.starts_with("Autopilot") && label.contains("Claude Code: each action asks"), "{label}");
    assert!(app.mode_hint_in(&cx).contains("may not use Auto yet") && app.mode_hint_in(&cx).contains("Settings → Agents"));
    for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480)] {
        let g = render_with(&mut app, &cx, w, h);
        let chip = g.hits.iter().find(|x| x.label.contains("each action asks")).unwrap_or_else(|| panic!("the chip at {w}x{h}: {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>()));
        assert!(chip.rect.x >= 0.0 && chip.rect.x + chip.rect.w <= w as f32 && chip.rect.y + chip.rect.h <= h as f32, "{:?} on screen at {w}x{h}", chip.rect);
    }
    // allowed: the plain mode, honoured
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.allow_auto = true).unwrap();
    assert_eq!(app.mode_label_in(&cx), app.mode_label());
    assert!(!app.mode_label_in(&cx).contains("each action asks"));
    // Ask first is never "not honoured"
    app.autopilot = false;
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.allow_auto = false).unwrap();
    assert_eq!(app.mode_label_in(&cx), "Ask first");
    // Autopilot for an advisor-only agent is refused, said on the chip
    cx.services.settings.lock().unwrap().update(|s| s.agents.copilot.advisor_only = true).unwrap();
    cx.services.ext_status.set("copilot", status(true, Auth::Unknown));
    app.choose_agent(&cx, "copilot".into());
    app.autopilot = true;
    let label = app.mode_label_in(&cx);
    assert!(label.contains("refused for GitHub Copilot"), "{label}");
    assert!(app.mode_hint_in(&cx).contains("advisor only"));
}

#[test]
fn autopilot_with_claude_code_runs_its_steps_through_the_agent_and_resumes_its_session() {
    let (app, cx) = with_models("chatext6");
    let root = std::env::temp_dir().join(format!("iyakki-chatext6-ws-{}", std::process::id()));
    let bin = std::env::temp_dir().join(format!("iyakki-chatext6-bin-{}", std::process::id()));
    for d in [&root, &bin] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(root.join("FEATURES.md"), "# Features\n").unwrap();
    // counts its runs, records its arguments, and answers with a result (it never touches the board)
    let script = bin.join("claude");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo 2.1.0; exit 0;; auth) echo '{{\"loggedIn\":true}}'; exit 0;; esac\nn=$(cat {d}/count 2>/dev/null || echo 0); n=$((n+1)); echo $n > {d}/count\nfor a in \"$@\"; do echo \"$a\"; done > {d}/args$n.txt\ncat <<'JSONL'\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-ap9\",\"model\":\"m\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"Looked at the goal and answered in words.\",\"session_id\":\"sess-ap9\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":10,\"output_tokens\":5}}}}\nJSONL\n",
            d = bin.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.command = script.display().to_string()).unwrap();
    let scope = ProjectScope { name: "demo".into(), root: root.canonicalize().unwrap(), key: "ws_00000000000000aa".into(), features: root.join("FEATURES.md"), cluster: None };
    let mut app = app.with_project(scope);
    app.choose_agent(&cx, "claude-code".into());
    app.agent = true;
    app.toggle_autopilot(&cx);
    assert!(app.autopilot() && app.autopilot_refusal(&cx).is_none(), "Claude Code can drive Autopilot");
    app.start_run(&cx, "make a small website".into());
    assert!(app.error.is_none(), "{:?}", app.error);
    for _ in 0..1500 {
        app.poll_run(&cx);
        if app.run.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.run.is_none(), "the autopilot run ended");
    // the harness asked for a plan twice (the model never used the board) and finished with its own summary
    let runs: usize = std::fs::read_to_string(bin.join("count")).unwrap().trim().parse().unwrap();
    assert!(runs >= 2, "{runs} steps");
    let a1 = std::fs::read_to_string(bin.join("args1.txt")).unwrap();
    let a2 = std::fs::read_to_string(bin.join("args2.txt")).unwrap();
    assert!(!a1.lines().any(|l| l == "--resume"), "step 1 starts a session");
    assert!(a2.contains("--resume\nsess-ap9"), "step 2 resumes the vendor session (stored: {:?}, file: {:?}); its arguments start {}", cx.services.ext_sessions.get(app.session_id().unwrap_or("-"), "claude-code"), std::fs::read_to_string(cx.services.dir().join("ext-sessions.json")), a2.lines().take(14).collect::<Vec<_>>().join(" "));
    assert!(a1.contains("AUTOPILOT goal: make a small website") && a1.contains("PROJECT STATE"), "{a1}");
    assert!(app.messages().iter().any(|m| m.content.contains("Autopilot finished")), "{:?}", app.messages().iter().map(|m| m.content.clone()).collect::<Vec<_>>());
    assert!(app.usage_text().contains('$'), "the vendor's cost shows: {}", app.usage_text());
    let _ = (std::fs::remove_dir_all(&root), std::fs::remove_dir_all(&bin));
}

#[test]
fn in_the_ide_the_agent_runs_in_the_workspace_with_the_project_state_and_the_choice_is_remembered_there() {
    let (app, cx) = with_models("chatext5");
    let root = std::env::temp_dir().join(format!("iyakki-chatext5-ws-{}", std::process::id()));
    let bin = std::env::temp_dir().join(format!("iyakki-chatext5-bin-{}", std::process::id()));
    for d in [&root, &bin] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    let record = bin.join("seen.txt");
    let script = fake_claude(&bin);
    // the same fake, but it also records where it ran and how it was started
    let body = std::fs::read_to_string(&script).unwrap().replace("cat <<'JSONL'", &format!("{{ pwd; for a in \"$@\"; do echo \"$a\"; done; }} > {}\ncat <<'JSONL'", record.display()));
    std::fs::write(&script, body).unwrap();
    std::fs::write(root.join("FEATURES.md"), "# Features\n").unwrap();
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.command = script.display().to_string()).unwrap();
    let scope = ProjectScope { name: "demo".into(), root: root.canonicalize().unwrap(), key: "ws_00000000000000aa".into(), features: root.join("FEATURES.md"), cluster: None };
    let mut app = app.with_project(scope.clone());
    app.choose_agent(&cx, "claude-code".into());
    assert_eq!(std::fs::read_to_string(root.join(".iyakki/agent")).unwrap(), "claude-code", "remembered for this workspace");
    assert_eq!(cx.services.settings.lock().unwrap().get().agents.default, None, "not for every chat of the system");
    assert_eq!(ChatApp::new().with_project(scope.clone()).ext_id(&cx).as_deref(), Some("claude-code"), "a new chat of the workspace starts with it");
    app.agent = true;
    app.start_run(&cx, "implement the thing".into());
    for _ in 0..600 {
        app.poll_run(&cx);
        if app.run.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let seen = std::fs::read_to_string(&record).unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines[0], root.canonicalize().unwrap().display().to_string(), "cwd is the workspace root");
    let sys = lines.iter().position(|l| *l == "--append-system-prompt").map(|i| lines[i + 1..].join("\n")).unwrap_or_default();
    assert!(sys.contains("PROJECT STATE") && sys.contains("demo"), "the workspace's PROJECT STATE reaches the agent: {sys}");
    assert!(sys.contains("history_search") && sys.contains("Do the work yourself"));
    assert!(lines.windows(2).any(|w| w[0] == "--add-dir" && w[1] == root.canonicalize().unwrap().display().to_string()));
    // going back to a built-in model is remembered too
    app.handle(&Event::Key(Key::Ctrl('p')), &cx);
    app.handle(&Event::Row(0), &cx);
    assert_eq!(std::fs::read_to_string(root.join(".iyakki/agent")).unwrap(), "-");
    let _ = (std::fs::remove_dir_all(&root), std::fs::remove_dir_all(&bin));
}

fn open_list(app: &mut ChatApp, cx: &AppCx) {
    app.handle(&Event::Key(Key::Ctrl('p')), cx);
}

fn row_of(app: &ChatApp, provider: &str, model: &str) -> usize {
    app.models.as_ref().unwrap().items.iter().position(|(p, m)| p == provider && m == model).unwrap_or_else(|| panic!("no row {provider}/{model}"))
}

#[test]
fn an_agent_expands_into_its_models_and_the_chosen_one_shows_in_the_footer() {
    let (mut app, cx) = with_models("chatext7");
    cx.services.ext_status.set("claude-code", status(true, Auth::SignedIn("claude.ai".into())));
    open_list(&mut app, &cx);
    let at = row_of(&app, "agent:claude-code", "");
    // the models follow their agent in the list, in the documented order
    let following: Vec<String> = app.models.as_ref().unwrap().items[at + 1..at + 8].iter().map(|(_, m)| m.clone()).collect();
    assert_eq!(following, ["default", "sonnet", "opus", "haiku", "fable", "opusplan", "custom model id"]);
    // keyboard: Down from the agent line reaches its models, Enter chooses
    app.models.as_mut().unwrap().sel = at;
    for _ in 0..2 {
        app.handle(&Event::Key(Key::Down), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(app.ext_id(&cx).as_deref(), Some("claude-code"));
    assert_eq!(app.ext_model_of(&cx, "claude-code"), "sonnet");
    let footer = app.ext_footer(&cx).unwrap();
    assert!(footer.starts_with("agent: Claude Code · Sonnet · new session"), "{footer}");
    // the list opens on that model; the agent line alone keeps the model
    open_list(&mut app, &cx);
    assert_eq!(app.models.as_ref().unwrap().sel, row_of(&app, "agent:claude-code", "sonnet"));
    let head = row_of(&app, "agent:claude-code", "");
    app.handle(&Event::Row(head), &cx);
    assert_eq!(app.ext_model_of(&cx, "claude-code"), "sonnet");
    // Default is a choice too: no model flag
    open_list(&mut app, &cx);
    let d = row_of(&app, "agent:claude-code", "default");
    app.handle(&Event::Row(d), &cx);
    assert_eq!(app.ext_model_of(&cx, "claude-code"), "");
    assert!(app.ext_footer(&cx).unwrap().contains("· Default ·"));
    // the same list for Copilot: only its default is shipped, and a custom id line
    open_list(&mut app, &cx);
    assert_eq!(row_of(&app, "agent:copilot", "default"), row_of(&app, "agent:copilot", "") + 1);
}

#[test]
fn the_list_with_models_fits_and_scrolls_at_small_sizes_and_shows_what_the_cli_said() {
    let (mut app, cx) = with_models("chatext8");
    cx.services.ext_status.set("claude-code", status(true, Auth::SignedIn("claude.ai".into())));
    let mut max = extagent::ModelInfo::new("max-model", "Max only", "most capable", extagent::ModelSource::Cli);
    max.available = Some(false);
    cx.services.ext_status.set_models("claude-code", vec![extagent::ModelInfo::new("fast-model", "Fast", "cheap", extagent::ModelSource::Cli), max]);
    for (w, h) in [(1280u32, 720u32), (800, 480)] {
        open_list(&mut app, &cx);
        let n = app.models.as_ref().unwrap().items.len();
        for end in [false, true] {
            app.models.as_mut().unwrap().sel = if end { n - 1 } else { 0 };
            let g = render_with(&mut app, &cx, w, h);
            let rows: Vec<_> = g.hits.iter().filter(|x| x.label == "model").collect();
            assert!(rows.len() >= 3, "{} rows at {w}x{h}", rows.len());
            for r in &rows {
                assert!(r.rect.x >= 0.0 && r.rect.y >= 0.0 && r.rect.x + r.rect.w <= w as f32 && r.rect.y + r.rect.h <= h as f32, "{:?} at {w}x{h}", r.rect);
            }
            assert!(rows.windows(2).all(|p| p[0].rect.y + p[0].rect.h <= p[1].rect.y + 0.5), "no overlap at {w}x{h}");
            if end {
                let last = rows.last().unwrap();
                assert!(matches!(last.ev, Event::Row(i) if i == n - 1), "the list scrolled to the last line at {w}x{h}");
            }
        }
        app.handle(&Event::Key(Key::Esc), &cx);
    }
    // the CLI's own list replaced the built-in one
    open_list(&mut app, &cx);
    let ids: Vec<String> = app.models.as_ref().unwrap().rows.iter().filter_map(|r| if let MRow::Model(a, m) = r { (a == "claude-code").then(|| m.id.clone()) } else { None }).collect();
    assert_eq!(ids, ["fast-model", "max-model"]);
}

#[test]
fn a_custom_model_id_is_typed_in_the_list_validated_and_remembered_for_the_agent() {
    let (mut app, cx) = with_models("chatext9");
    cx.services.ext_status.set("claude-code", status(true, Auth::Unknown));
    open_list(&mut app, &cx);
    let c = row_of(&app, "agent:claude-code", "custom model id");
    app.handle(&Event::Row(c), &cx);
    assert!(app.models.as_ref().is_some_and(|m| m.custom.is_some()), "the id is typed in the list");
    let g = render_with(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().filter(|h| h.label == "model").count() >= 3);
    // shell characters are refused and the list stays open
    for c in "x; rm -rf /".chars() {
        app.handle(&Event::Key(Key::Char(c)), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    assert!(app.models.is_some() && app.error.as_deref().is_some_and(|e| e.contains("letters, digits")), "{:?}", app.error);
    assert_eq!(app.ext_id(&cx), None);
    for _ in 0..11 {
        app.handle(&Event::Key(Key::Backspace), &cx);
    }
    for c in "claude-opus-5-5".chars() {
        app.handle(&Event::Key(Key::Char(c)), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    assert!(app.models.is_none());
    assert_eq!(app.ext_id(&cx).as_deref(), Some("claude-code"));
    assert_eq!(app.ext_model_of(&cx, "claude-code"), "claude-opus-5-5");
    assert!(app.ext_footer(&cx).unwrap().contains("· claude-opus-5-5 ·"));
    // remembered for the agent: it is a line of the list next time
    assert_eq!(cx.services.settings.lock().unwrap().get().agents.claude.custom_models, ["claude-opus-5-5"]);
    open_list(&mut app, &cx);
    assert!(app.models.as_ref().unwrap().items.iter().any(|(p, m)| p == "agent:claude-code" && m == "claude-opus-5-5"));
    // Esc leaves the typing without choosing
    let c = row_of(&app, "agent:claude-code", "custom model id");
    app.handle(&Event::Row(c), &cx);
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(app.models.as_ref().is_some_and(|m| m.custom.is_none()));
}

#[test]
fn the_model_is_remembered_per_workspace_and_sent_to_the_cli_as_model() {
    let (app, cx) = with_models("chatext10");
    let root = std::env::temp_dir().join(format!("iyakki-chatext10-ws-{}", std::process::id()));
    let bin = std::env::temp_dir().join(format!("iyakki-chatext10-bin-{}", std::process::id()));
    for d in [&root, &bin] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(root.join("FEATURES.md"), "# Features\n").unwrap();
    let script = bin.join("claude");
    std::fs::write(&script, format!("#!/bin/sh\ncase \"$1\" in --version) echo 2.1.0; exit 0;; auth) echo '{{\"loggedIn\":true}}'; exit 0;; esac\nfor a in \"$@\"; do echo \"$a\"; done > {d}/args.txt\ncat <<'JSONL'\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"fine\",\"session_id\":\"s\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}\nJSONL\n", d = bin.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.settings.lock().unwrap().update(|s| s.agents.claude.command = script.display().to_string()).unwrap();
    cx.services.ext_status.set("claude-code", status(true, Auth::Unknown));
    let scope = ProjectScope { name: "demo".into(), root: root.canonicalize().unwrap(), key: "ws_00000000000000bb".into(), features: root.join("FEATURES.md"), cluster: None };
    let mut app = app.with_project(scope.clone());
    open_list(&mut app, &cx);
    let r = row_of(&app, "agent:claude-code", "opus");
    app.handle(&Event::Row(r), &cx);
    assert_eq!(std::fs::read_to_string(root.join(".iyakki/agent")).unwrap(), "claude-code opus", "the agent and its model are remembered for the workspace");
    assert_eq!(cx.services.settings.lock().unwrap().get().agents.claude.model, "", "not for every chat of the system");
    // a new chat of the workspace starts with both
    let mut fresh = ChatApp::new().with_project(scope.clone());
    assert_eq!(fresh.ext_id(&cx).as_deref(), Some("claude-code"));
    assert_eq!(fresh.ext_model_of(&cx, "claude-code"), "opus");
    // and the run passes it
    app.agent = true;
    app.start_run(&cx, "hello".into());
    for _ in 0..600 {
        app.poll_run(&cx);
        if app.run.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let a = std::fs::read_to_string(bin.join("args.txt")).unwrap();
    assert!(a.contains("--model\nopus\n"), "{a}");
    // Default: no flag, and the file keeps only the agent
    open_list(&mut app, &cx);
    let d = row_of(&app, "agent:claude-code", "default");
    app.handle(&Event::Row(d), &cx);
    assert_eq!(std::fs::read_to_string(root.join(".iyakki/agent")).unwrap(), "claude-code");
    let _ = (std::fs::remove_dir_all(&root), std::fs::remove_dir_all(&bin));
}

#[test]
fn an_installed_agent_that_is_not_signed_in_says_sign_in_first_with_a_login_button_in_the_model_list() {
    let (mut app, cx) = with_models("chatext-login");
    cx.services.ext_status.set("claude-code", status(true, Auth::SignedOut));
    cx.services.ext_status.set("copilot", status(true, Auth::SignedIn("octocat".into())));
    app.handle(&Event::Key(Key::Ctrl('p')), &cx);
    let g = render_with(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB) && h.label == "Login"), "Login next to Claude Code");
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB + 1)), "none for the signed-in Copilot");
    // the same state in words: the vendor-specific help starts with "Sign in first" and names the button
    let help = cx.services.agents_cfg().agent("claude-code").unwrap().login_help();
    assert!(help.starts_with("Sign in first") && help.contains("Login") && help.contains("auth login"), "{help}");
}

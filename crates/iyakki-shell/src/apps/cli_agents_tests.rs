//! The CLI tile's Claude Code and GitHub Copilot entries: the install is shown and asked first, runs only the npm command in
//! the tile's own folder, tells Settings → Agents at once, and the sign-in command is put in the terminal without being run.

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::apps::tests::cx_with_backend;

struct NoStream;
impl iyakki_agent::iyakki_proxy::Streamer for NoStream {
    fn open(&self, url: &str, _h: &[(String, String)]) -> Result<iyakki_agent::iyakki_proxy::StreamHead, String> {
        Err(format!("this test never downloads ({url})"))
    }
}

fn script(p: &std::path::Path, body: &str) {
    std::fs::write(p, body).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Stand-in node and npm in `d`; npm "installs" a fake `claude` (answers `--version` and `auth status`) into its prefix.
fn stand_ins(d: &std::path::Path, node_ok: bool) -> (String, String, std::path::PathBuf) {
    let seen = d.join("npm-seen.txt");
    script(&d.join("node"), "#!/bin/sh\necho v22.1.0\n");
    script(
        &d.join("npm"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 10.0.0; exit 0; fi\necho \"ARGS $*\" > {seen}\necho \"PREFIX=$npm_config_prefix\" >> {seen}\nmkdir -p \"$npm_config_prefix/bin\"\ncat > \"$npm_config_prefix/bin/claude\" <<'EOS'\n#!/bin/sh\ncase \"$1\" in\n  --version) echo \"2.1.7 (Claude Code)\";;\n  auth) echo '{{\"loggedIn\":false,\"authMethod\":\"none\"}}'; exit 1;;\nesac\nEOS\nchmod +x \"$npm_config_prefix/bin/claude\"\n",
            seen = seen.display()
        ),
    );
    let node = if node_ok { d.join("node").display().to_string() } else { "/nonexistent/node".into() };
    (d.join("npm").display().to_string(), node, seen)
}

fn draw(app: &mut CliApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
    g
}

fn pick(app: &mut CliApp, cx: &AppCx, id: &str) {
    app.handle(&Event::Tab(1), cx);
    let i = CATALOG.iter().position(|d| d.id == id).unwrap();
    app.handle(&Event::Row(i), cx);
}

fn wait_install(cx: &AppCx, id: &str) {
    for _ in 0..500 {
        if cx.services.clis.job(id).is_some_and(|j| j.finished || j.error.is_some()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("install did not finish");
}

#[test]
fn claude_code_is_installed_after_a_confirmation_that_shows_the_command_and_settings_agents_sees_it_at_once() {
    let (cx, d) = cx_with_backend("cliagent1", iyakki_agent::chat::Unconfigured);
    let tools = d.join("stand-ins");
    std::fs::create_dir_all(&tools).unwrap();
    let (npm, node, seen) = stand_ins(&tools, true);
    cx.services.set_streamer(std::sync::Arc::new(NoStream));
    cx.services.clis.set_npm(&npm, &node);
    let mut app = CliApp::new(&cx);
    pick(&mut app, &cx, "claude-code");
    // not installed: no sign-in buttons yet; Settings → Agents says not installed or not found
    let g = draw(&mut app, &cx, 1100, 700);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(10)) && !g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB)));
    // asking shows the exact command; declining runs nothing
    app.handle(&Event::Tab(10), &cx);
    assert!(matches!(app.mode, Mode::ConfirmInstall("claude-code")));
    let plan = app.plan.clone().unwrap();
    assert!(plan.contains("`npm install -g @anthropic-ai/claude-code`") && plan.contains("no sudo") && plan.contains("npm_config_prefix") && plan.contains("downloads.claude.ai"), "{plan}");
    let g = draw(&mut app, &cx, 1100, 700);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('y'))), "the confirmation has an Install button");
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(app.mode, Mode::Browse) && !seen.exists() && cx.services.clis.installed().is_empty(), "declined: nothing ran");
    // confirming installs; the audit row is written
    app.handle(&Event::Tab(10), &cx);
    app.handle(&Event::Key(Key::Char('y')), &cx);
    wait_install(&cx, "claude-code");
    let rec = std::fs::read_to_string(&seen).unwrap();
    assert!(rec.starts_with("ARGS install -g @anthropic-ai/claude-code\n"), "{rec}");
    assert!(rec.contains(&cx.services.clis.npm_prefix("claude-code").display().to_string()));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("cli.install")));
    // Settings → Agents and the model list learn about it without a restart or a press of Detect
    for _ in 0..500 {
        if cx.services.ext_status.get("claude-code").is_some_and(|s| s.installed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let st = cx.services.ext_status.get("claude-code").expect("detected after the install");
    assert!(st.installed && st.version.contains("2.1.7"), "{st:?}");
    assert_eq!(st.chip, "sign in", "installed but not signed in: {st:?}");
    let entries = iyakki_agent::extagent::entries(&cx.services.agents_cfg(), &cx.services.ext_status);
    let e = entries.iter().find(|e| e.id == "claude-code").unwrap();
    assert!(!e.ready && e.chip == "sign in" && e.fix.contains("auth login"), "{e:?}");
    // the same state is what the tile shows, with the sign-in buttons
    let g = draw(&mut app, &cx, 1100, 700);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(crate::agent_login::LOGIN_TAB)) && g.hits.iter().any(|h| h.ev == Event::Tab(17)) && g.hits.iter().any(|h| h.ev == Event::Tab(11)), "Login, Status and Remove appear");
    // the chat can run it from the tile's folder with no command configured
    assert!(cx.services.agents_cfg().agent("claude-code").unwrap().program().ends_with("/cli/bin/claude"));
}

#[test]
fn sign_in_asks_the_shell_for_the_login_dialog_and_status_asks_the_cli() {
    let (cx, d) = cx_with_backend("cliagent2", iyakki_agent::chat::Unconfigured);
    let tools = d.join("stand-ins");
    std::fs::create_dir_all(&tools).unwrap();
    let (npm, node, _) = stand_ins(&tools, true);
    cx.services.set_streamer(std::sync::Arc::new(NoStream));
    cx.services.clis.set_npm(&npm, &node);
    let mut app = CliApp::new(&cx);
    pick(&mut app, &cx, "claude-code");
    // before it is installed: asks to install first
    app.handle(&Event::Tab(16), &cx);
    assert!(app.status_text().unwrap().contains("Install Claude Code first") && app.tab() == Tab::Clis);
    app.handle(&Event::Tab(10), &cx);
    app.handle(&Event::Key(Key::Char('y')), &cx);
    wait_install(&cx, "claude-code");
    let before = app.output_lines().len();
    app.handle(&Event::Tab(16), &cx);
    assert_eq!(crate::agent_login::take_request(), Some(("claude-code", false)), "the sign-in dialog is asked for (the shell opens it)");
    assert!(app.input.value.is_empty() && app.output_lines().len() == before, "nothing is typed or run in the terminal");
    assert!(!cx.audit.recent(20).iter().any(|l| l.contains("cli.exec")), "no command ran");
    // the status check asks the CLI's own `auth status` (the stand-in says: not signed in)
    app.handle(&Event::Tab(1), &cx);
    app.handle(&Event::Tab(17), &cx);
    assert!(app.status_text().unwrap().contains("Checking"));
    for _ in 0..500 {
        if cx.services.ext_status.get("claude-code").is_some_and(|s| s.auth == iyakki_agent::extagent::Auth::SignedOut) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(cx.services.ext_status.get("claude-code").unwrap().auth, iyakki_agent::extagent::Auth::SignedOut);
    let g = draw(&mut app, &cx, 1100, 700);
    assert!(g.hits.len() > 5);
}

#[test]
fn copilot_has_its_own_entry_and_says_what_is_unverified() {
    let (cx, d) = cx_with_backend("cliagent3", iyakki_agent::chat::Unconfigured);
    let tools = d.join("stand-ins");
    std::fs::create_dir_all(&tools).unwrap();
    let (npm, node, seen) = stand_ins(&tools, true);
    cx.services.set_streamer(std::sync::Arc::new(NoStream));
    cx.services.clis.set_npm(&npm, &node);
    let mut app = CliApp::new(&cx);
    pick(&mut app, &cx, "github-copilot");
    app.handle(&Event::Tab(10), &cx);
    let plan = app.plan.clone().unwrap();
    assert!(plan.contains("SHA256SUMS.txt") && plan.contains("no Node.js") && !plan.contains("npm install"), "{plan}");
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(!seen.exists());
    let d_ = clis::def("github-copilot").unwrap();
    assert!(d_.note.contains("SHA256SUMS.txt") && d_.bins[0].0 == "copilot");
}

#[test]
fn without_node_the_install_says_node_comes_first_and_when_that_fails_nothing_is_changed() {
    let (cx, d) = cx_with_backend("cliagent4", iyakki_agent::chat::Unconfigured);
    let tools = d.join("stand-ins");
    std::fs::create_dir_all(&tools).unwrap();
    let (npm, node, seen) = stand_ins(&tools, false);
    cx.services.set_streamer(std::sync::Arc::new(NoStream));
    cx.services.clis.set_npm(&npm, &node);
    let mut app = CliApp::new(&cx);
    // an npm tool (Claude Code and Copilot are native builds and need no Node.js)
    pick(&mut app, &cx, "codex");
    app.handle(&Event::Tab(10), &cx);
    assert!(app.plan.clone().unwrap().contains("Node.js is not on this machine"), "the confirmation already says it");
    draw(&mut app, &cx, 1100, 700);
    app.handle(&Event::Key(Key::Char('y')), &cx);
    // Node.js is fetched first; with no network the job fails with that reason and nothing is installed
    wait_install(&cx, "codex");
    let e = cx.services.clis.job("codex").unwrap().error.unwrap();
    assert!(e.contains("Node.js is needed first for OpenAI Codex CLI"), "{e}");
    assert!(!seen.exists() && cx.services.clis.installed().is_empty());
}

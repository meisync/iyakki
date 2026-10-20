//! The Login button, the chip and the sign-in dialog, with a stand-in `claude` that prints an address, waits for a pasted code
//! and answers `auth status` from a file. No vendor is ever called and no real home folder is touched.

use super::*;
use crate::apps::cli::CliApp;
use crate::apps::tests::{cx_with_backend, temp_dir};
use crate::apps::{App, AppCx};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

const URL: &str = "https://claude.example.test/oauth/authorize?code=true&client_id=00000000-0000-0000-0000-000000000000&response_type=code&scope=user%3Ainference&code_challenge=abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG&state=zyxwvutsrqponmlkjihgfedcba9876543210";
const TOKEN: &str = "sk-ant-oat01-ZZZZYYYYXXXXWWWWVVVVUUUUTTTT";

/// A `claude` stand-in: `auth status` counts its calls and answers from `state/signedin`; `auth login` prints the address in colour,
/// waits for the code and signs in on "good-code"; `auth logout` signs out.
fn fake_claude(dir: &std::path::Path) -> std::path::PathBuf {
    let st = dir.join("state");
    std::fs::create_dir_all(&st).unwrap();
    let body = format!(
        r#"#!/bin/sh
S='{st}'
case "$1 $2" in
  "auth status") echo x >> "$S/status-calls"
    if [ -f "$S/signedin" ]; then echo '{{"loggedIn":true,"authMethod":"claude.ai","email":"me@example.test"}}'; exit 0; fi
    echo '{{"loggedIn":false,"authMethod":"none"}}'; exit 1;;
  "auth login") echo "$*" > "$S/login-args"
    printf '\033[1mOpening your browser...\033[0m\r\n\033[36m%s\033[0m\r\nPaste code here if prompted > ' '{URL}'
    read code
    if [ "$code" = "good-code" ]; then touch "$S/signedin"; printf '\033[32mLogin successful\033[0m\r\n'; exit 0; fi
    echo "Login failed: the code was refused ({TOKEN})"; exit 1;;
  "auth logout") rm -f "$S/signedin"; echo "Logged out"; exit 0;;
esac
echo "2.1.7 (Claude Code)"
"#,
        st = st.display()
    );
    let p = dir.join("claude-fake");
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn setup(tag: &str) -> (AppCx, std::path::PathBuf, std::path::PathBuf) {
    let (cx, d) = cx_with_backend(tag, iyakki_agent::chat::Unconfigured);
    let tools = temp_dir(&format!("{tag}-tools"));
    let script = fake_claude(&tools);
    iyakki_agent::clis::write_fake_installed(&cx.services.clis, "claude-code", "#!/bin/sh\nexit 0\n");
    // the program the agent code looks for is `claude` in the tools folder
    std::os::unix::fs::symlink(&script, cx.services.clis.bin_dir().join("claude")).unwrap();
    (cx, d, tools)
}

fn settle_status(cx: &AppCx, want: impl Fn(&State) -> bool) -> State {
    let t0 = Instant::now();
    loop {
        let s = state(cx, extagent::CLAUDE_CODE, true);
        if want(&s) {
            return s;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "status stayed {s:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pump(m: &mut LoginModal, cx: &AppCx, what: &str, done: impl Fn(&LoginModal) -> bool) {
    let t0 = Instant::now();
    while !done(m) {
        m.tick(cx);
        assert!(t0.elapsed() < Duration::from_secs(20), "timed out waiting for {what}: {:?}", m.snapshot());
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn draw(m: &mut LoginModal) -> Gfx {
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(0, 0, 0));
    m.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, 3);
    g
}

fn audit_text(cx: &AppCx) -> String {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default()
}

fn type_text(m: &mut LoginModal, cx: &AppCx, t: &str) {
    for c in t.chars() {
        m.handle(&Event::Key(Key::Char(c)), cx);
    }
}

#[test]
fn the_login_button_shows_only_for_an_installed_tool_with_the_account_in_the_chip() {
    let (cx, _d, tools) = setup("alog1");
    let st = tools.join("state");
    let mut app = CliApp::new(&cx);
    app.handle(&Event::Tab(1), &cx);
    let pick = |app: &mut CliApp, id: &str| {
        let i = iyakki_agent::clis::CATALOG.iter().position(|d| d.id == id).unwrap();
        app.handle(&Event::Row(i), &cx);
    };
    let draw_app = |app: &mut CliApp| {
        let mut g = Gfx::new(1100, 700);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1100.0, 628.0), 1.0, &cx, 0);
        g
    };
    // Copilot is not installed: no button; Claude Code is
    pick(&mut app, "github-copilot");
    let g = draw_app(&mut app);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(LOGIN_TAB + 1)), "no Login for a tool that is not installed");
    pick(&mut app, "claude-code");
    let g = draw_app(&mut app);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(LOGIN_TAB)), "Login for the installed Claude Code");
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(LOGOUT_TAB)), "no Logout while nobody is signed in");
    assert_eq!(settle_status(&cx, |s| *s == State::SignedOut), State::SignedOut);
    assert_eq!(chip(&State::SignedOut), ("not signed in".to_string(), ERR));
    // signed in: the chip names the account and Logout appears
    std::fs::write(st.join("signedin"), "").unwrap();
    cx.services.ext_status.refresh_one(cx.services.agents_cfg(), extagent::CLAUDE_CODE, None);
    let s = settle_status(&cx, |s| matches!(s, State::SignedIn(_)));
    assert_eq!(chip(&s).0, "signed in as me@example.test, claude.ai");
    let g = draw_app(&mut app);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(LOGIN_TAB)) && g.hits.iter().any(|h| h.ev == Event::Tab(LOGOUT_TAB)));
    // the tab numbers mean the right agent
    assert_eq!(agent_of_tab(LOGIN_TAB), Some((extagent::CLAUDE_CODE, false)));
    assert_eq!(agent_of_tab(LOGOUT_TAB + 1), Some((extagent::COPILOT, true)));
    assert_eq!(agent_of_tab(LOGIN_TAB + 5), None);
}

#[test]
fn the_status_is_remembered_not_asked_again_on_every_frame() {
    let (cx, _d, tools) = setup("alog2");
    let calls = || std::fs::read_to_string(tools.join("state/status-calls")).map_or(0, |t| t.lines().count());
    settle_status(&cx, |s| *s == State::SignedOut);
    let n = calls();
    assert!(n >= 1);
    for _ in 0..50 {
        state(&cx, extagent::CLAUDE_CODE, true);
        state(&cx, extagent::COPILOT, false);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(calls(), n, "drawing 50 frames asked `auth status` no more often");
}

#[test]
fn claude_login_walks_through_choice_address_qr_paste_and_success_with_only_the_fact_audited() {
    let (cx, d, tools) = setup("alog3");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    settle_status(&cx, |s| *s == State::SignedOut);
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    // the choice: subscription is the default; the picture offers all three
    let g = draw(&mut m);
    for i in 0..3 {
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_PICK + i)), "method {i}");
    }
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_START)));
    m.handle(&Event::Key(Key::Down), &cx);
    m.handle(&Event::Key(Key::Up), &cx);
    m.handle(&Event::Tab(T_START), &cx);
    pump(&mut m, &cx, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some() && s.wants_input));
    let sn = m.snapshot().unwrap().clone();
    assert_eq!(sn.url.as_deref(), Some(URL));
    assert_eq!(std::fs::read_to_string(tools.join("state/login-args")).unwrap().trim(), "auth login --claudeai", "the subscription option");
    // the screen: address, QR code (a big one, drawn), the code field and a way to copy
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_COPY_URL)) && g.hits.iter().any(|h| h.ev == Event::Tab(T_SEND)) && g.hits.iter().any(|h| h.ev == Event::Tab(T_CANCEL)));
    assert!(matches!(&m.qr, Some((u, Some(q))) if u == URL && q.size >= 45));
    // the paste goes to the program only
    type_text(&mut m, &cx, "good-code");
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    assert_eq!(m.snapshot().unwrap().phase, Phase::Succeeded);
    assert!(m.field.value.is_empty(), "the field was emptied");
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.label == "Close"));
    // the chip follows the new state
    let s = settle_status(&cx, |s| matches!(s, State::SignedIn(_)));
    assert_eq!(chip(&s).0, "signed in as me@example.test, claude.ai");
    // the audit holds the two facts and nothing of the flow
    let a = audit_text(&cx);
    assert!(a.contains("agent.login.start") && a.contains("agent.login.finish") && a.contains("claude-code ok"), "{a}");
    for secret in ["good-code", "oauth", "claude.example", "code_challenge", TOKEN, "abcdefghijklmnop"] {
        assert!(!a.contains(secret), "the audit must not hold {secret}: {a}");
    }
    assert!(matches!(m.handle(&Event::Tab(T_CANCEL), &cx), Out::Close), "Close closes");
}

#[test]
fn a_refused_code_shows_the_vendors_message_without_the_token_and_try_again_works() {
    let (cx, d, _tools) = setup("alog4");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    m.handle(&Event::Tab(T_PICK + 1), &cx); // Console
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the prompt", |m| m.snapshot().is_some_and(|s| s.wants_input));
    type_text(&mut m, &cx, "wrong");
    m.handle(&Event::Tab(T_SEND), &cx);
    pump(&mut m, &cx, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    let Phase::Failed(why) = m.snapshot().unwrap().phase.clone() else { panic!("{:?}", m.snapshot()) };
    assert!(why.contains("the code was refused") && !why.contains("sk-ant") && !why.contains(TOKEN), "{why}");
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_AGAIN)) && g.hits.iter().any(|h| h.label == "Close"));
    let a = audit_text(&cx);
    assert!(a.contains("claude-code failed") && !a.contains(TOKEN) && !a.contains("wrong") && !a.contains("refused"), "{a}");
    // Try again goes back to the choice
    m.handle(&Event::Tab(T_AGAIN), &cx);
    assert!(draw(&mut m).hits.iter().any(|h| h.ev == Event::Tab(T_START)));
}

#[test]
fn cancel_stops_the_program_and_is_audited_and_leaving_with_escape_does_the_same() {
    let (cx, d, _tools) = setup("alog5");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some()));
    assert!(m.running());
    assert!(matches!(m.handle(&Event::Key(Key::Esc), &cx), Out::Stay), "the dialog waits until the program is gone");
    pump(&mut m, &cx, "the stop", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    assert_eq!(m.snapshot().unwrap().phase, Phase::Cancelled);
    assert!(audit_text(&cx).contains("claude-code cancelled"));
    assert!(matches!(m.handle(&Event::Key(Key::Esc), &cx), Out::Close));
}

#[test]
fn logout_runs_the_vendors_command_and_the_chip_goes_back() {
    let (cx, d, tools) = setup("alog6");
    std::fs::write(tools.join("state/signedin"), "").unwrap();
    cx.services.ext_status.refresh_one(cx.services.agents_cfg(), extagent::CLAUDE_CODE, None);
    settle_status(&cx, |s| matches!(s, State::SignedIn(_)));
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    assert!(sign_out_in(&cx, extagent::CLAUDE_CODE, &home).unwrap().contains("Signed out"));
    settle_status(&cx, |s| *s == State::SignedOut);
    assert!(audit_text(&cx).contains("agent.logout") && audit_text(&cx).contains("claude-code ok"));
    // Copilot signs out by removing its token from the vault and the file: with the vault locked it says so
    let e = sign_out_in(&cx, extagent::COPILOT, &home).unwrap_err();
    assert!(e.contains("vault is locked"), "{e}");
}

#[test]
fn a_device_code_flow_shows_the_code_big_and_needs_no_field() {
    let (cx, d, _tools) = setup("alog7");
    let prog = d.join("copilot");
    std::fs::write(&prog, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '0.0.400'; exit 0; fi\nprintf 'First copy your one-time code: \\033[1mA1B2-C3D4\\033[0m\\r\\nThen open https://github.com/login/device\\r\\nWaiting for authorization...\\r\\n'\nsleep 1\nexit 0\n").unwrap();
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.settings.lock().unwrap().update(|s| s.agents.copilot.command = prog.display().to_string()).unwrap();
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::COPILOT).with_home(&home);
    assert!(draw(&mut m).hits.iter().all(|h| h.ev != Event::Tab(T_START)), "Copilot has no choice to make");
    m.start(&cx);
    pump(&mut m, &cx, "the code", |m| m.snapshot().is_some_and(|s| s.code.is_some() && s.url.is_some()));
    let sn = m.snapshot().unwrap().clone();
    assert_eq!((sn.code.as_deref(), sn.url.as_deref(), sn.wants_input), (Some("A1B2-C3D4"), Some("https://github.com/login/device"), false));
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_COPY_CODE)) && g.hits.iter().any(|h| h.ev == Event::Tab(T_COPY_URL)) && !g.hits.iter().any(|h| h.ev == Event::Tab(T_SEND)));
    pump(&mut m, &cx, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    assert_eq!(m.snapshot().unwrap().phase, Phase::Succeeded);
    let a = audit_text(&cx);
    assert!(a.contains("github-copilot ok") || a.contains("copilot ok"), "{a}");
    assert!(!a.contains("A1B2") && !a.contains("github.com/login"), "{a}");
}

#[test]
fn the_dialog_draws_at_small_and_large_sizes() {
    let (cx, d, _tools) = setup("alog8");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the address", |m| m.snapshot().is_some_and(|s| s.url.is_some() && s.wants_input));
    m.set_relay_for_demo("https://192.168.1.5:8443/login/0123456789abcdef0123456789abcdef");
    for (w, h, sc) in [(800u32, 600u32, 0.7f32), (1280, 720, 1.0), (1920, 1080, 1.5)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        m.render(&mut g, R::new(0.0, 0.0, w as f32, h as f32), sc, 5);
        assert!(g.hits.iter().any(|x| x.ev == Event::Tab(T_CANCEL)), "{w}x{h}");
        for hit in g.hits.iter().filter(|x| x.label != "backdrop") {
            assert!(hit.rect.x >= -1.0 && hit.rect.y >= -1.0 && hit.rect.x + hit.rect.w <= w as f32 + 1.0 && hit.rect.y + hit.rect.h <= h as f32 + 1.0, "{} is off screen at {w}x{h}: {:?}", hit.label, hit.rect);
        }
    }
    m.handle(&Event::Key(Key::Esc), &cx);
}

const CODE: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789#st-ate_0123456789ABCDEFGHIJKLMNOPQ";

/// A `claude` that wants the long code: it signs in only on exactly `CODE`, and answers a wrong one with a message and asks again.
fn picky_claude(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("claude-picky");
    let st = dir.join("state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(&p, format!("#!/bin/sh\ncase \"$1 $2\" in\n \"auth status\") [ -f '{st}/signedin' ] && echo '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\"}}' && exit 0; echo '{{\"loggedIn\":false,\"authMethod\":\"none\"}}'; exit 1;;\n \"auth login\")\n  printf 'Open: %s\\r\\nPaste code here if prompted > ' '{URL}'\n  while read c; do\n   echo \"$c\" >> '{st}/received'\n   if [ \"$c\" = '{CODE}' ]; then touch '{st}/signedin'; echo 'Login successful'; exit 0; fi\n   sleep 1; printf 'Invalid code. Please try again.\\r\\nPaste code here if prompted > '\n  done;;\nesac\necho '2.1.7'\n", st = st.display())).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn setup_picky(tag: &str) -> (AppCx, std::path::PathBuf, std::path::PathBuf) {
    let (cx, d) = cx_with_backend(tag, iyakki_agent::chat::Unconfigured);
    let tools = temp_dir(&format!("{tag}-tools"));
    let script = picky_claude(&tools);
    iyakki_agent::clis::write_fake_installed(&cx.services.clis, "claude-code", "#!/bin/sh\nexit 0\n");
    std::os::unix::fs::symlink(&script, cx.services.clis.bin_dir().join("claude")).unwrap();
    (cx, d, tools)
}

fn hit_label(g: &Gfx, ev: Event) -> Option<String> {
    g.hits.iter().find(|h| h.ev == ev).map(|h| h.label.clone())
}

#[test]
fn the_field_checks_the_code_before_sending_cleans_it_and_a_refusal_keeps_the_dialog_for_a_retry() {
    let (cx, d, tools) = setup_picky("alog9");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the prompt", |m| m.snapshot().is_some_and(|s| s.wants_input));
    // a code with a space is not sent
    type_text(&mut m, &cx, "abc def");
    m.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(m.field_text(), "abc def", "kept for correction");
    assert!(!tools.join("state/received").exists(), "nothing was sent");
    // a wrong but well-formed one is sent and refused; the dialog stays open and asks again
    m.field.set("");
    m.handle(&Event::Paste(format!("  \"{}\"\r\n", "x".repeat(30))), &cx);
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the refusal", |m| m.snapshot().is_some_and(|s| s.note.is_some()));
    let sn = m.snapshot().unwrap().clone();
    assert!(!sn.phase.finished() && sn.wants_input && sn.note.as_deref().unwrap().contains("Invalid code"), "{sn:?}");
    assert_eq!(std::fs::read_to_string(tools.join("state/received")).unwrap().trim(), "x".repeat(30), "quotes and whitespace were cleaned off");
    assert!(m.running());
    // the right one, pasted as the whole redirect address
    m.field.set("");
    m.handle(&Event::Paste(format!("https://platform.claude.example.test/oauth/code/callback?code={}&state={}", CODE.split('#').next().unwrap(), CODE.split('#').nth(1).unwrap()), ), &cx);
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_PASTE)), "a Paste button");
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    assert_eq!(m.snapshot().unwrap().phase, Phase::Succeeded);
    assert!(std::fs::read_to_string(tools.join("state/received")).unwrap().lines().any(|l| l == CODE), "code#state was extracted from the address");
    let a = audit_text(&cx);
    assert!(!a.contains("xxxxxxxx") && !a.contains(&CODE[..30]), "{a}");
}

#[test]
fn the_clipboard_is_offered_in_one_tap_only_for_code_like_text_and_nothing_is_filled_in_alone() {
    let (cx, d, _tools) = setup_picky("alog10");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the prompt", |m| m.snapshot().is_some_and(|s| s.wants_input));
    crate::ui::clipboard::copy("just a sentence the person copied somewhere else entirely");
    m.refresh_clip_offer();
    assert_eq!(m.clip_offer(), None);
    assert!(!draw(&mut m).hits.iter().any(|h| h.ev == Event::Tab(T_PASTE_OFFER)));
    crate::ui::clipboard::copy(CODE);
    m.refresh_clip_offer();
    assert_eq!(m.clip_offer(), Some(CODE.len()));
    let g = draw(&mut m);
    assert_eq!(hit_label(&g, Event::Tab(T_PASTE_OFFER)).as_deref(), Some(&*format!("Paste the copied code ({} characters)", CODE.len())));
    assert_eq!(m.field_text(), "", "nothing is filled in without a tap");
    m.handle(&Event::Tab(T_PASTE_OFFER), &cx);
    assert_eq!(m.field_text(), CODE);
    assert_eq!(m.clip_offer(), None, "the offer goes once it is used");
    m.handle(&Event::Key(Key::Esc), &cx);
}

#[test]
fn the_phone_relay_forwards_the_code_once_and_leaves_no_trace_of_it() {
    let (cx, d, tools) = setup_picky("alog11");
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut m = LoginModal::new(extagent::CLAUDE_CODE).with_home(&home);
    m.handle(&Event::Key(Key::Enter), &cx);
    pump(&mut m, &cx, "the prompt", |m| m.snapshot().is_some_and(|s| s.wants_input));
    // the dialog serves a page for it (the address is made here: a test machine may have no network)
    let token = iyakki_agent::extagent::relay::open("Claude Code");
    m.set_relay_for_demo(&format!("https://192.168.1.5:8443/login/{token}"));
    assert!(m.relay_url().unwrap().ends_with(&token));
    let g = draw(&mut m);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_OPEN)), "Open in browser next to Copy address");
    // the phone sends it: wrong token first, then the right one
    let post = |t: &str, c: &str| iyakki_agent::extagent::relay::handle("POST", &format!("/login/{t}"), Some("application/x-www-form-urlencoded"), format!("code={}", c.replace('#', "%23")).as_bytes(), "192.168.1.9").unwrap().0;
    assert_eq!(post(&"0".repeat(32), CODE), 404);
    assert_eq!(post(&token, CODE), 200);
    pump(&mut m, &cx, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    assert_eq!(m.snapshot().unwrap().phase, Phase::Succeeded);
    assert!(std::fs::read_to_string(tools.join("state/received")).unwrap().lines().any(|l| l == CODE), "forwarded exactly as typed");
    assert!(m.relay_url().is_none(), "the page went with the login");
    assert_eq!(post(&token, CODE), 404, "and its token is dead");
    let a = audit_text(&cx);
    assert!(a.contains("agent.login.relay") && a.contains("used"), "{a}");
    assert!(!a.contains(&CODE[..30]) && !a.contains(&token), "{a}");
}

const GH_TOKEN: &str = "ghu_FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE0002";

/// A `copilot` stand-in: `login --device-code` shows a code, then writes what the real one writes where there is no keychain (the token in
/// plain text in `$HOME/.copilot/config.json`); `--version` answers.
fn fake_copilot(cx: &AppCx, d: &std::path::Path) {
    let p = d.join("copilot-fake");
    std::fs::write(&p, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '0.0.400'; exit 0; fi\nprintf 'First copy your one-time code: A1B2-C3D4\\r\\nThen open https://github.com/login/device\\r\\n'\nsleep 1\nmkdir -p \"$HOME/.copilot\"\nprintf '// User settings belong in settings.json.\\n{{\"theme\":\"dark\",\"authTokens\":{{\"https://github.com:octocat\":\"{GH_TOKEN}\"}},\"loggedInUsers\":[{{\"host\":\"https://github.com\",\"login\":\"octocat\"}}]}}\\n' > \"$HOME/.copilot/config.json\"\nexit 0\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    iyakki_agent::clis::write_fake_installed(&cx.services.clis, "github-copilot", "#!/bin/sh\nexit 0\n");
    std::os::unix::fs::symlink(&p, cx.services.clis.bin_dir().join("copilot")).unwrap();
}

fn copilot_login(cx: &AppCx, home: &std::path::Path) -> LoginModal {
    let mut m = LoginModal::new(extagent::COPILOT).with_home(home);
    m.start(cx);
    pump(&mut m, cx, "the end", |m| m.snapshot().is_some_and(|s| s.phase.finished()));
    assert_eq!(m.snapshot().unwrap().phase, Phase::Succeeded);
    m
}

#[test]
fn a_copilot_login_moves_its_plain_token_into_the_persons_vault_and_hands_it_only_to_the_program_started_for_them() {
    let (cx, d) = cx_with_backend("alog12", iyakki_agent::chat::Unconfigured);
    cx.services.on_login(&cx.session.user, "tester-pass-12", false);
    fake_copilot(&cx, &d);
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let m = copilot_login(&cx, &home);
    // the vault has the account and the token; the plain file keeps the other settings and no token
    let v = cx.services.user_get(&cx.session.user, iyakki_agent::extagent::login::COPILOT_VAULT).unwrap();
    let (login, token) = iyakki_agent::extagent::login::parse_vault_entry(&v).unwrap();
    assert_eq!((login.as_str(), token.as_str()), ("octocat", GH_TOKEN));
    let file = std::fs::read_to_string(home.join(".copilot/config.json")).unwrap();
    assert!(!file.contains("ghu_") && file.contains("\"theme\": \"dark\"") && file.contains("octocat") && file.starts_with("// User settings"), "{file}");
    assert_eq!(std::fs::metadata(home.join(".copilot/config.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(m.after.as_ref().is_some_and(|(t, ok)| *ok && t.contains("in your vault")), "{:?}", m.after);
    // the chip says where the token is
    let s = settle_status_for(&cx, extagent::COPILOT, |s| matches!(s, State::SignedIn(_)));
    assert_eq!(chip(&s).0, "signed in as octocat (token in your vault)");
    // the token reaches the program started for this person as the variable it reads, and nothing else: not this process, not other agents, not other people
    let (var, t) = cx.services.agent_vault_token(&cx.session.user, extagent::COPILOT).unwrap();
    assert_eq!((var, t.as_str()), ("COPILOT_GITHUB_TOKEN", GH_TOKEN));
    assert!(cx.services.agent_vault_token(&cx.session.user, extagent::CLAUDE_CODE).is_none());
    assert!(cx.services.agent_vault_token("someone-else", extagent::COPILOT).is_none());
    for k in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        assert!(std::env::var(k).is_err(), "{k} must not be in this process's own environment");
    }
    let cfg = cx.services.agents_cfg();
    let inv = cfg.agent(extagent::COPILOT).unwrap().invocation(&iyakki_agent::extagent::Plan { workspace: &d, prompt: "hi", system: "", model: "", resume: None, mcp_config: None, key: Some(&t), max_turns: 1, no_tools: false });
    assert!(inv.env.iter().any(|(k, v)| k == "COPILOT_GITHUB_TOKEN" && v == GH_TOKEN) && inv.env.iter().any(|(k, _)| k == "HOME"), "the run gets the token and the person's home");
    assert!(!inv.argv.iter().any(|a| a.contains("ghu_")), "never on the command line");
    let a = audit_text(&cx);
    assert!(a.contains("agent.login.vault") && a.contains("imported") && !a.contains("ghu_") && !a.contains("FAKE"), "{a}");
    // sign out: the vault entry and the account go
    assert!(sign_out_in(&cx, extagent::COPILOT, &home).unwrap().contains("removed"));
    assert!(cx.services.user_get(&cx.session.user, iyakki_agent::extagent::login::COPILOT_VAULT).is_err());
    // (the home the status looks at here is the machine's own, so only "no longer signed in as octocat" can be asserted)
    let s = settle_status_for(&cx, extagent::COPILOT, |s| !matches!(s, State::SignedIn(_)));
    assert!(!chip(&s).0.contains("octocat"), "{s:?}");
    assert!(cx.services.agent_vault_token(&cx.session.user, extagent::COPILOT).is_none());
}

#[test]
fn with_the_vault_locked_the_token_stays_where_it_is_and_the_dialog_says_how_to_unlock() {
    let (cx, d) = cx_with_backend("alog13", iyakki_agent::chat::Unconfigured);
    // no on_login: the personal vault is not open
    fake_copilot(&cx, &d);
    let home = d.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let m = copilot_login(&cx, &home);
    let (t, ok) = m.after.clone().unwrap();
    assert!(!ok && t.contains("vault is locked") && t.contains("Settings → Vault"), "{t}");
    assert!(std::fs::read_to_string(home.join(".copilot/config.json")).unwrap().contains(GH_TOKEN), "left alone, so nothing is lost");
    assert!(cx.services.agent_vault_token(&cx.session.user, extagent::COPILOT).is_none());
    assert!(audit_text(&cx).contains("github-copilot locked") && !audit_text(&cx).contains("ghu_"));
    // the dialog draws the note under "Signed in"
    let mut m = m;
    assert!(draw(&mut m).hits.iter().any(|h| h.label == "Close"));
}

fn settle_status_for(cx: &AppCx, agent: &str, want: impl Fn(&State) -> bool) -> State {
    let t0 = Instant::now();
    cx.services.ext_status.refresh_one(cx.services.agents_cfg(), agent, None);
    loop {
        let s = state(cx, agent, true);
        if want(&s) {
            return s;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "status stayed {s:?}: {:?} / {:?}", cx.services.ext_status.get(agent), { let c = cx.services.agents_cfg(); (c.vault_logins, c.vault_locked, c.env, cx.services.user_get(&cx.session.user, "github-copilot").map(|v| v.len()).map_err(|e| e.to_string())) });
        std::thread::sleep(Duration::from_millis(30));
        if t0.elapsed().as_millis() % 700 < 40 {
            cx.services.ext_status.refresh_one(cx.services.agents_cfg(), agent, None);
        }
    }
}

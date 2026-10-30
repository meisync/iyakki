use super::*;
use std::os::unix::fs::PermissionsExt;
use crate::extagent::{AgentCfg, Auth, ExternalAgent};
use crate::extagent::copilot::copilot_files_auth;

struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Dir {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("iyakki-login-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn script(&self, name: &str, body: &str) -> String {
        let f = self.0.join(name);
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        f.display().to_string()
    }
    fn spec(&self, agent: &str, prog: &str) -> Spec {
        // the real command line shape, with the fake program as the program
        let mut s = spec(agent, prog, Method::Subscription, &self.0).unwrap();
        s.cwd = self.0.clone();
        s
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wait_for(s: &LoginSession, what: &str, f: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let t0 = Instant::now();
    loop {
        let sn = s.snapshot();
        if f(&sn) {
            return sn;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "timed out waiting for {what}: {sn:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Is the process there and not a zombie (read from /proc, no signal sent)?
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|t| t.rsplit(") ").next().is_some_and(|r| !r.starts_with('Z')))
}

const LONG: &str = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&code_challenge=abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG&state=zyxwvutsrqponmlkjihgfedcba9876543210";

#[test]
fn colour_codes_and_wrapped_lines_do_not_break_the_address() {
    let coloured = format!("\x1b[1mOpen this link in your browser:\x1b[0m\r\n\x1b[36m{LONG}\x1b[39m\r\nPaste code here if prompted > ");
    let p = parse(&coloured);
    assert_eq!(p.url.as_deref(), Some(LONG));
    assert!(p.wants_code);
    // the program wrapped the address at 80 columns
    let wrapped: String = LONG.as_bytes().chunks(80).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\r\n");
    let p = parse(&format!("Visit:\r\n{wrapped}\r\n\r\nPaste code here if prompted > "));
    assert_eq!(p.url.as_deref(), Some(LONG), "wrapped address is joined");
    // the sentence after the address is not part of it
    let p = parse(&format!("go to {LONG}\nThen come back"));
    assert_eq!(p.url.as_deref(), Some(LONG));
    let p = parse(&format!("{LONG}\nDone\n"));
    assert_eq!(p.url.as_deref(), Some(LONG), "a short word on the next line is not added");
    // an address inside an OSC 8 hyperlink and a short one
    assert_eq!(parse("\x1b]8;;https://github.com/login/device\x07https://github.com/login/device\x1b]8;;\x07").url.as_deref(), Some("https://github.com/login/device"));
    assert_eq!(parse("open (https://github.com/login/device).").url.as_deref(), Some("https://github.com/login/device"));
    assert_eq!(parse("nothing yet").url, None);
}

#[test]
fn device_codes_are_found_with_their_colours_and_the_last_one_wins() {
    let p = parse("\x1b[1mFirst copy your one-time code:\x1b[0m \x1b[32mA1B2-C3D4\x1b[0m\nThen open https://github.com/login/device\n");
    assert_eq!(p.code.as_deref(), Some("A1B2-C3D4"));
    assert_eq!(p.url.as_deref(), Some("https://github.com/login/device"));
    assert_eq!(parse("code: ABCD-1234\nretry: WXYZ-9876").code.as_deref(), Some("WXYZ-9876"));
    assert_eq!(parse("the well-known HELP-DESK page").code, None);
    assert_eq!(parse("xA1B2-C3D4").code, None, "inside a longer word");
    assert!(find_question("Store the token in plaintext at ~/.copilot/config.json? (y/n)").is_some());
    assert!(find_question("Waiting for authorization...").is_none());
}

#[test]
fn failure_text_has_no_tokens_or_addresses() {
    let out = "Opening https://claude.ai/oauth/authorize?x=1\nerror: invalid grant sk-ant-oat01-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 was refused\n";
    let t = plain_failure(out);
    assert!(t.contains("invalid grant") && t.contains("(removed)") && !t.contains("sk-ant") && !t.contains("claude.ai"), "{t}");
    assert!(plain_failure("getaddrinfo ENOTFOUND api.example\n").contains("network"));
    assert!(plain_failure("").contains("without saying why"));
}

#[test]
fn spec_builds_each_vendors_command_and_pins_the_home_folder() {
    let home = Path::new("/h/alice");
    let c = spec(crate::extagent::CLAUDE_CODE, "claude", Method::Console, home).unwrap();
    assert_eq!(c.argv, ["claude", "auth", "login", "--console"]);
    assert!(c.env.contains(&("HOME".into(), "/h/alice".into())) && c.env.contains(&("CLAUDE_CONFIG_DIR".into(), "/h/alice/.claude".into())) && c.takes_code);
    assert_eq!(spec(crate::extagent::CLAUDE_CODE, "claude", Method::Subscription, home).unwrap().argv[3], "--claudeai");
    assert_eq!(spec(crate::extagent::CLAUDE_CODE, "claude", Method::Sso, home).unwrap().argv[3], "--sso");
    let g = spec(crate::extagent::COPILOT, "/bin/copilot", Method::Subscription, home).unwrap();
    assert_eq!(g.argv, ["/bin/copilot", "login", "--device-code"]);
    assert!(g.env.contains(&("HOME".into(), "/h/alice".into())) && !g.env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR") && !g.takes_code);
    assert!(spec("custom:x", "x", Method::Subscription, home).is_none());
}

const CLAUDE_FAKE: &str = r#"printf '\033[1mOpening browser to sign in...\033[0m\r\n'
printf 'If it does not open, visit:\r\n\033[36m%s\033[0m\r\n' "$FAKE_URL"
{ echo "HOME=$HOME"; echo "CFG=$CLAUDE_CONFIG_DIR"; echo "ARGS=$*"; echo "API=${ANTHROPIC_API_KEY:-none}"; } > "$FAKE_REC"
printf 'Paste code here if prompted > '
read code
echo "$code" > "$FAKE_CODE"
if [ "$code" = "good-code-123" ]; then printf '\033[32mLogin successful\033[0m\r\n'; exit 0; fi
echo "Login failed: the code was not accepted (sk-ant-oat01-AAAABBBBCCCCDDDDEEEEFFFFGGGG)"; exit 1"#;

fn claude_session(d: &Dir) -> (LoginSession, PathBuf, PathBuf) {
    let prog = d.script("claude", CLAUDE_FAKE);
    let (rec, code) = (d.0.join("rec"), d.0.join("code"));
    let mut sp = d.spec(crate::extagent::CLAUDE_CODE, &prog);
    sp.env.push(("FAKE_URL".into(), LONG.into()));
    sp.env.push(("FAKE_REC".into(), rec.display().to_string()));
    sp.env.push(("FAKE_CODE".into(), code.display().to_string()));
    std::env::set_var("ANTHROPIC_API_KEY", "must-not-be-passed");
    (LoginSession::start(&sp), rec, code)
}

#[test]
fn claude_login_shows_the_address_forwards_the_pasted_code_and_succeeds() {
    let d = Dir::new("ok");
    let (s, rec, code) = claude_session(&d);
    let sn = wait_for(&s, "the address", |s| s.url.is_some() && s.wants_input);
    assert_eq!(sn.url.as_deref(), Some(LONG));
    assert_eq!(sn.phase, Phase::Waiting);
    s.send_line("good-code-123");
    let sn = wait_for(&s, "the end", |s| s.phase.finished());
    assert_eq!(sn.phase, Phase::Succeeded, "{sn:?}");
    assert_eq!(std::fs::read_to_string(&code).unwrap().trim(), "good-code-123", "the code reached the program, once");
    let r = std::fs::read_to_string(&rec).unwrap();
    assert!(r.contains(&format!("HOME={}", d.0.display())) && r.contains(&format!("CFG={}/.claude", d.0.display())), "{r}");
    assert!(r.contains("ARGS=auth login --claudeai") && r.contains("API=none"), "{r}");
    // what the program printed back is not echoed by the terminal, and the snapshot never carries the pasted code
    assert!(!format!("{sn:?}").contains("good-code-123"));
}

#[test]
fn a_refused_code_fails_in_plain_words_without_the_token() {
    let d = Dir::new("bad");
    let (s, _, _) = claude_session(&d);
    wait_for(&s, "the prompt", |s| s.wants_input);
    s.send_line("wrong");
    let sn = wait_for(&s, "the end", |s| s.phase.finished());
    let Phase::Failed(why) = &sn.phase else { panic!("{sn:?}") };
    assert!(why.contains("not accepted") && why.contains("(removed)") && !why.contains("sk-ant") && !why.contains("claude.ai"), "{why}");
    assert_eq!(sn.line, *why);
    assert!(!format!("{sn:?}").contains("AAAABBBB"));
}

#[test]
fn cancel_stops_the_program_and_everything_it_started() {
    let d = Dir::new("cancel");
    let prog = d.script("claude", &format!("printf 'visit {LONG}\\r\\n'\nsleep 60 &\necho $! > '{0}/child'\nread x\nsleep 60", d.0.display()));
    let s = LoginSession::start(&d.spec(crate::extagent::CLAUDE_CODE, &prog));
    wait_for(&s, "the address", |s| s.url.is_some());
    let t0 = Instant::now();
    s.cancel();
    let sn = wait_for(&s, "cancelled", |s| s.phase.finished());
    assert_eq!(sn.phase, Phase::Cancelled);
    assert!(t0.elapsed() < Duration::from_secs(10));
    let child: i32 = std::fs::read_to_string(d.0.join("child")).unwrap().trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!alive(child), "the background child was stopped with its group");
}

#[test]
fn a_device_code_flow_needs_no_typing_and_ends_by_itself() {
    let d = Dir::new("device");
    let prog = d.script("copilot", "printf 'To sign in, use a web browser to open \\033[4mhttps://github.com/login/device\\033[0m\\r\\nand enter the code \\033[1mA1B2-C3D4\\033[0m\\r\\nWaiting for authorization...\\r\\n'\nsleep 1\necho \"Signed in as octocat\"\nexit 0");
    let s = LoginSession::start(&d.spec(crate::extagent::COPILOT, &prog));
    let sn = wait_for(&s, "the code", |s| s.code.is_some() && s.url.is_some());
    assert_eq!((sn.code.as_deref(), sn.url.as_deref()), (Some("A1B2-C3D4"), Some("https://github.com/login/device")));
    assert!(!sn.wants_input, "no field for a flow that polls");
    assert_eq!(wait_for(&s, "the end", |s| s.phase.finished()).phase, Phase::Succeeded);
}

#[test]
fn a_question_can_be_answered_and_a_missing_program_fails_clearly() {
    let d = Dir::new("question");
    let prog = d.script("copilot", "printf 'Code A1B2-C3D4 at https://github.com/login/device\\r\\nNo keychain found. Store the token in plaintext? (y/n) '\nread a\n[ \"$a\" = y ] && exit 0\nexit 3");
    let s = LoginSession::start(&d.spec(crate::extagent::COPILOT, &prog));
    let sn = wait_for(&s, "the question", |s| s.question.is_some());
    assert!(sn.wants_input && sn.question.unwrap().contains("plaintext"));
    s.send_line("y");
    assert_eq!(wait_for(&s, "the end", |s| s.phase.finished()).phase, Phase::Succeeded);
    let gone = LoginSession::start(&d.spec(crate::extagent::CLAUDE_CODE, "/nonexistent/claude"));
    let sn = gone.snapshot();
    assert!(matches!(&sn.phase, Phase::Failed(m) if m.contains("not installed")), "{sn:?}");
    let silent = d.script("claude", "exit 2");
    let s = LoginSession::start(&d.spec(crate::extagent::CLAUDE_CODE, &silent));
    assert!(matches!(wait_for(&s, "the end", |s| s.phase.finished()).phase, Phase::Failed(m) if m.contains("without saying why")));
}

#[test]
fn leaving_the_screen_stops_a_running_sign_in() {
    let d = Dir::new("drop");
    let prog = d.script("claude", &format!("echo $$ > '{0}/pid'\nsleep 60", d.0.display()));
    let s = LoginSession::start(&d.spec(crate::extagent::CLAUDE_CODE, &prog));
    let t0 = Instant::now();
    while !d.0.join("pid").exists() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: i32 = std::fs::read_to_string(d.0.join("pid")).unwrap().trim().parse().unwrap();
    drop(s);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!alive(pid));
}

#[test]
fn copilot_files_say_who_is_signed_in_without_reading_a_token() {
    let d = Dir::new("cfiles");
    let none = |_: &str| None;
    let home = d.0.display().to_string();
    assert_eq!(copilot_files_auth(None, Some(&home), &none), Auth::SignedOut, "no ~/.copilot at all");
    std::fs::create_dir_all(d.0.join(".copilot")).unwrap();
    assert_eq!(copilot_files_auth(None, Some(&home), &none), Auth::Unknown, "a folder without config.json: cannot tell");
    std::fs::write(d.0.join(".copilot/config.json"), "// User settings belong in settings.json.\n// This file is managed automatically.\n{\"logged_in_users\":[{\"host\":\"https://github.com\",\"login\":\"octocat\"}],\"last_logged_in_user\":{\"host\":\"https://github.com\",\"login\":\"octocat\"},\"copilot_tokens\":{\"x\":\"ghu_SECRETSECRETSECRETSECRET\"}}").unwrap();
    let a = copilot_files_auth(None, Some(&home), &none);
    assert_eq!(a, Auth::SignedIn("octocat".into()));
    assert!(!format!("{a:?}").contains("SECRET"));
    std::fs::write(d.0.join(".copilot/config.json"), "{\"logged_in_users\":[]}").unwrap();
    assert_eq!(copilot_files_auth(None, Some(&home), &none), Auth::SignedOut);
    std::fs::write(d.0.join(".copilot/config.json"), "{\"theme\":\"dark\"}").unwrap();
    assert_eq!(copilot_files_auth(None, Some(&home), &none), Auth::Unknown, "a shape we do not know is never read as signed out");
    std::fs::write(d.0.join(".copilot/config.json"), "not json").unwrap();
    assert_eq!(copilot_files_auth(None, Some(&home), &none), Auth::Unknown);
    // COPILOT_HOME wins over HOME, and a token variable counts as signed in
    let other = Dir::new("chome");
    std::fs::write(other.0.join("config.json"), "{\"last_logged_in_user\":{\"login\":\"hubot\"}}").unwrap();
    assert_eq!(copilot_files_auth(Some(&other.0.display().to_string()), Some(&home), &none), Auth::SignedIn("hubot".into()));
    let env = |k: &str| (k == "GH_TOKEN").then(|| "ghp_x".to_string());
    assert_eq!(copilot_files_auth(None, Some(&home), &env), Auth::SignedIn("token from $GH_TOKEN".into()));
}

#[test]
fn claude_auth_status_names_the_account_when_it_says_so() {
    let c = crate::extagent::claude_code::ClaudeCode::new(AgentCfg::default());
    let ok = |json: &str| crate::wsexec::Out { code: Some(0), stdout: json.into(), ..Default::default() };
    assert_eq!(c.parse_auth(&ok(r#"{"loggedIn":true,"authMethod":"claude.ai","email":"me@example.test"}"#)), Auth::SignedIn("me@example.test, claude.ai".into()));
    assert_eq!(c.parse_auth(&ok(r#"{"loggedIn":true,"authMethod":"claude.ai"}"#)), Auth::SignedIn("claude.ai".into()));
    assert_eq!(c.parse_auth(&crate::wsexec::Out { code: Some(1), stdout: r#"{"loggedIn":false,"authMethod":"none"}"#.into(), ..Default::default() }), Auth::SignedOut);
}

#[test]
fn logout_runs_claude_auth_logout_and_copilot_has_no_command() {
    let d = Dir::new("logout");
    let prog = d.script("claude", &format!("echo \"$* HOME=$HOME CFG=$CLAUDE_CONFIG_DIR\" > '{0}/out'\nexit 0", d.0.display()));
    assert!(logout(crate::extagent::CLAUDE_CODE, &prog, &d.0).unwrap().contains("Signed out"));
    let out = std::fs::read_to_string(d.0.join("out")).unwrap();
    assert!(out.starts_with("auth logout ") && out.contains(&format!("CFG={}/.claude", d.0.display())), "{out}");
    let bad = d.script("claude2", "echo 'no session'; exit 1");
    assert!(logout(crate::extagent::CLAUDE_CODE, &bad, &d.0).unwrap_err().contains("no session"));
    assert!(logout(crate::extagent::COPILOT, "copilot", &d.0).unwrap_err().contains("/logout"));
}

#[test]
fn a_pasted_code_is_cleaned_extracted_and_checked_before_it_is_sent() {
    let long = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789#st-ate_0123456789ABCDEFGHIJKLMNOPQ";
    let c = check_code(long, true);
    assert_eq!((c.code.as_str(), c.problem.clone(), c.warning.clone(), c.from_url), (long, None, None, false));
    assert_eq!(c.len, long.len());
    // surrounding whitespace, line breaks and quotes
    assert_eq!(check_code(&format!("  \n\"{long}\"\r\n "), true).code, long);
    assert_eq!(check_code(&format!("`{long}`"), true).code, long);
    // a phone wrapped it
    let wrapped = format!("{}\n{}", &long[..50], &long[50..]);
    assert_eq!(check_code(&wrapped, true).code, long);
    // the whole redirect address, state in the query or after #
    let a = check_code("https://platform.claude.com/oauth/code/callback?code=AAAA1111BBBB2222CCCC3333&state=ssssTTTT4444uuuu", true);
    assert_eq!((a.code.as_str(), a.from_url), ("AAAA1111BBBB2222CCCC3333#ssssTTTT4444uuuu", true));
    let b = check_code("https://x.example/cb?code=AAAA1111BBBB2222CCCC3333#frag-state_99", true);
    assert_eq!(b.code, "AAAA1111BBBB2222CCCC3333#frag-state_99");
    assert_eq!(check_code("https://x.example/cb?code=A%23B%2Fc1234567890123456&state=s", false).code, "A#B/c1234567890123456#s");
    assert!(check_code("https://claude.ai/login", true).problem.unwrap().contains("no code"));
    // what blocks and what only warns
    assert!(check_code("", true).problem.is_some());
    assert!(check_code("abc def", true).problem.unwrap().contains("spaces"));
    assert!(check_code("abc$def12345678901234567", true).problem.unwrap().contains("character"));
    let short = check_code("abc#def", true);
    assert!(short.problem.is_none() && short.warning.unwrap().contains("too short"));
    let nostate = check_code("AbCdEfGhIjKlMnOpQrStUvWxYz0123", true);
    assert!(nostate.problem.is_none() && nostate.warning.unwrap().contains('#'));
    assert!(check_code("AbCdEfGhIjKlMnOpQrStUvWxYz0123", false).warning.is_none());
    // the clipboard offer is strict
    assert!(looks_like_code(long, true));
    assert!(!looks_like_code("hello world, this is a sentence of text", true));
    assert!(!looks_like_code("short#code", true));
    assert!(!looks_like_code("https://claude.ai/oauth/authorize?x=1", true));
    assert!(!looks_like_code(&"a".repeat(80), true), "no # where Claude has one");
    assert!(looks_like_code(&"a".repeat(80), false));
}

#[test]
fn a_refused_code_that_is_asked_for_again_keeps_the_login_alive_for_a_retry() {
    let d = Dir::new("retry");
    // refuses the first code, accepts the second
    let prog = d.script("claude", "printf 'visit https://claude.example.test/oauth/authorize?code=true&client_id=00000000000000000000\\r\\nPaste code here if prompted > '\nread a\nsleep 1\nprintf '\\033[31mInvalid code. Please try again.\\033[0m\\r\\nPaste code here if prompted > '\nread b\n[ \"$b\" = good ] && { echo 'Login successful'; exit 0; }\nexit 1");
    let s = LoginSession::start(&d.spec(crate::extagent::CLAUDE_CODE, &prog));
    wait_for(&s, "the prompt", |s| s.wants_input);
    s.send_line("bad");
    let sn = wait_for(&s, "the refusal", |s| s.note.is_some());
    assert!(!sn.phase.finished() && sn.phase == Phase::Waiting && sn.wants_input, "{sn:?}");
    assert!(sn.note.unwrap().contains("Invalid code") && !sn.line.is_empty());
    s.send_line("good");
    assert_eq!(wait_for(&s, "the end", |s| s.phase.finished()).phase, Phase::Succeeded);
}

const FILE_TOKEN: &str = "ghu_FAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE0001";

fn config_with_token(d: &Dir, body: &str) -> PathBuf {
    std::fs::create_dir_all(d.0.join(".copilot")).unwrap();
    let f = d.0.join(".copilot/config.json");
    std::fs::write(&f, format!("// User settings belong in settings.json.\n// This file is managed automatically.\n{body}")).unwrap();
    f
}

#[test]
fn copilots_plain_file_token_is_found_in_either_shape_and_scrubbed_keeping_everything_else() {
    let d = Dir::new("scrub");
    assert!(copilot_file_token(&d.0).is_none(), "no file");
    let _f = config_with_token(&d, &format!("{{\"firstLaunchAt\":\"2026-10-18\",\"theme\":\"dark\",\"authTokens\":{{\"https://github.com:octocat\":\"{FILE_TOKEN}\"}},\"loggedInUsers\":[{{\"host\":\"https://github.com\",\"login\":\"octocat\"}}],\"lastLoggedInUser\":{{\"host\":\"https://github.com\",\"login\":\"octocat\"}}}}"));
    let t = copilot_file_token(&d.0).unwrap();
    assert_eq!((t.login.as_str(), t.token.as_str()), ("octocat", FILE_TOKEN));
    // the account from the key when no user list says
    config_with_token(&d, &format!("{{\"authTokens\":{{\"https://github.com:hubot\":{{\"token\":\"{FILE_TOKEN}\"}}}}}}"));
    assert_eq!(copilot_file_token(&d.0).unwrap().login, "hubot");
    config_with_token(&d, &format!("{{\"authTokens\":[\"{FILE_TOKEN}\"],\"loggedInUsers\":[{{\"login\":\"ann\"}}]}}"));
    assert_eq!(copilot_file_token(&d.0).unwrap().login, "ann");
    // scrub: the token goes, the header, the settings and the account list stay, the file stays private
    let f = config_with_token(&d, &format!("{{\"firstLaunchAt\":\"2026-10-18\",\"theme\":\"dark\",\"authTokens\":{{\"https://github.com:octocat\":\"{FILE_TOKEN}\"}},\"loggedInUsers\":[{{\"login\":\"octocat\"}}],\"lastLoggedInUser\":{{\"login\":\"octocat\"}}}}"));
    scrub_copilot_file(&d.0, false).unwrap();
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(!text.contains("ghu_") && !text.contains("FAKE"), "{text}");
    assert!(text.starts_with("// User settings belong in settings.json.\n// This file is managed automatically.\n") && text.contains("\"theme\": \"dark\"") && text.contains("firstLaunchAt") && text.contains("octocat"), "{text}");
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(copilot_file_token(&d.0).is_none(), "nothing left to take");
    assert_eq!(copilot_files_auth(None, Some(&d.0.display().to_string()), &|_| None), Auth::SignedIn("octocat".into()), "the account list is still there");
    // a sign-out forgets the account too; an absent file is not an error
    scrub_copilot_file(&d.0, true).unwrap();
    assert_eq!(copilot_files_auth(None, Some(&d.0.display().to_string()), &|_| None), Auth::SignedOut);
    assert!(scrub_copilot_file(&Dir::new("nofile").0, true).is_ok());
}

#[test]
fn a_vault_entry_holds_the_account_and_the_token_and_a_bad_one_is_refused() {
    let e = vault_entry("octocat", FILE_TOKEN);
    let (l, t) = parse_vault_entry(&e).unwrap();
    assert_eq!((l.as_str(), t.as_str()), ("octocat", FILE_TOKEN));
    assert!(parse_vault_entry("not json").is_none() && parse_vault_entry("{\"login\":\"x\"}").is_none() && parse_vault_entry("{\"token\":\"\"}").is_none());
}

#[test]
fn the_vault_login_is_the_status_and_a_locked_vault_is_not_called_signed_out() {
    let d = Dir::new("vstat");
    let mut cfg = AgentCfg { env: vec![("HOME".into(), d.0.display().to_string())], vault_login: Some("octocat".into()), ..Default::default() };
    let c = crate::extagent::copilot::Copilot::new(cfg.clone());
    assert_eq!(c.auth_offline(), Auth::SignedIn("octocat (token in your vault)".into()));
    cfg.vault_login = None;
    assert_eq!(crate::extagent::copilot::Copilot::new(cfg.clone()).auth_offline(), Auth::SignedOut, "the person's home has no sign-in");
    cfg.vault_locked = true;
    assert_eq!(crate::extagent::copilot::Copilot::new(cfg).auth_offline(), Auth::Unknown, "a vault that cannot be opened cannot be said to be empty");
}

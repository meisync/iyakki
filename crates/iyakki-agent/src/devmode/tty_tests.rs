//! F-DEV-16: a real pseudo-terminal behind `/v1/dev/shell/exec?tty=1`: size, input, Ctrl+C, resize, the grant and audit rules, and the
//! pure parts (window size limits, terminal type, the argument list). The terminal tests run a real `/bin/sh` on a real pty.

use super::shellexec::{clamp_win, clean_term, tty_argv};
use super::tests_adb::{call, key, rig, Rig};
use super::scopes::Scope;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn jv(o: &super::api::Out) -> Value {
    serde_json::from_slice(&o.body).unwrap_or(Value::Null)
}

fn admin() -> iyakki_sec::policy::Policy {
    iyakki_sec::policy::Role::Admin.template()
}

/// A session of a computer that holds the shell scope and an approved grant. Returns (rig, key, policy, sid).
fn session(tag: &str) -> (Rig, super::DevKey, iyakki_sec::policy::Policy, u64) {
    let r = rig(tag);
    let adm = admin();
    std::fs::write(r.dir.join("devmode/keys.json"), serde_json::to_string(&vec![key("laptop", "bob", &[Scope::Install, Scope::Shell])]).unwrap()).unwrap();
    let k = r.dm.keys().into_iter().find(|k| k.name == "laptop").unwrap();
    r.dm.shell_request(&k, false, 30, "test").unwrap();
    r.dm.shell_approve(&adm, "root", "laptop", 10).unwrap();
    let o = call(&r, &k, &adm, "POST", "/v1/dev/shell/open", &[], b"").unwrap();
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    let sid = jv(&o)["sid"].as_u64().unwrap();
    (r, k, adm, sid)
}

/// Read until the command is done or `needle` shows up (or 15 s); returns (all text, done, code).
fn read_until(r: &Rig, k: &super::DevKey, p: &iyakki_sec::policy::Policy, sid: u64, id: u64, needle: &str) -> (String, bool, Option<i64>) {
    let (mut since, mut all, t0) = (0u64, Vec::new(), Instant::now());
    loop {
        let (s, i) = (sid.to_string(), id.to_string());
        let sn = since.to_string();
        let o = call(r, k, p, "GET", "/v1/dev/shell/read", &[("sid", &s), ("id", &i), ("since", &sn), ("wait", "200"), ("enc", "b64")], b"").unwrap();
        let v = jv(&o);
        if let Some(b) = v["b64"].as_str().and_then(crate::sshkey::unb64) {
            all.extend(b);
        }
        since = v["next"].as_u64().unwrap_or(since);
        let text = String::from_utf8_lossy(&all).to_string();
        if v["done"].as_bool() == Some(true) || (!needle.is_empty() && text.contains(needle)) || t0.elapsed() > Duration::from_secs(15) {
            return (text, v["done"].as_bool() == Some(true), v["code"].as_i64());
        }
    }
}

fn exec_tty(r: &Rig, k: &super::DevKey, p: &iyakki_sec::policy::Policy, sid: u64, line: &str, rows: &str, cols: &str) -> u64 {
    let s = sid.to_string();
    let o = call(r, k, p, "POST", "/v1/dev/shell/exec", &[("sid", &s), ("tty", "1"), ("rows", rows), ("cols", cols), ("term", "xterm")], line.as_bytes()).unwrap();
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    jv(&o)["id"].as_u64().unwrap()
}

#[test]
fn window_sizes_terminal_names_and_the_shell_arguments_are_checked() {
    assert_eq!(clamp_win(0, 0), (24, 80));
    assert_eq!(clamp_win(30, 100), (30, 100));
    assert_eq!(clamp_win(100_000, 70_000), (500, 500));
    assert_eq!(clamp_win(0, 120), (24, 120));
    assert_eq!(clean_term("xterm-256color"), "xterm-256color");
    assert_eq!(clean_term("screen.linux"), "screen.linux");
    for bad in ["", "a b", "x;rm", "../x", &"a".repeat(33), "tab\t"] {
        assert_eq!(clean_term(bad), "xterm-256color", "{bad:?}");
    }
    assert_eq!(tty_argv(""), ["-i"]);
    assert_eq!(tty_argv("  "), ["-i"]);
    assert_eq!(tty_argv("ls -l"), ["-c", "ls -l"]);
}

#[cfg(unix)]
#[test]
fn a_command_on_a_terminal_sees_its_size_the_terminal_name_and_a_real_tty() {
    let (r, k, p, sid) = session("tty-a");
    let id = exec_tty(&r, &k, &p, sid, "stty size; echo T=$TERM; test -t 0 && test -t 1 && echo ISATTY; tty", "30", "100");
    let (text, done, code) = read_until(&r, &k, &p, sid, id, "");
    assert!(done && code == Some(0), "{text:?} {code:?}");
    assert!(text.contains("30 100"), "{text:?}");
    assert!(text.contains("T=xterm") && text.contains("ISATTY"), "{text:?}");
    assert!(text.contains("/dev/"), "tty prints the terminal's name: {text:?}");
    // the same line without a terminal has none
    let s = sid.to_string();
    let o = call(&r, &k, &p, "POST", "/v1/dev/shell/exec", &[("sid", &s)], b"test -t 0 || echo NOTTY").unwrap();
    let id2 = jv(&o)["id"].as_u64().unwrap();
    let (t2, _, _) = read_until(&r, &k, &p, sid, id2, "");
    assert!(t2.contains("NOTTY"), "{t2:?}");
}

#[cfg(unix)]
#[test]
fn keys_go_in_as_bytes_the_terminal_edits_the_line_and_ctrl_c_stops_the_foreground_command() {
    let (r, k, p, sid) = session("tty-b");
    // an interactive shell (empty line): type a command with a mistake fixed by backspace, then exit
    let id = exec_tty(&r, &k, &p, sid, "", "24", "80");
    let (s, i) = (sid.to_string(), id.to_string());
    let send = |bytes: &[u8]| {
        let o = call(&r, &k, &p, "POST", "/v1/dev/shell/stdin", &[("sid", &s), ("id", &i)], bytes).unwrap();
        assert_eq!(o.status, 200);
    };
    send(b"echo helXX\x7f\x7flo-$((20+22))\n");
    // the echo differs between shells and terminals (backspace comes back as erase sequences, a prompt may precede the output). "hello-42" can only come
    // from the command running (the echo of the typed line holds "hello-$((20+22))"), and the two erased letters never reached it.
    let (text, done, _) = read_until(&r, &k, &p, sid, id, "hello-42");
    assert!(text.contains("hello-42") && !text.contains("helXXlo-42") && !done, "{text:?}");
    // a long-running command, interrupted with the byte Ctrl+C
    send(b"sleep 30\n");
    std::thread::sleep(Duration::from_millis(400));
    send(&[3]);
    send(b"echo after-$((3+4))\n");
    let (text, _, _) = read_until(&r, &k, &p, sid, id, "after-7");
    // the answer came long before the sleep would have ended (the tests would not wait 30 s)
    assert!(text.contains("after-7"), "{text:?}");
    // Ctrl+D ends the shell
    send(&[4]);
    let (_, done, code) = read_until(&r, &k, &p, sid, id, "");
    assert!(done && code == Some(0), "{code:?}");
    // the audit log names the terminal and never the typed keys
    let log: String = r.dm.activity(200).iter().map(|x| x.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("(terminal)") && log.contains("interactive shell") && !log.contains("hello-"), "{log}");
    // the end of the session says how much was typed (two counts), still without the keys
    let t0 = Instant::now();
    let mut log = log;
    while !log.contains("lines (not recorded)") && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        log = r.dm.activity(200).iter().map(|x| x.line()).collect::<Vec<_>>().join("\n");
    }
    assert!(log.contains("typed ") && log.contains("in 3 lines (not recorded)") && !log.contains("hello-") && !log.contains("sleep 30"), "{log}");
}

#[cfg(unix)]
#[test]
fn resizing_changes_what_the_command_sees_and_a_plain_command_has_no_terminal_to_resize() {
    let (r, k, p, sid) = session("tty-c");
    let id = exec_tty(&r, &k, &p, sid, "sleep 1; stty size", "24", "80");
    let (s, i) = (sid.to_string(), id.to_string());
    let o = call(&r, &k, &p, "POST", "/v1/dev/shell/resize", &[("sid", &s), ("id", &i), ("rows", "41"), ("cols", "133")], b"").unwrap();
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    let (text, done, _) = read_until(&r, &k, &p, sid, id, "");
    assert!(done && text.contains("41 133"), "{text:?}");
    // after it ended, a resize says so
    let o = call(&r, &k, &p, "POST", "/v1/dev/shell/resize", &[("sid", &s), ("id", &i), ("rows", "10"), ("cols", "10")], b"").unwrap();
    assert_eq!(o.status, 409);
    // a plain command has no terminal
    let o = call(&r, &k, &p, "POST", "/v1/dev/shell/exec", &[("sid", &s)], b"sleep 1").unwrap();
    let pid = jv(&o)["id"].as_u64().unwrap().to_string();
    let o = call(&r, &k, &p, "POST", "/v1/dev/shell/resize", &[("sid", &s), ("id", &pid), ("rows", "10"), ("cols", "10")], b"").unwrap();
    assert!(o.status == 409 && String::from_utf8_lossy(&o.body).contains("no terminal"));
}

#[cfg(unix)]
#[test]
fn the_terminal_needs_the_same_grant_and_dies_with_it() {
    let (r, k, p, sid) = session("tty-d");
    let id = exec_tty(&r, &k, &p, sid, "sleep 30", "24", "80");
    // the grant ends (the administrator ends the sessions): the command is killed and the session is gone
    r.dm.shell_end(&p, "root", "laptop").unwrap();
    let t0 = Instant::now();
    loop {
        let (s, i) = (sid.to_string(), id.to_string());
        let o = call(&r, &k, &p, "GET", "/v1/dev/shell/read", &[("sid", &s), ("id", &i), ("since", "0"), ("wait", "0")], b"").unwrap();
        if o.status != 200 {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "the session should end with its grant");
        std::thread::sleep(Duration::from_millis(100));
    }
    // and with no grant no terminal can even be opened
    let o = call(&r, &k, &p, "POST", "/v1/dev/shell/open", &[], b"").unwrap();
    assert_eq!(o.status, 409);
    r.clock.fetch_add(0, Ordering::SeqCst);
}

#[cfg(unix)]
#[test]
fn a_read_from_beyond_the_end_of_the_output_answers_nothing_and_does_not_panic() {
    let (r, k, p, sid) = session("tty-since");
    let id = exec_tty(&r, &k, &p, sid, "echo hello", "24", "80");
    let (text, done, _) = read_until(&r, &k, &p, sid, id, "");
    assert!(done && text.contains("hello"));
    let (s, i) = (sid.to_string(), id.to_string());
    let o = call(&r, &k, &p, "GET", "/v1/dev/shell/read", &[("sid", &s), ("id", &i), ("since", "18446744073709551000"), ("wait", "0")], b"").unwrap();
    let v = jv(&o);
    assert_eq!((o.status, v["text"].as_str(), v["lost"].as_u64()), (200, Some(""), Some(0)), "{v}");
}

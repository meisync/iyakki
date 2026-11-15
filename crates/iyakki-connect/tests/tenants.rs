// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! `iyakki-connect tenants`: the list of tenants the signed-in account may use, read from the tenant listing route of a fake tenant over
//! real TLS. It proves the connector's handling (pin first, password from stdin, current marked, control characters dropped), not a
//! real cluster.

mod common;

use iyakki_connect::{pins::Store, run, Ctx, Fail};
use std::io::Cursor;
use std::time::Duration;

fn go(c: &Ctx, input: &str, args: &[&str]) -> (Result<(), Fail>, String) {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let r = run(&a, c, &mut Cursor::new(input.to_string()), &mut out);
    (r, String::from_utf8(out).unwrap())
}

#[test]
fn tenants_lists_what_the_server_says_marks_the_current_one_and_needs_a_pin_and_a_password() {
    let r = common::start_remote();
    let d = std::env::temp_dir().join(format!("iyakki-connect-test-tenants-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let c = Ctx { store: Store::new(&d), timeout: Duration::from_secs(5), interactive: false };
    let addr = &r.target.addr;
    let (res, _) = go(&c, "right\n", &["tenants", addr, common::USER]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("not pinned")), "an unpinned tenant is not asked");
    go(&c, "", &["pin", addr, "--yes"]).0.unwrap();
    let (res, out) = go(&c, "right\n", &["tenants", addr, common::USER]);
    res.unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("* acme") && lines[0].contains("Acme Ltd") && lines[0].contains("(member)"), "{out}");
    assert!(lines[1].starts_with("  beta") && lines[1].contains("(owner)"), "{out}");
    assert!(!out.contains('\u{7}'), "control characters from the server are not printed");
    let (res, _) = go(&c, "wrong\n", &["tenants", addr, common::USER]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("refused")));
    let (res, _) = go(&c, "\n", &["tenants", addr, common::USER]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("no password")));
}

#[test]
fn switch_moves_to_another_tenant_says_what_the_server_stopped_and_refuses_one_that_is_not_yours() {
    let r = common::start_remote();
    let d = std::env::temp_dir().join(format!("iyakki-connect-test-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let c = Ctx { store: Store::new(&d), timeout: Duration::from_secs(5), interactive: false };
    let addr = &r.target.addr;
    let (res, _) = go(&c, "right\n", &["switch", addr, common::USER, "beta"]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("not pinned")), "an unpinned tenant is not asked");
    go(&c, "", &["pin", addr, "--yes"]).0.unwrap();
    let (res, out) = go(&c, "right\n", &["switch", addr, common::USER, "beta"]);
    res.unwrap();
    assert!(out.contains("now in beta") && out.contains("you are owner") && out.contains("before: acme"), "{out}");
    assert!(out.contains("stopped 2 running chat(s) and locked the previous tenant's vault"), "{out}");
    assert!(!out.contains('\u{7}'), "control characters from the server are not printed");
    let (res, _) = go(&c, "right\n", &["switch", addr, common::USER, "nope"]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("no such tenant")));
    let (res, _) = go(&c, "wrong\n", &["switch", addr, common::USER, "beta"]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("refused")));
    let (res, _) = go(&c, "\n", &["switch", addr, common::USER, "beta"]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("no password")));
}

#[test]
fn login_signs_in_with_the_password_from_standard_input_says_so_and_never_echoes_it() {
    let r = common::start_remote();
    let d = std::env::temp_dir().join(format!("iyakki-connect-test-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let c = Ctx { store: Store::new(&d), timeout: Duration::from_secs(5), interactive: false };
    let addr = &r.target.addr;
    // not pinned: nothing is sent, the password is not read as a secret to try
    let (res, out) = go(&c, "right\n", &["login", addr, common::USER]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("not pinned")) && out.is_empty());
    go(&c, "", &["pin", addr, "--yes"]).0.unwrap();
    // the right password: a success line naming the account, and the password is nowhere in the output
    let (res, out) = go(&c, "right\n", &["login", addr, common::USER]);
    res.unwrap();
    assert!(out.starts_with("signed in to ") && out.contains(&format!("as {}", common::USER)), "{out}");
    assert!(!out.contains("right"), "the password is never printed: {out}");
    // a Windows line ending after the password is not part of it
    let (res, _) = go(&c, "right\r\n", &["login", addr, common::USER]);
    res.unwrap();
    // the wrong password is refused with the server's reason and prints no success line
    let (res, out) = go(&c, "wrong\n", &["login", addr, common::USER]);
    assert!(matches!(res, Err(Fail::Error(m)) if m.contains("sign-in refused")), "a wrong password is refused");
    assert!(out.is_empty(), "{out}");
    // an empty line is not sent as a password, and a password in the arguments is not accepted
    assert!(matches!(go(&c, "\n", &["login", addr, common::USER]).0, Err(Fail::Error(m)) if m.contains("no password")));
    assert!(matches!(go(&c, "", &["login", addr, common::USER, "right"]).0, Err(Fail::Error(m)) if m.contains("usage")));
    let _ = std::fs::remove_dir_all(&d);
}

//! End-to-end tests driving the real `iyakki` binary against WASM fixtures in /tests/fixtures.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name)
}

fn iyakki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_iyakki")).args(args).output().unwrap()
}

#[test]
fn runs_hello() {
    let o = iyakki(&["run", fixture("hello.wat").to_str().unwrap()]);
    assert!(o.status.success());
    assert_eq!(String::from_utf8_lossy(&o.stdout), "hello from guest\n");
}

#[test]
fn exit_code_propagates() {
    let o = iyakki(&["run", fixture("exit7.wat").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(7));
}

#[test]
fn infinite_loop_is_stopped_by_fuel() {
    let o = iyakki(&["run", fixture("spin.wat").to_str().unwrap(), "--fuel", "50000"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("fuel"));
}

#[test]
fn filesystem_access_needs_a_grant() {
    let f = fixture("write_ws.wat");
    let denied = iyakki(&["run", f.to_str().unwrap()]);
    assert_eq!(denied.status.code(), Some(0), "denied errno is negative -> clamped to 0 exit; see stderr log");
    assert!(String::from_utf8_lossy(&denied.stderr).contains("result=Some(-2)"));
    let allowed = iyakki(&["run", f.to_str().unwrap(), "--allow-write", "/ws"]);
    assert!(String::from_utf8_lossy(&allowed.stderr).contains("result=Some(0)"));
}

#[test]
fn bad_input_fails_cleanly() {
    assert!(!iyakki(&["run", "/no/such/file.wasm"]).status.success());
    assert_eq!(iyakki(&["bogus-command"]).status.code(), Some(2));
    assert!(iyakki(&["detect"]).status.success());
}

#[test]
fn net_status_lists_loopback() {
    let o = iyakki(&["net", "status"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("Loopback"));
}

/// IYAKKI as PID 1 must survive a console that fails every write (a PC without a serial port, with the console
/// routed to it): `println!`/`eprintln!` would panic, and a panic in PID 1 is a kernel panic.
#[test]
fn a_console_that_rejects_every_write_does_not_crash_iyakki() {
    use std::process::{Command, Stdio};
    let full = || std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_iyakki")).arg("detect").stdout(Stdio::from(full())).stderr(Stdio::from(full())).status().unwrap();
    assert!(out.success(), "detect printed into /dev/full and must still succeed, got {out:?}");
    let out = Command::new(env!("CARGO_BIN_EXE_iyakki")).arg("hwreport").stdout(Stdio::from(full())).stderr(Stdio::from(full())).status().unwrap();
    assert!(out.success(), "hwreport: {out:?}");
    // the log goes through the same path
    let out = Command::new(env!("CARGO_BIN_EXE_iyakki")).arg("storage").env("IYAKKI_LOG", "trace").stdout(Stdio::from(full())).stderr(Stdio::from(full())).status().unwrap();
    assert!(out.success(), "storage: {out:?}");
}

/// `iyakki vpn inspect FILE` as a built program (F-VPN-10): the report on a standard output, the exit code (0 accepted, 1 not accepted,
/// 2 not readable), and not one value of the file anywhere in either output. Every secret below is a made-up canary.
#[test]
fn the_built_vpn_inspect_command_reports_on_files_and_never_prints_a_value() {
    const KEY: &str = "CANARYKEYMATERIAL0123456789ABCDEF";
    const HOST: &str = "canary-server-7731.example.net";
    const PASS: &str = "canaryuser-pass-file-4471";
    let dir = std::env::temp_dir().join(format!("iyakki-vpn-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, text: &str| -> String {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    let run = |file: &str| iyakki(&["vpn", "inspect", file]);
    let all = |o: &Output| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));

    // a good OpenVPN profile: accepted, exit 0, described, and no value printed
    let good = write(
        "good.ovpn",
        &format!("client\ndev tun\nproto udp\nremote {HOST} 1194\nremote-cert-tls server\ncipher AES-256-GCM\nauth SHA256\nauth-user-pass\nnobind\n<ca>\n-----BEGIN CERTIFICATE-----\nFAKECA\n-----END CERTIFICATE-----\n</ca>\n<key>\n-----BEGIN PRIVATE KEY-----\n{KEY}\n-----END PRIVATE KEY-----\n</key>\n"),
    );
    let o = run(&good);
    assert_eq!(o.status.code(), Some(0), "{}", all(&o));
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(text.contains("Kind: OpenVPN profile") && text.contains("Needs a user name and password: YES") && text.contains("accept this profile as it is: YES"), "{text}");
    assert!(!all(&o).contains(KEY) && !all(&o).contains(HOST) && !all(&o).contains("FAKECA"), "a value of the file reached the output:\n{text}");

    // a provider-style file with a program-running line: not accepted, exit 1, the refusal and what to do are said, the script path is not
    let bad = write("bad.ovpn", &format!("client\nremote {HOST} 1194\nup /home/canaryperson/{PASS}.sh\nscript-security 2\n<ca>\nx\n</ca>\n"));
    let o = run(&bad);
    assert_eq!(o.status.code(), Some(1), "{}", all(&o));
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(text.contains("REFUSED") && text.contains("accept this profile as it is: NO") && text.contains("What you can do"), "{text}");
    assert!(!all(&o).contains(PASS) && !all(&o).contains("canaryperson") && !all(&o).contains(HOST), "{text}");

    // a WireGuard file: described, and neither its private key nor its endpoint is printed
    let wg = write("wg.conf", &format!("[Interface]\nPrivateKey = {KEY}\nAddress = 10.8.0.2/24\n[Peer]\nPublicKey = {KEY}\nEndpoint = {HOST}:51820\nAllowedIPs = 0.0.0.0/0\n"));
    let o = run(&wg);
    assert!(matches!(o.status.code(), Some(0) | Some(1)), "{}", all(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("WireGuard") && !all(&o).contains(KEY) && !all(&o).contains(HOST), "{}", all(&o));

    // not readable (missing, a folder, over the size limit) and wrong use: exit 2 or 1, and the path is never echoed
    let o = run(&dir.join("missing.ovpn").to_string_lossy());
    assert_eq!(o.status.code(), Some(2));
    assert!(!all(&o).contains("missing.ovpn") && !all(&o).contains(&*dir.to_string_lossy()), "{}", all(&o));
    assert_eq!(run(&dir.to_string_lossy()).status.code(), Some(2), "a folder is not a profile");
    let big = write("big.ovpn", &"#".repeat(2 * 1024 * 1024));
    assert_eq!(run(&big).status.code(), Some(1), "a huge file is refused without being read whole");
    assert_eq!(iyakki(&["vpn"]).status.code(), Some(2), "no file given");
    assert_eq!(iyakki(&["vpn", "inspect"]).status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}

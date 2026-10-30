use super::*;

/// Every secret below is made up. After redaction none of them may appear anywhere.
const SECRETS: &[&str] = &[
    "ann@example.org",
    "hunter2hunter2",
    "sk-abcdefghijklmnopqrstuvwxyz0123456789",
    "ghp_0123456789abcdefghij0123456789abcdef",
    "AKIAABCDEFGHIJKLMNOP",
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop",
    "s3cr3tC00kieValue99",
    "Zm9vOmJhcg0123456789ab",
    "0123456789abcdef0123456789abcdef0123",
    "session=xyz789query",
    "alice.example@mail.example.org",
    "aa:bb:cc:dd:ee:01",
    "203.0.113.45",
    "2001:db8:85a3::8a2e:370:7334",
    "annsmith",
    "openai-key-value-777",
    "BEGIN-PRIVATE-BODY-MIIEvQIBADANBgkqhkiG9w0BAQEFAASC",
];

fn corpus() -> String {
    [
        "Cookie: sid=s3cr3tC00kieValue99; theme=dark",
        "set-cookie: auth=s3cr3tC00kieValue99; HttpOnly",
        "Authorization: Bearer Zm9vOmJhcg0123456789ab",
        "GET https://api.example.org/v1/items?token=ghp_0123456789abcdefghij0123456789abcdef&session=xyz789query#frag HTTP/1.1",
        "clone https://annsmith:hunter2hunter2@git.example.org/team/repo.git failed",
        "password=hunter2hunter2 user=admin",
        "{\"password\": \"hunter2hunter2\", \"name\": \"x\"}",
        "api_key: sk-abcdefghijklmnopqrstuvwxyz0123456789",
        "the key is sk-abcdefghijklmnopqrstuvwxyz0123456789 in env",
        "AWS id AKIAABCDEFGHIJKLMNOP in config",
        "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop sent",
        "vault:openai-key=openai-key-value-777",
        "vault entry \"gitea-token\" = \"openai-key-value-777\"",
        "secret hash 0123456789abcdef0123456789abcdef0123 stored",
        "mail alice.example@mail.example.org bounced",
        "wlan0 link/ether aa:bb:cc:dd:ee:01 brd ff:ff:ff:ff:ff:ff",
        "connect from 203.0.113.45:51234 refused",
        "route via 2001:db8:85a3::8a2e:370:7334 up",
        "opened /home/annsmith/notes/todo.md and /Users/annsmith/x",
        "-----BEGIN PRIVATE KEY-----\nBEGIN-PRIVATE-BODY-MIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END PRIVATE KEY-----",
    ]
    .join("\n")
}

#[test]
fn no_secret_in_the_corpus_survives_redaction() {
    let out = redact(&corpus());
    for s in SECRETS {
        assert!(!out.contains(s), "{s:?} survived:\n{out}");
    }
}

#[test]
fn every_line_is_redacted_on_its_own_too() {
    for l in corpus().lines() {
        let out = redact_line(l);
        for s in SECRETS {
            // the key block's body is on lines of its own: only the whole-text pass removes it
            if l.contains("BEGIN-PRIVATE-BODY") || l.contains("-----") {
                continue;
            }
            assert!(!out.contains(s), "{s:?} survived in {l:?} -> {out:?}");
        }
    }
}

#[test]
fn the_shape_of_a_line_is_kept_so_it_can_still_be_read() {
    assert_eq!(redact_line("Cookie: sid=abc"), "Cookie: ***");
    assert_eq!(redact_line("password=hunter2hunter2 user=admin"), "password=*** user=admin");
    assert_eq!(redact_line("{\"password\": \"hunter2hunter2\", \"name\": \"x\"}"), "{\"password\": \"***\", \"name\": \"x\"}");
    assert_eq!(redact_line("GET https://api.example.org/v1/items?a=1&b=2#f ok"), "GET https://api.example.org/v1/items?*** ok");
    assert_eq!(redact_line("clone https://bob:pw1234@git.example.org/r.git"), "clone https://[credentials]@git.example.org/r.git");
    assert_eq!(redact_line("opened /home/ann/notes.md"), "opened /home/[user]/notes.md");
    assert_eq!(redact_line("from 203.0.113.45:80 and 127.0.0.1:7979"), "from [ip]:80 and 127.0.0.1:7979");
    assert_eq!(redact_line("eth0 aa:bb:cc:dd:ee:01"), "eth0 [mac]");
    assert_eq!(redact_line("mail bob@example.org."), "mail [email].");
}

#[test]
fn ordinary_text_is_not_damaged() {
    for l in [
        "IYAKKI 0.1.0-alpha.2 · 2026-10-27 · kernel 6.12.4",
        "Dev cluster (k3s)    running   node Ready",
        "12:34:56 sound card 0: PCH [HDA Intel PCH]",
        "The disk is almost full.",
        "tokens used: 1200",
        "vault: unlocked",
        "1024 x 768 at 60 Hz",
        "loopback ::1 and 127.0.0.1",
    ] {
        assert_eq!(redact_line(l), l, "changed: {l}");
    }
}

fn inputs() -> Inputs {
    Inputs {
        version: vec!["IYAKKI 0.1.0-alpha.2 · kernel 6.12".into()],
        services: vec!["Clock                ready".into(), "Dev cluster (k3s)    failed    exit 1 on 10.255.77.1".into()],
        hardware: "CPU\n  Intel at /home/annsmith\n".into(),
        doctor: vec!["[Problem] The disk is almost full.".into(), "    token=hunter2hunter2".into()],
        engine_log: "100 helper: v4l2src busy\n101 key sk-abcdefghijklmnopqrstuvwxyz0123456789 seen".into(),
        boot_log: "booted\nCookie: s3cr3tC00kieValue99".into(),
        switches: vec!["developer mode".into()],
        checkup: vec!["1 passed, 1 failed, 0 skipped, 10 not done yet".into(), "Microphone: Fail - Only silence was heard (USB Audio).".into(), "  advice: contact me at ann@example.org".into()],
    }
}

#[test]
fn the_preview_is_exactly_the_file_and_holds_none_of_the_secrets() {
    let b = build(&inputs());
    let shown = preview(&b);
    let dir = std::env::temp_dir().join(format!("iyakki-diagshare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = save(&b, &dir, "bundle.txt").unwrap();
    let written = std::fs::read(&p).unwrap();
    assert_eq!(written, shown.as_bytes(), "byte for byte what was shown");
    for s in SECRETS {
        assert!(!shown.contains(s), "{s:?}\n{shown}");
    }
    for title in ["Version", "Services", "Doctor", "Check-up", "Hardware", "Web engine log", "Boot log"] {
        assert!(shown.contains(&format!("=== {title}")), "{title}");
    }
    assert!(shown.starts_with("IYAKKI diagnostics."));
    assert!(shown.contains("v4l2src busy"), "the helper's own words stay");
    assert!(shown.contains("/home/[user]"));
    // never replaces a file, never walks out of the folder
    assert!(save(&b, &dir, "bundle.txt").unwrap_err().contains("already"));
    assert!(save(&b, &dir, "../x.txt").is_err());
    assert!(save(&b, &dir, "").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_huge_log_is_cut_from_the_old_end_to_the_size_cap() {
    let mut i = inputs();
    i.engine_log = (0..40_000).map(|n| format!("{n} line {n} of a very long kept log with some filler text")).collect::<Vec<_>>().join("\n");
    let shown = preview(&build(&i));
    assert!(shown.len() <= MAX_BYTES + 1024, "{}", shown.len());
    assert!(shown.contains("line 39999 "), "the newest lines stay");
    assert!(!shown.contains("\n0 line 0 "), "the oldest went first");
}

#[test]
fn an_empty_section_says_so_and_the_file_name_is_a_plain_dated_name() {
    let b = build(&Inputs::default());
    assert!(preview(&b).contains("(nothing)"));
    assert_eq!(file_name(0), "iyakki-diagnostics-19700101-000000.txt");
    assert_eq!(file_name(1_800_000_000), "iyakki-diagnostics-20270115-080000.txt");
    assert_eq!(file_name(951_782_400 + 3661), "iyakki-diagnostics-20000229-010101.txt", "a leap day");
}

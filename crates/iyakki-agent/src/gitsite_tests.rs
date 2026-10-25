use super::*;

fn p(s: &str) -> Result<RepoUrl, String> {
    parse(s, ParseOptions::default())
}

fn temp(tag: &str) -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("gs-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn https_ssh_and_scp_like_addresses_are_read() {
    let u = p("https://GitHub.com/octo/Hello-World.git").unwrap();
    assert_eq!((u.scheme, u.host.as_str(), u.path.as_str(), u.site().as_str()), (Scheme::Https, "github.com", "octo/Hello-World.git", "github.com"));
    assert_eq!(u.short(), "octo/Hello-World");
    let u = p("git@github.com:octo/repo.git").unwrap();
    assert_eq!((u.scheme, u.host.as_str(), u.user.as_deref(), u.path.as_str()), (Scheme::Ssh, "github.com", Some("git"), "octo/repo.git"));
    assert_eq!(u.ssh_port(), 22);
    let u = p("ssh://git@git.example.org:2222/team/app").unwrap();
    assert_eq!((u.host.as_str(), u.port, u.ssh_port(), u.site().as_str()), ("git.example.org", Some(2222), 2222, "git.example.org"));
    let u = p("https://gitea.lan:3000/me/x.git").unwrap();
    assert_eq!(u.site(), "gitea.lan");
    let u = p("example.org:team/app.git").unwrap();
    assert_eq!((u.scheme, u.user), (Scheme::Ssh, None));
}

#[test]
fn credentials_in_an_address_are_refused_and_never_echoed() {
    for bad in ["https://me:hunter2@github.com/o/r.git", "https://ghp_abcdef:x-oauth-basic@github.com/o/r", "ssh://git:pw@host/o/r"] {
        let e = p(bad).unwrap_err();
        assert!(e.contains("Settings → Git and SSH keys"), "{e}");
        assert!(!e.contains("hunter2") && !e.contains("ghp_abcdef") && !e.contains("pw@"), "{e}");
    }
    assert_eq!(redact_url("https://me:hunter2@github.com/o/r.git"), "https://me@github.com/o/r.git");
    assert_eq!(redact_url("https://github.com/o/r.git"), "https://github.com/o/r.git");
    assert_eq!(redact_url("git@host:o/r"), "git@host:o/r");
    // a user name without a password is not a secret
    assert_eq!(p("https://me@github.com/o/r.git").unwrap().user.as_deref(), Some("me"));
}

#[test]
fn addresses_that_run_programs_or_become_options_are_refused() {
    for bad in ["", "   ", "-oProxyCommand=evil", "--upload-pack=evil", "ext::sh -c evil", "fd::3", "file:///etc", "/etc/passwd", "../x", "git://host/o/r", "ftp://h/x", "https://host", "https://ho st/o/r", "https://host/o/../../x", "https://-host/o/r", "git@host:-x", "git@host:o/r;rm", "git@ho$st:o/r", "https://host/o/r`x`", "just words", "host:"] {
        assert!(p(bad).is_err(), "should be refused: {bad:?}");
    }
    assert!(parse("file:///tmp/x", ParseOptions { allow_local: true }).is_ok());
    assert!(parse("/tmp/x/y.git", ParseOptions { allow_local: true }).is_ok());
}

#[test]
fn the_text_area_gives_one_line_per_repository_with_an_optional_branch() {
    let l = split_lines("https://a/b/c.git\n\n  git@h:o/r.git #dev  \nhttps://x/y/z#release/1\n# a comment line\n");
    assert_eq!(l.len(), 3);
    assert_eq!((l[0].url.as_str(), l[0].branch.as_str()), ("https://a/b/c.git", ""));
    assert_eq!((l[1].url.as_str(), l[1].branch.as_str()), ("git@h:o/r.git", "dev"));
    assert_eq!((l[2].url.as_str(), l[2].branch.as_str()), ("https://x/y/z", "release/1"));
    assert!(valid_branch("main") && valid_branch("feature/x-1") && valid_branch(""));
    for b in ["-x", "a b", "a..b", "x/", "a:b", "a~1", "--upload-pack=x", "a@{1}"] {
        assert!(!valid_branch(b), "{b}");
    }
}

#[test]
fn folder_names_come_from_the_address_are_unique_and_stay_inside() {
    let root = temp("names");
    let u = p("https://github.com/octo/Hello World?.git".replace(' ', "-").as_str()).unwrap();
    let mut taken = HashSet::new();
    assert_eq!(folder_for(&root, &u, "", &taken).unwrap(), "Hello-World");
    taken.insert("hello-world".to_string());
    assert_eq!(folder_for(&root, &u, "", &taken).unwrap(), "Hello-World-2", "the same name twice in one batch (case-insensitive)");
    // an existing non-empty folder is never used; an empty one may be
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(root.join("app/file"), "x").unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let a = p("git@h:o/app.git").unwrap();
    assert_eq!(folder_for(&root, &a, "", &HashSet::new()).unwrap(), "app-2");
    assert!(folder_for(&root, &a, "app", &HashSet::new()).unwrap_err().contains("already exists"));
    assert_eq!(folder_for(&root, &a, "empty", &HashSet::new()).unwrap(), "empty");
    // escapes
    for bad in ["../x", "a/b", "..", ".", ".hidden", "/etc", "a\\b", "x y", "x;y"] {
        assert!(folder_for(&root, &a, bad, &HashSet::new()).is_err(), "{bad}");
    }
    // a symbolic link, even to an empty folder, is not a free name
    std::os::unix::fs::symlink("/tmp", root.join("lnk")).unwrap();
    assert!(!folder_free(&root, "lnk", &HashSet::new()));
    // odd URL paths give plain names
    assert_eq!(suggested_name(&p("https://h/o/.git").unwrap()), "repo");
    assert_eq!(suggested_name(&p("https://h/o/%2e%2e.git").unwrap()), "2e-2e");
    assert_eq!(clean_name("../../etc"), "etc");
    assert_eq!(clean_name("Ünï cödé"), "n-c-d");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repo_entries_keep_the_commands_through_a_reclone_and_describe_without_secrets() {
    let mut v = vec![];
    upsert(&mut v, RepoEntry { url: "https://h/o/a.git".into(), path: "a".into(), branch: "main".into(), key: "token".into(), state: "cloned".into(), ..Default::default() });
    v[0].test = "cargo test".into();
    upsert(&mut v, RepoEntry { url: "https://h/o/a.git".into(), path: "a".into(), branch: "dev".into(), ..Default::default() });
    upsert(&mut v, RepoEntry { url: "git@h:o/b.git".into(), path: "b".into(), ..Default::default() });
    assert_eq!(v.len(), 2);
    assert_eq!((v[0].branch.as_str(), v[0].test.as_str()), ("dev", "cargo test"));
    let d = describe(&v);
    assert!(d.contains("- a/ <- https://h/o/a.git (branch dev); test: cargo test") && d.contains("- b/ <- git@h:o/b.git"), "{d}");
    let json = serde_json::to_string(&v).unwrap();
    assert!(!json.contains("\"run\"") && !json.contains("depth"), "empty values are not written: {json}");
    assert_eq!(describe(&[]), "");
    let leaky = RepoEntry { url: "https://me:hunter2@h/o/c.git".into(), path: "c".into(), ..Default::default() };
    assert!(!describe(&[leaky]).contains("hunter2"));
}

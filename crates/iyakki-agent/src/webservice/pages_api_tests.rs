//! The routes behind the Files, Git and Flows pages: move and mkdir, the read-only git views, and starting and listing flow runs.

use super::*;
use std::process::Command;

fn on_disk(h: &H, rel: &str) -> std::path::PathBuf {
    h.dir.join("cloud").join("files").join(rel)
}
fn post(h: &H, tok: &str, path: &str, body: Value) -> (u16, Value) {
    let (s, v, _) = h.call("POST", path, Some(tok), Some(body));
    (s, v)
}
fn get(h: &H, tok: &str, path: &str) -> (u16, Value) {
    let (s, v, _) = h.call("GET", path, Some(tok), None);
    (s, v)
}
fn mv(h: &H, tok: &str, from: &str, to: &str) -> u16 {
    post(h, tok, "/api/v1/files/move", json!({"namespace": "web", "from": from, "to": to})).0
}
fn put(h: &H, tok: &str, path: &str, content: &str) {
    assert_eq!(post(h, tok, "/api/v1/files/write", json!({"namespace": "web", "path": path, "content": content})).0, 200);
}
fn audit_has(h: &H, id: &str) -> bool {
    h.accounts.lines().iter().any(|l| l.contains(&format!("api.{id}")))
}

// ------------------------------------------------------------------------------------------------------------ move

#[test]
fn move_renames_files_and_folders_inside_one_space_and_audits_it() {
    let (h, _, ann, _, _) = acme_cloud();
    put(&h, &ann, "a/one.txt", "hello");
    put(&h, &ann, "a/two.txt", "second");
    let base = on_disk(&h, "ns/web/people/ann");
    assert_eq!(mv(&h, &ann, "a/one.txt", "a/uno.txt"), 200);
    assert!(!base.join("a/one.txt").exists() && std::fs::read_to_string(base.join("a/uno.txt")).unwrap() == "hello", "the same bytes, new name (quota neutral)");
    assert_eq!(mv(&h, &ann, "a", "b"), 200, "a folder with its files");
    assert_eq!(std::fs::read_to_string(base.join("b/two.txt")).unwrap(), "second");
    assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "dest"})).0, 200);
    assert_eq!(mv(&h, &ann, "b/uno.txt", "dest/uno.txt"), 200, "into another folder");
    assert_eq!(mv(&h, &ann, "b/uno.txt", "b/x.txt"), 404, "the source is gone");
    assert_eq!(mv(&h, &ann, "dest/uno.txt", "nowhere/uno.txt"), 404, "the folder to move into must exist");
    assert!(audit_has(&h, "files_move") && audit_has(&h, "files_mkdir"));
    // the personal and shared spaces work the same way, each on its own
    assert_eq!(post(&h, &ann, "/api/v1/files/write", json!({"path": "p.txt", "content": "x"})).0, 200);
    assert_eq!(post(&h, &ann, "/api/v1/files/move", json!({"from": "p.txt", "to": "q.txt"})).0, 200);
    assert!(on_disk(&h, "personal/ann/q.txt").exists());
}

#[test]
fn move_refuses_to_replace_unless_asked_and_never_replaces_a_folder() {
    let (h, _, ann, _, _) = acme_cloud();
    put(&h, &ann, "x.txt", "X");
    put(&h, &ann, "y.txt", "Y");
    assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "d/inner"})).0, 200);
    let base = on_disk(&h, "ns/web/people/ann");
    assert_eq!(mv(&h, &ann, "x.txt", "y.txt"), 409);
    assert_eq!(std::fs::read_to_string(base.join("y.txt")).unwrap(), "Y", "nothing changed");
    assert_eq!(post(&h, &ann, "/api/v1/files/move", json!({"namespace": "web", "from": "x.txt", "to": "y.txt", "overwrite": false})).0, 409);
    assert_eq!(post(&h, &ann, "/api/v1/files/move", json!({"namespace": "web", "from": "x.txt", "to": "d", "overwrite": true})).0, 409, "a folder is never replaced");
    assert_eq!(post(&h, &ann, "/api/v1/files/move", json!({"namespace": "web", "from": "d", "to": "y.txt", "overwrite": true})).0, 409, "a folder does not replace a file");
    assert_eq!(post(&h, &ann, "/api/v1/files/move", json!({"namespace": "web", "from": "x.txt", "to": "y.txt", "overwrite": true})).0, 200);
    assert_eq!(std::fs::read_to_string(base.join("y.txt")).unwrap(), "X");
    assert_eq!(mv(&h, &ann, "d", "d/inner/deeper"), 400, "a folder cannot move into itself");
    assert_eq!(mv(&h, &ann, "", "z"), 400, "the folder as a whole stays");
    assert_eq!(mv(&h, &ann, "y.txt", ""), 400);
    assert_eq!(post(&h, &ann, "/api/v1/files/move", json!({"namespace": "web", "from": "y.txt", "to": "z", "scope": "shared", "extra": 1})).0, 400, "unknown fields are refused");
}

#[test]
fn move_keeps_to_the_jail_for_traversal_absolute_paths_and_links() {
    let (h, _, ann, _, _) = acme_cloud();
    put(&h, &ann, "ok.txt", "x");
    let outside = h.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "top secret").unwrap();
    for bad in ["../x", "/etc/passwd", "a/../../b", "..", "/"] {
        assert_eq!(mv(&h, &ann, "ok.txt", bad), 400, "to {bad}");
        assert_eq!(mv(&h, &ann, bad, "fine.txt"), 400, "from {bad}");
    }
    assert!(on_disk(&h, "ns/web/people/ann/ok.txt").exists());
    #[cfg(unix)]
    {
        let mine = on_disk(&h, "ns/web/people/ann");
        std::os::unix::fs::symlink(&outside, mine.join("out")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), mine.join("peek")).unwrap();
        for (f, t) in [("out/secret.txt", "stolen.txt"), ("peek", "stolen.txt"), ("out", "o2"), ("ok.txt", "out/in.txt"), ("ok.txt", "peek")] {
            assert_eq!(mv(&h, &ann, f, t), 400, "{f} -> {t}");
        }
        assert!(outside.join("secret.txt").exists() && !outside.join("in.txt").exists() && !mine.join("stolen.txt").exists());
        // the lower layer opens every folder without following links even if a link appears after the path check
        let m = mine.join("swap");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("f"), "f").unwrap();
        std::fs::remove_dir_all(&m).unwrap();
        std::os::unix::fs::symlink(&outside, &m).unwrap();
        assert!(crate::cloudfiles::rename(&mine, "swap/secret.txt", "stolen2.txt", false).is_err());
        assert!(crate::cloudfiles::rename(&mine, "ok.txt", "swap/in.txt", false).is_err());
        assert!(crate::cloudfiles::mkdir(&mine, "swap/new").is_err());
        assert!(!outside.join("in.txt").exists() && !outside.join("new").exists() && outside.join("secret.txt").exists());
    }
}

#[test]
fn move_and_mkdir_follow_the_roles_of_the_namespace() {
    let (h, root, ann, bob, gus) = acme_cloud();
    put(&h, &ann, "v.txt", "v");
    // bob is not in web; gus is a stranger to the tenant
    assert_eq!(mv(&h, &bob, "v.txt", "w.txt"), 404);
    assert_eq!(post(&h, &bob, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "n"})).0, 404);
    assert_eq!(mv(&h, &gus, "v.txt", "w.txt"), 403);
    assert_eq!(post(&h, &gus, "/api/v1/files/mkdir", json!({"path": "n"})).0, 403);
    // a viewer reads but does not write
    assert_eq!(post(&h, &root, "/api/v1/namespaces/web/members", Some(&root).map(|_| json!({"name": "bob", "role": "viewer"})).unwrap()).0, 200);
    assert_eq!(post(&h, &bob, "/api/v1/files/mkdir", json!({"scope": "shared", "namespace": "web", "path": "n"})).0, 403);
    assert_eq!(post(&h, &bob, "/api/v1/files/move", json!({"scope": "shared", "namespace": "web", "from": "a", "to": "b"})).0, 403);
    assert!(!on_disk(&h, "ns/web/shared/n").exists());
}

#[test]
fn mkdir_makes_real_folders_and_the_old_keep_file_still_lists() {
    let (h, _, ann, _, _) = acme_cloud();
    let base = on_disk(&h, "ns/web/people/ann");
    assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "docs/notes"})).0, 200);
    assert!(base.join("docs/notes").is_dir() && !base.join("docs/notes/.keep").exists(), "a folder, not a hidden file");
    assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "docs/notes"})).0, 409);
    put(&h, &ann, "file", "x");
    assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "file"})).0, 409);
    assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": "file/sub"})).0, 400, "a file on the way");
    for bad in ["", "..", "../x", "/abs", "a/../../b", "a\nb"] {
        assert_eq!(post(&h, &ann, "/api/v1/files/mkdir", json!({"namespace": "web", "path": bad})).0, 400, "{bad:?}");
    }
    // old folders made with a .keep file are still listed and deleted as before
    put(&h, &ann, "old/.keep", "");
    let (s, v) = get(&h, &ann, "/api/v1/files?namespace=web&path=old");
    assert_eq!((s, v["entries"][0]["name"].as_str()), (200, Some(".keep")));
}

// -------------------------------------------------------------------------------------------------------------- git

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let o = Command::new("git").env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin").env("HOME", "/nonexistent").env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=T", "-c", "user.email=t@example.org", "-c", "commit.gpgsign=false", "-C"]).arg(dir).args(args).output().expect("git");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn have_git() -> bool {
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}
/// A clone-like repository `repo` in ann's namespace folder with three commits.
fn repo(h: &H) -> std::path::PathBuf {
    let d = on_disk(h, "ns/web/people/ann/repo");
    std::fs::create_dir_all(&d).unwrap();
    git(&d, &["init", "-q", "-b", "main"]);
    for (i, t) in ["one", "two", "three"].iter().enumerate() {
        std::fs::write(d.join("f.txt"), format!("{t}\n")).unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", &format!("commit {t} {i}")]);
    }
    git(&d, &["remote", "add", "origin", "https://alice:hunter2secret@example.org/o/r.git"]);
    d
}
const GQ: &str = "namespace=web&folder=repo";

#[test]
fn git_status_branches_and_log_are_read_only_and_paged() {
    if !have_git() {
        return;
    }
    let (h, _, ann, _, _) = acme_cloud();
    let d = repo(&h);
    std::fs::write(d.join("f.txt"), "changed\n").unwrap();
    std::fs::write(d.join("new.txt"), "n\n").unwrap();
    let before = git(&d, &["status", "--porcelain"]);
    let (s, v) = get(&h, &ann, &format!("/api/v1/git/status?{GQ}"));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["branch"].as_str(), v["changed"].as_u64(), v["untracked"].as_u64(), v["dirty"].as_bool()), (Some("main"), Some(1), Some(1), Some(true)));
    assert!(v["changes"].as_array().unwrap().iter().any(|c| c["path"] == "f.txt" && c["kind"] == "changed"));
    let all = v.to_string();
    assert!(!all.contains("hunter2secret") && !all.contains("alice"), "no credential in the answer: {all}");
    assert_eq!(v["origin"], "https://example.org/o/r.git");
    let (s, b) = get(&h, &ann, &format!("/api/v1/git/branches?{GQ}"));
    assert_eq!((s, b["current"].as_str(), b["branches"][0]["name"].as_str()), (200, Some("main"), Some("main")));
    // the log: newest first, two per page
    let (s, p0) = get(&h, &ann, &format!("/api/v1/git/log?{GQ}&per=2"));
    assert_eq!((s, p0["commits"].as_array().unwrap().len(), p0["more"].as_bool()), (200, 2, Some(true)));
    assert_eq!(p0["commits"][0]["subject"], "commit three 2");
    let (_, p1) = get(&h, &ann, &format!("/api/v1/git/log?{GQ}&per=2&page=1"));
    assert_eq!((p1["commits"].as_array().unwrap().len(), p1["more"].as_bool(), p1["commits"][0]["subject"].as_str()), (1, Some(false), Some("commit one 0")));
    let (_, big) = get(&h, &ann, &format!("/api/v1/git/log?{GQ}&per=100000"));
    assert_eq!(big["per"], 200, "the page size is capped at 200");
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/log?{GQ}&per=abc")).0, 400);
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/log?{GQ}&ref=--output%3D/tmp/x")).0, 400, "an option is not a ref");
    assert_eq!(git(&d, &["status", "--porcelain"]), before, "reading changed nothing");
}

#[test]
fn git_diff_shows_text_refuses_binary_and_caps_the_size() {
    if !have_git() {
        return;
    }
    let (h, _, ann, _, _) = acme_cloud();
    let d = repo(&h);
    std::fs::write(d.join("f.txt"), "changed <b>x</b>\n").unwrap();
    let (s, v) = get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&path=f.txt"));
    assert_eq!(s, 200, "{v}");
    let lines = v["files"][0]["lines"].as_array().unwrap();
    assert!(lines.iter().any(|l| l["kind"] == "add" && l["text"] == "+changed <b>x</b>") && lines.iter().any(|l| l["kind"] == "del" && l["text"] == "-three"));
    // a commit
    let sha = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    let (s, c) = get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&commit={sha}"));
    assert_eq!((s, c["files"][0]["added"].as_u64(), c["files"][0]["removed"].as_u64()), (200, Some(1), Some(1)));
    // binary: refused for a path; listed without content in a commit
    std::fs::write(d.join("pic.bin"), [0u8, 1, 2, 3, 0, 255, 0, 0]).unwrap();
    git(&d, &["add", "pic.bin"]);
    git(&d, &["commit", "-q", "-m", "binary"]);
    std::fs::write(d.join("pic.bin"), [0u8, 9, 9, 9, 0, 255, 0, 1]).unwrap();
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&path=pic.bin")).0, 415);
    let sha2 = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    let (s, c) = get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&commit={sha2}"));
    assert_eq!((s, c["files"][0]["binary"].as_bool(), c["files"][0]["lines"].as_array().map(|a| a.len())), (200, Some(true), Some(0)));
    // too large
    std::fs::write(d.join("f.txt"), "y".repeat(300 * 1024).replace("yyyyyyyy", "yyyyyyy\n")).unwrap();
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&path=f.txt")).0, 413);
    // bad requests
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}")).0, 400);
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&path=f.txt&commit={sha}")).0, 400);
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&path=.git/config")).0, 400);
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&path=../../x")).0, 400);
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/diff?{GQ}&commit=zz;ls")).0, 400);
}

#[test]
fn git_views_keep_to_the_jail_the_roles_and_an_untrusted_configuration() {
    if !have_git() {
        return;
    }
    let (h, _, ann, bob, gus) = acme_cloud();
    let d = repo(&h);
    for route in ["status", "branches", "log"] {
        assert_eq!(get(&h, &ann, &format!("/api/v1/git/{route}?{GQ}")).0, 200, "{route}");
        assert_eq!(get(&h, &bob, &format!("/api/v1/git/{route}?{GQ}")).0, 404, "{route}: bob is not in the namespace");
        assert_eq!(get(&h, &gus, &format!("/api/v1/git/{route}?{GQ}")).0, 403, "{route}: gus is a stranger");
        assert_eq!(get(&h, &ann, &format!("/api/v1/git/{route}?namespace=web")).0, 400, "{route}: the folder is needed");
        for bad in ["../repo", "a/b", "..", "/etc", "re po"] {
            assert_eq!(get(&h, &ann, &format!("/api/v1/git/{route}?namespace=web&folder={bad}")).0, 400, "{route} {bad}");
        }
        assert_eq!(get(&h, &ann, &format!("/api/v1/git/{route}?namespace=web&folder=nothing")).0, 404, "{route}: not a clone");
    }
    // a link as the folder, or as .git, is not followed
    #[cfg(unix)]
    {
        let mine = on_disk(&h, "ns/web/people/ann");
        std::os::unix::fs::symlink(&d, mine.join("alias")).unwrap();
        assert_eq!(get(&h, &ann, "/api/v1/git/status?namespace=web&folder=alias").0, 400);
        std::fs::create_dir_all(mine.join("fake")).unwrap();
        std::os::unix::fs::symlink(d.join(".git"), mine.join("fake/.git")).unwrap();
        assert_eq!(get(&h, &ann, "/api/v1/git/status?namespace=web&folder=fake").0, 404);
    }
    // the person can write into .git with the file routes; a configuration that could run a program is refused, and nothing runs
    let marker = h.dir.join("pwned");
    let cfg = std::fs::read_to_string(d.join(".git/config")).unwrap();
    for evil in [format!("[core]\n\tfsmonitor = touch {}\n", marker.display()), format!("[filter \"x\"]\n\tclean = touch {}\n", marker.display()), format!("[include]\n\tpath = {}\n", marker.display()), "[core]\n\tworktree = /etc\n".to_string()] {
        std::fs::write(d.join(".git/config"), format!("{cfg}{evil}")).unwrap();
        std::fs::write(d.join(".gitattributes"), "* filter=x\n").unwrap();
        for route in ["status", "branches", "log"] {
            assert_eq!(get(&h, &ann, &format!("/api/v1/git/{route}?{GQ}")).0, 400, "{route} with {evil}");
        }
        assert!(!marker.exists(), "nothing ran");
    }
    std::fs::write(d.join(".git/config"), &cfg).unwrap();
    std::fs::remove_file(d.join(".gitattributes")).unwrap();
    assert_eq!(get(&h, &ann, &format!("/api/v1/git/status?{GQ}")).0, 200);
}

#[test]
fn git_scrub_and_configuration_checks() {
    use super::super::gitread::{clone_ok, scrub};
    assert_eq!(scrub("fatal: https://u:p%40ss@host.org/x.git failed"), "fatal: https://host.org/x.git failed");
    assert_eq!(scrub("see https://a@b.org and ssh://git:t@h:22/r"), "see https://b.org and ssh://h:22/r");
    assert_eq!(scrub("no address, 3 @ 4"), "no address, 3 @ 4");
    assert_eq!(scrub("https://host.org/a@b"), "https://host.org/a@b", "an @ after the first slash is part of the path");
    assert!(clone_ok(std::path::Path::new("/nonexistent-dir-xyz")).is_err());
}

// ------------------------------------------------------------------------------------------------------------ flows

#[test]
fn flows_a_run_starts_lists_in_pages_and_is_read_by_id() {
    let (h, _, ann, bob, gus) = acme_cloud();
    let (s, v) = post(&h, &ann, "/api/v1/flows/f1/run", json!({}));
    assert_eq!((s, v["run"].as_str(), v["flow"].as_str()), (200, Some("r1"), Some("f1")), "{v}");
    let (s, v) = post(&h, &ann, "/api/v1/flows/f1/run", json!({"draft": true, "inputs": {"a": 1}}));
    assert_eq!((s, v["run"].as_str()), (200, Some("rd2")));
    assert_eq!(h.call("POST", "/api/v1/flows/f1/run", Some(&ann), None).0, 200, "no body is an empty request");
    assert_eq!(post(&h, &ann, "/api/v1/flows/nope/run", json!({})).0, 404);
    assert_eq!(post(&h, &ann, "/api/v1/flows/f1/run", json!({"inputs": [1]})).0, 400);
    assert_eq!(post(&h, &ann, "/api/v1/flows/f1/run", json!({"approve": true})).0, 400, "unknown fields are refused");
    assert!(audit_has(&h, "flows_run"));
    // runs: 7 in all
    let (s, p0) = get(&h, &ann, "/api/v1/flows/f1/runs?per=3");
    assert_eq!((s, p0["runs"].as_array().unwrap().len(), p0["more"].as_bool(), p0["runs"][0]["id"].as_str()), (200, 3, Some(true), Some("r7")));
    assert_eq!(p0["runs"][0]["blocked"], "asks: delete the old files?", "a waiting question is shown, never answered here");
    let (_, p2) = get(&h, &ann, "/api/v1/flows/f1/runs?per=3&page=2");
    assert_eq!((p2["runs"].as_array().unwrap().len(), p2["more"].as_bool()), (1, Some(false)));
    assert_eq!(get(&h, &ann, "/api/v1/flows/f1/runs?per=1000").1["per"], 100);
    assert_eq!(get(&h, &ann, "/api/v1/flows/nope/runs").0, 404);
    assert_eq!(get(&h, &ann, "/api/v1/flows/f1/runs/r1").1["status"], "succeeded");
    assert_eq!(get(&h, &ann, "/api/v1/flows/f1/runs/r9").0, 404);
    // a stranger to the tenant is refused
    for (m, p) in [("POST", "/api/v1/flows/f1/run"), ("GET", "/api/v1/flows/f1/runs"), ("GET", "/api/v1/flows/f1/runs/r1")] {
        assert_eq!(h.call(m, p, Some(&gus), if m == "POST" { Some(json!({})) } else { None }).0, 403, "{m} {p}");
    }
    let _ = bob;
    // there is no route that answers a question for the person
    assert!(!ROUTES.iter().any(|r| r.path.contains("/flows") && (r.path.contains("approv") || r.path.contains("deny") || r.path.contains("answer"))));
}

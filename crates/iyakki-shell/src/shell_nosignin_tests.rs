//! No automatic sign-in (owner decision 2026-10-28): a person is signed in only by typing their password, which also opens their
//! vault. A remembered name never carries a password; time passing, empty or wrong passwords and a restart never sign anyone in.

use super::tests::*;
use super::*;
use std::path::{Path, PathBuf};

fn machine(tag: &str) -> (ShellEnv, String) {
    crate::apps::tests::isolated_home();
    let (e, _h) = env(tag);
    {
        let mut st = e.store.lock().unwrap();
        st.create_first_admin("root", "rootpass12").unwrap();
    }
    (e, "root".into())
}

#[test]
fn a_machine_with_an_account_and_a_remembered_name_starts_at_the_sign_in_screen_and_stays_there() {
    let (e, user) = machine("nosign1");
    remember_user(&e, &user);
    let svc = e.services.clone();
    let mut sh = Shell::new(e);
    assert_eq!(sh.screen_name(), "login");
    // the name is remembered, the password never is
    let Screen::Login(f) = &sh.screen else { panic!("login form") };
    assert_eq!(f.fields[0].value, "root");
    assert!(f.fields[1].value.is_empty(), "no password is prefilled");
    for _ in 0..600 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.screen_name(), "login", "time passing signs nobody in");
    assert!(sh.session.is_none());
    assert!(!svc.user_vault_unlocked("root"), "the vault stays locked until the password is typed");
}

#[test]
fn an_empty_or_wrong_password_signs_nobody_in_and_the_right_one_signs_in_and_opens_the_vault() {
    let (e, user) = machine("nosign2");
    remember_user(&e, &user);
    let svc = e.services.clone();
    let mut sh = Shell::new(e);
    // Enter on the empty password field
    key(&mut sh, Key::Enter);
    assert!(sh.session.is_none() && sh.screen_name() == "login");
    login(&mut sh, "root", "not-the-password");
    assert!(sh.session.is_none() && sh.screen_name() == "login");
    assert!(!svc.user_vault_unlocked("root"));
    login(&mut sh, "root", "rootpass12");
    assert!(sh.session.is_some(), "the typed password signs in");
    assert!(svc.user_vault_unlocked("root"), "and the same password opens the vault");
}

/// A file's production code: everything before its first test module.
fn production(text: &str) -> &str {
    let cut = ["mod tests {", "\n#[cfg(test)]\n#[path", "\n#[cfg(test)]\nmod "].iter().filter_map(|m| text.find(m)).min().unwrap_or(text.len());
    &text[..cut]
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn nothing_in_the_program_or_the_image_scripts_signs_in_without_the_password() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for c in ["iyakki-shell", "iyakki-remote", "iyakki-core", "iyakki-gui", "iyakki-agent", "iyakki-core-api"] {
        rust_files(&root.join("crates").join(c).join("src"), &mut files);
    }
    let mut calls = Vec::new();
    let mut overrides = Vec::new();
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.contains("test") || p.to_string_lossy().contains("/tests/") {
            continue;
        }
        let text = std::fs::read_to_string(p).unwrap();
        // production code only: the test modules of a file come after its first #[cfg(test)]
        let prod = production(&text);
        if prod.contains("sign_in_checked(") && !name.contains("shell.rs") {
            calls.push(p.strip_prefix(&root).unwrap_or(p).display().to_string());
        }
        let low = prod.to_lowercase();
        if ["autologin", "auto_login", "auto-login", "autosignin", "auto_signin", "auto-sign-in"].iter().any(|w| low.contains(w)) {
            overrides.push(p.strip_prefix(&root).unwrap_or(p).display().to_string());
        }
    }
    // the only caller outside the shell is the browser sign-in, which checks the password with the same store first
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].ends_with("iyakki-remote/src/screen.rs"), "{calls:?}");
    let screen = std::fs::read_to_string(root.join("crates/iyakki-remote/src/screen.rs")).unwrap();
    assert!(screen.find("store.authenticate(user.trim(), password)").unwrap() < screen.find("sign_in_checked(").unwrap(), "the password is checked first");
    // inside the shell, sign_in is reached from the typed forms only
    let shell = std::fs::read_to_string(root.join("crates/iyakki-shell/src/shell.rs")).unwrap();
    let prod = production(&shell);
    let inner: Vec<&str> = prod.lines().filter(|l| l.contains("self.sign_in(")).collect();
    assert_eq!(inner.len(), 3, "submit_login, submit_setup and sign_in_checked: {inner:?}");
    assert!(overrides.is_empty(), "no auto sign-in setting exists: {overrides:?}");
    // the image scripts and the boot configuration do not log anyone in either
    let mut scripts = Vec::new();
    for d in ["dist/buildroot", "dist/webview"] {
        for e in std::fs::read_dir(root.join(d)).into_iter().flatten().flatten() {
            if e.path().is_file() {
                scripts.push(e.path());
            }
        }
    }
    for p in scripts {
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        let low = t.to_lowercase();
        assert!(!low.contains("autologin") && !low.contains("auto-login") && !low.contains("--autologin"), "{} signs in on its own", p.display());
    }
}

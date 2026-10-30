//! The vault's lock state is said in one place in Settings, in plain words; waiting passwords are named without any value.

use super::vaultstate_page::{status_words, waiting_line};

#[test]
fn a_locked_personal_vault_says_how_to_open_it_and_an_open_one_says_so() {
    let (t, ok) = status_words(false, true, false);
    assert!(!ok && t.starts_with("LOCKED") && t.contains("sign in with your password") && t.contains("press u"), "{t}");
    let (t, ok) = status_words(false, true, true);
    assert!(ok && t.contains("unlocked"), "{t}");
    let (t, ok) = status_words(true, true, false);
    assert!(!ok && t.starts_with("LOCKED") && t.contains("admin"), "{t}");
    assert!(status_words(true, false, false).0.contains("not created yet"));
}

#[test]
fn passwords_waiting_for_a_locked_vault_are_named_there_and_only_while_it_is_locked() {
    let d = std::env::temp_dir().join(format!("iyakki-vs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(waiting_line(&d, "admin", false), None, "nothing waits");
    iyakki_agent::vaultpending::add(&d, iyakki_agent::quickinstall::pending_of(iyakki_agent::quickinstall::Item::ClickHouse, "admin")).unwrap();
    let l = waiting_line(&d, "admin", false).expect("it is named");
    assert!(l.contains("ClickHouse password") && l.contains("once you unlock it"), "{l}");
    assert_eq!(waiting_line(&d, "admin", true), None, "an open vault shows no warning");
    assert_eq!(waiting_line(&d, "someone-else", false), None, "another person's passwords are not listed");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn only_the_vault_page_reports_a_locked_vault_as_a_status() {
    // the status words come from one function; no other Settings page draws a "Status: ... LOCKED" line of its own
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/apps");
    let mut offenders = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !(name.starts_with("settings") && name.ends_with(".rs")) || name.ends_with("_tests.rs") || name == "settings_vaultstate.rs" {
            continue;
        }
        let text = std::fs::read_to_string(e.path()).unwrap();
        // settings.rs holds the Vault page's drawing code and its own test module; cut the tests off
        let prod = text.split("#[cfg(test)]").next().unwrap_or("");
        if prod.lines().any(|l| l.contains("\"LOCKED") || l.contains("LOCKED (") || l.contains("LOCKED —")) {
            offenders.push(name);
        }
    }
    let page = std::fs::read_to_string(dir.join("settings.rs")).unwrap();
    assert!(page.contains("vaultstate_page::status_words(true,") && page.contains("vaultstate_page::status_words(false,"), "the Vault page uses the one wording");
    assert!(offenders.is_empty(), "LOCKED is worded only in settings_vaultstate.rs: {offenders:?}");
}

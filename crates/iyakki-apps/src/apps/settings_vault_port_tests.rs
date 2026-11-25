//! Settings -> Vault and External secret providers on a fake port: the page shows the system vault to administrators and the person's own to
//! everyone, the lock key locks and is audited, the unlock and new-secret forms open, a provider is checked, its token is stored and the provider
//! saved and audited in that order, a refusal stops it, and a test runs the port's job. No sleeps; the test job is awaited on its channel.

use super::account_port::{FakeAcct, SecretProvidersPort, VaultPort};
use super::*;
use crate::apps::tests::cx_with_admin_store;

fn app(tab: Tab) -> SettingsApp {
    let cx = cx_with_admin_store("vault-fake");
    let mut a = SettingsApp::new(&cx);
    a.go_to(tab);
    a
}

fn draw(a: &mut SettingsApp, p: &dyn VaultPort, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    a.render_vault(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, p);
    g
}

#[test]
fn an_administrator_starts_on_the_system_vault_and_tab_switches_to_the_personal_one() {
    let p = FakeAcct::new(true);
    *p.system_secrets.lock().unwrap() = vec!["a".into(), "b".into()];
    *p.user_secrets.lock().unwrap() = vec!["mine".into()];
    let mut a = app(Tab::Vault);
    assert_eq!(a.vault_page_names(&p).unwrap(), vec!["a", "b"]);
    a.handle_vault(&Key::Tab, &p);
    assert_eq!(a.vault_page_names(&p).unwrap(), vec!["mine"]);
    // a person who is not an administrator never sees the system vault, Tab does nothing for them
    let q = FakeAcct::new(false);
    *q.system_secrets.lock().unwrap() = vec!["a".into()];
    *q.user_secrets.lock().unwrap() = vec!["mine".into()];
    let mut b = app(Tab::Vault);
    b.handle_vault(&Key::Tab, &q);
    assert_eq!(b.vault_page_names(&q).unwrap(), vec!["mine"]);
}

#[test]
fn the_lock_key_locks_the_system_vault_and_is_audited_only_there() {
    let p = FakeAcct::new(true);
    let mut a = app(Tab::Vault);
    a.handle_vault(&Key::Char('l'), &p);
    assert_eq!(p.locked_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(p.system_vault_state(), (true, false));
    assert_eq!(p.audited(), vec![("vault.system.lock".to_string(), String::new())]);
    // on the personal vault (or for a non-administrator) the key does nothing
    let q = FakeAcct::new(false);
    let mut b = app(Tab::Vault);
    b.handle_vault(&Key::Char('l'), &q);
    assert_eq!(q.locked_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn the_forms_and_the_delete_question_open_for_the_right_vault() {
    let p = FakeAcct::new(true);
    *p.system_secrets.lock().unwrap() = vec!["tok".into()];
    let mut a = app(Tab::Vault);
    a.handle_vault(&Key::Char('n'), &p);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::NewSecret(true), _))));
    a.modal = None;
    a.handle_vault(&Key::Char('d'), &p);
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::Secret(true, n), q)) if n == "tok" && q.contains("tok")));
    a.modal = None;
    a.handle_vault(&Key::Char('f'), &p);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::Keyfile, _))));
    // the personal vault: unlock only while it is locked
    a.modal = None;
    a.handle_vault(&Key::Tab, &p);
    a.handle_vault(&Key::Char('u'), &p);
    assert!(a.modal.is_none(), "it is open: nothing to unlock");
    p.user_unlocked.store(false, std::sync::atomic::Ordering::SeqCst);
    a.handle_vault(&Key::Char('u'), &p);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::UnlockVault, _))));
    a.modal = None;
    a.handle_vault(&Key::Char('R'), &p);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::ResetPersonal, _))));
}

#[test]
fn the_page_draws_the_state_of_the_port_at_several_sizes_and_names_what_waits() {
    for (admin, unlocked) in [(true, true), (true, false), (false, false)] {
        let p = FakeAcct::new(admin);
        *p.vault_state.lock().unwrap() = (true, unlocked);
        p.user_unlocked.store(unlocked, std::sync::atomic::Ordering::SeqCst);
        *p.slots.lock().unwrap() = vec!["admin".into()];
        *p.waiting.lock().unwrap() = Some("Waiting for this vault: ClickHouse password.".into());
        let mut a = app(Tab::Vault);
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
            let g = draw(&mut a, &p, w, h);
            assert_eq!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Tab)), admin, "{w}x{h}: the switch between the vaults");
        }
    }
}

// ---- external secret providers ----

fn form_for(addr: &str, token: &str) -> Form {
    let mut f = SettingsApp::secret_provider_form();
    f.set_value(0, "corp-vault");
    f.set_value(2, addr);
    f.set_value(3, token);
    f
}

#[test]
fn a_provider_is_checked_then_its_token_stored_then_it_is_saved_and_audited() {
    let p = FakeAcct::new(true);
    let mut a = app(Tab::SecretProviders);
    a.save_secret_provider(&p, &form_for("https://vault.example.com:8200", "s3cret")).unwrap();
    assert_eq!(p.secret_providers().len(), 1);
    assert_eq!(p.secret_providers()[0].0, "corp-vault");
    assert_eq!(p.system_secrets.lock().unwrap().clone(), vec!["corp-vault-token".to_string()]);
    assert_eq!(p.audited(), vec![("vault.system.set".to_string(), "corp-vault-token".to_string()), ("settings.secret_provider.save".to_string(), "corp-vault".to_string())]);
}

#[test]
fn a_bad_alias_a_missing_token_or_an_invalid_provider_stores_nothing() {
    let p = FakeAcct::new(true);
    let mut a = app(Tab::SecretProviders);
    let mut f = form_for("https://vault.example.com:8200", "tok");
    f.set_value(0, "Bad Alias");
    assert!(a.save_secret_provider(&p, &f).is_err_and(|e| e.contains("Alias")));
    assert!(a.save_secret_provider(&p, &form_for("https://vault.example.com:8200", "")).is_err_and(|e| e.contains("token")));
    assert!(a.save_secret_provider(&p, &form_for("not an address", "tok")).is_err());
    assert!(p.system_secrets.lock().unwrap().is_empty(), "no token is stored before the provider is valid");
    assert!(p.secret_providers().is_empty());
    assert!(p.audited().is_empty());
}

#[test]
fn a_refused_store_keeps_the_provider_out_and_the_reason_is_returned() {
    let p = FakeAcct::new(true);
    *p.refuse.lock().unwrap() = Some("The system vault is locked.".into());
    let mut a = app(Tab::SecretProviders);
    let e = a.save_secret_provider(&p, &form_for("https://vault.example.com:8200", "tok")).unwrap_err();
    assert!(e.contains("locked"));
    assert!(p.secret_providers().is_empty());
}

#[test]
fn the_keys_open_the_form_ask_before_removing_and_only_for_administrators() {
    let p = FakeAcct::new(true);
    let mut a = app(Tab::SecretProviders);
    a.save_secret_provider(&p, &form_for("https://vault.example.com:8200", "tok")).unwrap();
    a.handle_secret_providers(&Key::Char('a'), &p);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::SecretProvider, _))));
    a.modal = None;
    a.handle_secret_providers(&Key::Char('d'), &p);
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::SecretProvider(n), _)) if n == "corp-vault"));
    // not an administrator: nothing opens, and the refusal is audited
    let q = FakeAcct::new(false);
    q.save_provider("x", p.secret_providers()[0].1.clone()).unwrap();
    let mut b = app(Tab::SecretProviders);
    b.handle_secret_providers(&Key::Char('a'), &q);
    b.handle_secret_providers(&Key::Char('d'), &q);
    assert!(b.modal.is_none());
    assert!(q.audited().iter().all(|l| l.0 == "settings.denied") && !q.audited().is_empty());
}

#[test]
fn the_test_key_runs_the_ports_job_and_the_answer_names_the_provider() {
    let p = FakeAcct::new(true);
    let mut a = app(Tab::SecretProviders);
    a.save_secret_provider(&p, &form_for("https://vault.example.com:8200", "tok")).unwrap();
    a.handle_secret_providers(&Key::Char('t'), &p);
    let job = a.job.take().expect("a job runs");
    match job.rx.recv().expect("it answers") {
        JobOut::Msg(Ok(m)) => assert_eq!(m, "corp-vault: reachable, token accepted"),
        _ => panic!("expected a message"),
    }
    assert_eq!(p.tested.lock().unwrap().clone(), vec!["corp-vault".to_string()]);
    *p.refuse.lock().unwrap() = Some("connection refused".into());
    a.handle_secret_providers(&Key::Char('t'), &p);
    match a.job.take().unwrap().rx.recv().unwrap() {
        JobOut::Msg(Err(e)) => assert_eq!(e, "corp-vault: connection refused"),
        _ => panic!("expected a refusal"),
    }
}

#[test]
fn the_list_draws_the_providers_of_the_port_at_several_sizes() {
    let p = FakeAcct::new(true);
    let mut a = app(Tab::SecretProviders);
    a.save_secret_provider(&p, &form_for("https://vault.example.com:8200", "tok")).unwrap();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_secret_providers(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
}

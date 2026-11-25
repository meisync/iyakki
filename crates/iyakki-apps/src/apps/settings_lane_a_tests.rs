//! Settings tests of lane ST-A moved out of `settings.rs` (S0).

use super::*;
use super::tests::*;

#[test]
fn non_admins_cannot_change_system_settings_and_it_is_audited() {
    let cx = user("set4");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    assert_eq!(a.tabs(), &[Tab::Overview, Tab::Account, Tab::Notifications, Tab::Vault, Tab::Backup, Tab::EncHome, Tab::Models, Tab::Skills, Tab::Rag, Tab::Assistant, Tab::GeminiVoice, Tab::Voice, Tab::Radio, Tab::Browser, Tab::HostAccess, Tab::AppEvents, Tab::Shares, Tab::ScreenSaver, Tab::HomeLayout, Tab::Views, Tab::Appearance, Tab::Accessibility, Tab::Time, Tab::Power, Tab::Computer, Tab::GitKeys], "no secret-provider tab for users; everyone has Notifications, Screen saver, Time, Power, Accessibility, This computer and Home layout; whoever has the AI IDE has Git and SSH keys");
    for k in ['a', 'e', 'd', 'k', 'x'] {
        key(&mut a, &cx, Key::Char(k));
        assert!(!a.has_modal(), "{k}");
    }
    assert_eq!(a.status().unwrap().1, false);
    assert!(status(&a).contains("administrators"));
    goto(&mut a, &cx, Tab::Rag);
    key(&mut a, &cx, Key::Char(']'));
    key(&mut a, &cx, Key::Char('a'));
    assert!(!a.has_modal());
    assert_eq!(cx.settings().lock().unwrap().get().rag.top_k, 5, "unchanged");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.denied")));
    // even a hand-built admin form submitted by a non-admin is refused at save time
    let mut forced = SettingsApp::new(&cx).on(Tab::Models);
    let form = SettingsApp::provider_form(&cx, None).text("x", "", "");
    forced.modal = Some(Modal::Form(FormKind::Provider(None), form));
    key(&mut forced, &cx, Key::Enter);
    assert!(cx.settings().lock().unwrap().get().providers.is_empty());
}

#[test]
fn personal_vault_is_private_to_the_user() {
    let cx = user("set5");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Vault);
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "github-token");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "ghp_personal_secret");
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{:?}", a.status());
    assert_eq!(cx.vault().user_names("amy").unwrap(), vec!["github-token"]);
    // the system vault is not reachable from a user session at all
    assert!(cx.vault().system_names().is_err() || !cx.session.policy.manage_settings);
    key(&mut a, &cx, Key::Char('l')); // lock is a system-vault action: ignored for users
    assert!(cx.vault().user_vault_unlocked("amy"));
    // delete with confirmation
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('n')); // anything but y cancels
    assert_eq!(cx.vault().user_names("amy").unwrap().len(), 1);
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(cx.vault().user_names("amy").unwrap().is_empty());
}

#[test]
fn system_vault_secrets_lock_and_keyfile() {
    let (cx, d) = admin("set6");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Vault);
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "slack-webhook");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "https://hooks.example/abc");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(cx.vault().system_names().unwrap(), vec!["slack-webhook"]);
    // invalid names are rejected in the form
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "Bad Name");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "v");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "form stays open with the reason");
    key(&mut a, &cx, Key::Esc);
    // key file
    key(&mut a, &cx, Key::Char('f'));
    for _ in 0..80 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, &d.join("unattended.key").display().to_string());
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{:?}", a.status());
    assert!(d.join("unattended.key").exists());
    // lock now
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(cx.services.system_vault_state(), (true, false));
    let mut g = Gfx::new(1100, 650);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
}

#[test]
fn external_secret_provider_roundtrip() {
    let (cx, _) = admin("set8");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::SecretProviders);
    key(&mut a, &cx, Key::Char('a'));
    typ(&mut a, &cx, "corp");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Enter); // type: HashiCorp (default)
    typ(&mut a, &cx, "http://bao:8200");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "s.root-token");
    for _ in 0..3 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(!a.has_modal(), "{:?}", a.status());
    assert_eq!(&*cx.secrets().resolve("vault:corp-token", None).unwrap(), "s.root-token");
    key(&mut a, &cx, Key::Char('t'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("reachable"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(cx.settings().lock().unwrap().get().secret_providers.is_empty());
}

#[test]
fn password_change_rekeys_the_personal_vault() {
    let (cx, _) = admin("set9");
    // root's account exists in the store with password rootpass12
    cx.vault().user_set("root", "tok", "personal").unwrap();
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Account);
    key(&mut a, &cx, Key::Char('p'));
    typ(&mut a, &cx, "wrong-old-pw");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "brand-new-pw1");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "brand-new-pw1");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "wrong current password keeps the form open");
    // fix the first field and retry (focus is on the last field; one Tab wraps to the first)
    key(&mut a, &cx, Key::Tab);
    for _ in 0..20 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "rootpass12");
    for _ in 0..2 {
        key(&mut a, &cx, Key::Enter);
    }
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal(), "{:?}", a.status());
    assert!(cx.store.lock().unwrap().authenticate("root", "brand-new-pw1").is_ok());
    cx.services.on_logout("root", true);
    assert_eq!(cx.services.on_login("root", "brand-new-pw1", true).user_vault, iyakki_shared::services::VaultState::Unlocked);
    assert_eq!(cx.vault().user_names("root").unwrap(), vec!["tok"], "personal secrets survive the change");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("account.password")));
}

#[test]
fn mismatched_new_passwords_are_rejected() {
    let (cx, _) = admin("set10");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Account);
    key(&mut a, &cx, Key::Char('p'));
    typ(&mut a, &cx, "rootpass12");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "new-password-1");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "new-password-2");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal());
    assert!(cx.store.lock().unwrap().authenticate("root", "rootpass12").is_ok(), "nothing changed");
}

#[test]
fn admin_makes_an_empty_disk_the_data_disk_after_confirming() {
    let Some((env, root)) = machine("empty") else { return eprintln!("mke2fs missing; skipping") };
    let (cx, _) = admin("storage1");
    let mut a = SettingsApp::new(&cx).with_storage(env);
    assert!(a.tabs().contains(&Tab::Storage));
    goto(&mut a, &cx, Tab::Storage);
    assert_eq!(a.list_len(&cx), 3);
    select_disk(&mut a, &cx, "vdb");
    key(&mut a, &cx, Key::Char('m'));
    assert!(a.has_modal());
    typ(&mut a, &cx, "vda");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a wrong name keeps the confirmation open");
    assert_eq!(iyakki_core_api::device_types::storage::probe(&root.join("dev/vdb")).unwrap(), iyakki_core_api::device_types::storage::Content::Empty, "nothing happened yet");
    for _ in 0..3 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "vdb");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(status(&a).contains("vdb is now the IYAKKI data disk") && status(&a).contains("Reboot"), "{}", status(&a));
    assert_eq!(iyakki_core_api::device_types::storage::probe(&root.join("dev/vdb")).unwrap(), iyakki_core_api::device_types::storage::Content::Iyakki);
    assert!(a.devices.iter().find(|d| d.name == "vdb").unwrap().content == iyakki_core_api::device_types::storage::Content::Iyakki, "the list refreshed");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("storage.persist") && l.contains("vdb")));
}

#[test]
fn in_the_window_disks_are_virtual_made_and_deleted_from_the_storage_tab() {
    let Some((_, _)) = machine("virt-probe") else { return eprintln!("mke2fs missing; skipping") };
    let root = std::env::temp_dir().join(format!("iyakki-ui-virtual-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let v = iyakki_core_api::device_types::storage::virt::VirtualDisks::open(&root).unwrap();
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("users.json"), "{}").unwrap();
    let env = StorageEnv { roots: v.roots(), mounter: Arc::new(v.mounter()), mke2fs: "/usr/sbin/mke2fs".into(), scratch: root.clone(), data_dir: data, virtual_disks: Some(v) };
    let (cx, _) = admin("storage-virtual");
    let mut a = SettingsApp::new(&cx).with_storage(env);
    goto(&mut a, &cx, Tab::Storage);
    assert_eq!(a.list_len(&cx), 0, "no real drive is listed");
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.has_modal());
    for _ in 0..2 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "banana");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "a size that is not a size is refused and the question stays");
    for _ in 0..6 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "128M");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("Made the virtual disk vda"), "{}", status(&a));
    assert_eq!(a.list_len(&cx), 1);
    assert!(root.join("dev/vda").exists());
    // the disk can become the data disk with the same confirmation as a real one
    select_disk(&mut a, &cx, "vda");
    key(&mut a, &cx, Key::Char('m'));
    typ(&mut a, &cx, "vda");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(status(&a).contains("vda is now the IYAKKI data disk"), "{}", status(&a));
    // and be deleted again, by typing its name
    key(&mut a, &cx, Key::Char('x'));
    typ(&mut a, &cx, "vdb");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "the wrong name keeps the disk");
    for _ in 0..3 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "vda");
    key(&mut a, &cx, Key::Enter);
    assert!(status(&a).contains("Removed vda"), "{}", status(&a));
    assert_eq!(a.list_len(&cx), 0);
    assert!(!root.join("dev/vda").exists());
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("storage.virtual.add")) && cx.audit.recent(10).iter().any(|l| l.contains("storage.virtual.remove")));
}

#[test]
fn storage_refuses_the_boot_medium_and_needs_erase_for_disks_with_data() {
    let Some((env, root)) = machine("guard") else { return eprintln!("mke2fs missing; skipping") };
    let (cx, _) = admin("storage2");
    let mut a = SettingsApp::new(&cx).with_storage(env);
    goto(&mut a, &cx, Tab::Storage);
    select_disk(&mut a, &cx, "sda");
    key(&mut a, &cx, Key::Char('m'));
    assert!(!a.has_modal() && status(&a).contains("booted from"), "{}", status(&a));
    // a disk with data asks for a second, explicit confirmation
    select_disk(&mut a, &cx, "vdc");
    key(&mut a, &cx, Key::Char('m'));
    assert!(a.has_modal());
    typ(&mut a, &cx, "vdc");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "erase");
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal(), "lower-case erase is not enough");
    assert_eq!(iyakki_core_api::device_types::storage::probe(&root.join("dev/vdc")).unwrap(), iyakki_core_api::device_types::storage::Content::Other("data of unknown type".into()));
    for _ in 0..5 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "ERASE");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert_eq!(iyakki_core_api::device_types::storage::probe(&root.join("dev/vdc")).unwrap(), iyakki_core_api::device_types::storage::Content::Iyakki);
    assert_eq!(iyakki_core_api::device_types::storage::probe(&root.join("dev/vdb")).unwrap(), iyakki_core_api::device_types::storage::Content::Empty, "other disks untouched");
}

#[test]
fn storage_is_admin_only_and_renders() {
    let ucx = user("storage3");
    assert!(!SettingsApp::new(&ucx).tabs().contains(&Tab::Storage));
    // a hand-built request from a non-admin is refused at save time
    let mut forced = SettingsApp::new(&ucx);
    let f = Form::new("x").text("Type vdb", "vdb", "");
    forced.modal = Some(Modal::Form(FormKind::Persist("vdb".into(), false), f));
    key(&mut forced, &ucx, Key::Enter);
    assert!(!forced.is_busy());
    let (cx, _) = admin("storage4");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Storage);
    let mut g = Gfx::new(1100, 650);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
    if let Some((env, _)) = machine("render") {
        let mut b = SettingsApp::new(&cx).with_storage(env);
        goto(&mut b, &cx, Tab::Storage);
        g.clear(rgb(0, 0, 0));
        b.render(&mut g, R::new(0.0, 72.0, 1100.0, 578.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.label == "/dev/vdb"), "disks are clickable rows");
    }
}

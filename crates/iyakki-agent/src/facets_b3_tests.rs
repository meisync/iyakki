use super::*;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn services(tag: &str) -> (Arc<Services>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("iyakki-facetsb3-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap(), dir)
}

#[test]
fn the_add_on_facet_reads_what_the_agent_detects() {
    let (svc, dir) = services("avaddon");
    assert_eq!(RealAvAddon.detect(svc.dir()), crate::avaddon::detect(svc.dir()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_chosen_policy_the_lock_seals_nothing_and_the_unlock_opens_nothing() {
    let (svc, dir) = services("lock");
    assert_eq!(RealProfileLock.on_screen_lock(svc.dir(), "ann"), Ok(false));
    assert_eq!(RealProfileLock.on_screen_unlock(svc.dir(), "ann", Some("not a key")), Ok(false));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_client_media_facet_knows_no_device_that_was_never_enrolled() {
    let (svc, dir) = services("media");
    assert!(RealClientMedia.device_holds(svc.dir(), "nobody").is_empty());
    let e = RealClientMedia.set_permission(svc.dir(), "nobody", "teleport", true, "ann", 1).unwrap_err();
    assert!(e.contains("teleport"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_device_tools_wrapper_adds_the_device_tools_around_what_it_is_given() {
    let (svc, dir) = services("wrap");
    let inner: Arc<dyn Tools> = Arc::new(iyakki_shared::agent::NoTools);
    let w = RealToolWraps.wrap_device_tools(inner, svc.dir(), "ann", true, Arc::new(|_, _| {}));
    assert!(w.defs().iter().any(|d| d.spec.name == "client_devices"), "the wrapper brings client_devices");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fresh_install_keeps_no_computer_page_no_wizards_and_no_vpn_profile() {
    let (svc, dir) = services("fresh");
    let b3 = svc.b3();
    assert!(b3.computer_page().is_none(), "no computer is attached before the real start");
    assert!(b3.computer_set_sharing(iyakki_shared::hwprofile::Category::Sound, true).is_err());
    let be = b3.wizard_backend("ann", Arc::new(|_, _| {}));
    assert!(be.installed().is_empty() && be.pending().is_empty());
    let vpn = b3.vpn_manager("ann", Arc::new(|_, _| {}));
    assert!(vpn.profiles().is_empty() && vpn.summary().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_vpn_handle_answers_like_the_manager_it_holds() {
    let (svc, dir) = services("vpn");
    let m = crate::vpn_mgr::Manager::shared(&svc, "ann", Arc::new(|_, _| {}));
    let h = VpnHandle::arc(m.clone());
    assert_eq!(h.profiles().len(), m.profiles().len());
    assert_eq!(h.guard_line(), m.guard_line());
    assert!(h.set_enabled("v9", true, 1).is_err(), "no such profile");
    let _ = std::fs::remove_dir_all(dir);
}

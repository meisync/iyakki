// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup -> Full web engine (F-BROW-40): the card is on the page, Install shows what it is and what it may do before anything
//! starts, and nothing is started until the confirm button. Fakes only: no store, no download.
use iyakki_agent::facets_h::RealServices as _;
use super::*;
use crate::apps::store_port::CxPort;
use iyakki_agent::quickinstall as qi;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_sec::policy::Role;

fn rig(tag: &str, role: Role) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    let mut f = Facts::default();
    for it in Item::ALL {
        f.presence.push((it, Presence::Missing));
    }
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(Machine { ram_mb: 16_000, ..Default::default() });
    a.setup.set_facts(f);
    a.setup.launch = Some(iyakki_tile_api::store_qi::Launch { exe: "/usr/bin/true".into(), data: cx.data_dir().join("qi") });
    a.handle(&Event::Tab(4), &cx);
    assert!(a.section == Section::Setup);
    a.setup.enter(&CxPort(&cx));
    (a, cx)
}

#[test]
fn install_shows_the_card_first_and_nothing_starts_before_the_confirm_button() {
    let (mut a, cx) = rig("bpkg-confirm", Role::Admin);
    let idx = Item::ALL.iter().position(|x| *x == Item::BrowserPackage).unwrap();
    a.setup.open_popup(Item::BrowserPackage);
    a.handle(&Event::Tab(PRIMARY + idx), &cx);
    let c = a.setup.confirm.as_ref().expect("the card opens");
    assert_eq!(c.title, "Full web engine: read, then confirm");
    let text = c.review.join("\n");
    for must in ["not in the base image", "store you pinned", "signed description, size and checksum", "network and sound, nothing else", "Remove takes it away"] {
        assert!(text.contains(must), "missing {must}: {text}");
    }
    assert!(qi::install_jobs(&cx.services.real().jobs, &cx.session.user).is_empty(), "no job before the button");
}

#[test]
fn the_item_has_a_name_an_id_a_licence_line_and_belongs_to_a_group() {
    crate::apps::store_port::install_table();
    let i = Item::BrowserPackage;
    assert_eq!(Item::parse("browser-package"), Some(i));
    assert!(!i.title().is_empty() && !i.licence().is_empty() && !i.about().is_empty());
    assert!(browser_package_review().len() >= 5);
}

struct StoreUp(Vec<u8>);
impl iyakki_proxy::Upstream for StoreUp {
    fn request(&self, _: &str, url: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        if url == format!("https://store.example/browser/{}/manifest.json", std::env::consts::ARCH) {
            Ok(iyakki_proxy::UpstreamResponse { status: 200, body: self.0.clone() })
        } else {
            Err(format!("{url} is not on this store"))
        }
    }
}

#[test]
fn the_card_shows_the_version_and_size_read_from_the_pinned_store_off_the_screens_thread_and_says_so_when_it_cannot() {
    use iyakki_shared::store_catalog::{Config, SourceCfg, SourceKind};
    let (mut a, mut cx) = rig("bpkg-store-card", Role::Admin);
    let key = iyakki_agent::pkg::signing_key_from_hex(&"04".repeat(32)).unwrap();
    let m = iyakki_agent::browserpkg::Manifest { version: "2026.11-3".into(), size: 90 << 20, sha256: "0".repeat(64), arch: std::env::consts::ARCH.into() };
    let manifest = iyakki_agent::browserpkg::manifest_doc(&m, &key).to_string().into_bytes();
    let svc = crate::apps::tests::temp_dir("bpkg-store-card-svc");
    cx.services = crate::apps::tests::services_in(&svc, std::sync::Arc::new(StoreUp(manifest)));
    let mut cfg = Config::default();
    cfg.sources = vec![SourceCfg { id: "s".into(), name: "s".into(), kind: SourceKind::Url, location: "https://store.example/catalog.json".into(), enabled: true, pin: iyakki_agent::pkg::public_key_hex(&key) }];
    cfg.save(&cx.data_dir().join("store")).unwrap();
    let idx = Item::ALL.iter().position(|x| *x == Item::BrowserPackage).unwrap();
    a.setup.open_popup(Item::BrowserPackage);
    a.handle(&Event::Tab(PRIMARY + idx), &cx);
    // the card opens at once with the reading line; the screen is not held by the network
    assert_eq!(a.setup.confirm.as_ref().unwrap().review[0], PKG_CARD_READING);
    while a.setup.confirm.as_ref().unwrap().review[0] == PKG_CARD_READING {
        a.setup.tick(&CxPort(&cx));
        std::thread::yield_now();
    }
    let first = a.setup.confirm.as_ref().unwrap().review[0].clone();
    assert!(first.contains("2026.11-3") && first.contains("90 MB"), "{first}");
    assert!(qi::install_jobs(&cx.services.real().jobs, &cx.session.user).is_empty(), "reading the card starts nothing");
    // a store that cannot be read: the reason on the card, and the person may still confirm
    cfg.sources[0].location = "https://nowhere.example/catalog.json".into();
    cfg.save(&cx.data_dir().join("store")).unwrap();
    a.setup.confirm = None;
    a.setup.open_popup(Item::BrowserPackage);
    a.handle(&Event::Tab(PRIMARY + idx), &cx);
    while a.setup.confirm.as_ref().unwrap().review[0] == PKG_CARD_READING {
        a.setup.tick(&CxPort(&cx));
        std::thread::yield_now();
    }
    let first = a.setup.confirm.as_ref().unwrap().review[0].clone();
    assert!(first.contains("could not be read") && first.contains("still confirm"), "{first}");
}

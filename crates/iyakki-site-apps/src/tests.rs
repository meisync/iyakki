use super::*;

const MANIFEST: &str = r#"{"name":"Shop","short_name":"S","start_url":"/home?utm=1","icons":[{"src":"i/48.png","sizes":"48x48"},{"src":"/i/192.png","sizes":"192x192"},{"src":"/i/big.png","sizes":"1024x1024"}]}"#;

/// A folder of its own for one test (no two tests, no two runs, share one).
fn scratch(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("iyakki-site-apps-{}-{}-{tag}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn page() -> PageInfo {
    PageInfo { url: "https://www.shop.example/a/b?q=1#x".into(), title: "Shop - deals".into(), manifest: Some(MANIFEST.into()), manifest_url: Some("https://www.shop.example/m.json".into()), favicon: Some("/fav.ico".into()) }
}

#[test]
fn origins_are_https_or_local_http_only() {
    assert_eq!(origin_of("HTTPS://Www.Shop.Example:8443/x").as_deref(), Some("https://www.shop.example:8443"));
    assert_eq!(origin_of("http://localhost:3000/").as_deref(), Some("http://localhost:3000"));
    assert!(origin_of("http://shop.example/").is_none());
    assert!(origin_of("https://user@shop.example/").is_none());
    assert!(origin_of("file:///etc/passwd").is_none());
    assert_eq!(host_of("https://shop.example:8443/x"), "shop.example");
}

#[test]
fn relative_addresses_resolve_against_their_document() {
    assert_eq!(resolve("https://a.example/x/y.html", "z.png").as_deref(), Some("https://a.example/x/z.png"));
    assert_eq!(resolve("https://a.example/x/y.html", "/z.png").as_deref(), Some("https://a.example/z.png"));
    assert_eq!(resolve("https://a.example/x/y.html", "//c.example/z").as_deref(), Some("https://c.example/z"));
    assert_eq!(resolve("https://a.example", "z").as_deref(), Some("https://a.example/z"));
    assert!(resolve("https://a.example/", " ").is_none());
}

#[test]
fn the_manifest_gives_name_start_and_the_largest_reasonable_icon() {
    let m = parse_manifest(MANIFEST, "https://www.shop.example/m.json").unwrap();
    assert_eq!(m.name, "Shop");
    assert_eq!(m.start_url.as_deref(), Some("https://www.shop.example/home?utm=1"));
    assert_eq!(m.icon.as_deref(), Some("https://www.shop.example/i/192.png"), "1024 is over the cap");
    assert!(parse_manifest("not json", "https://a.example/").is_none());
    let short = parse_manifest(r#"{"short_name":"Sh"}"#, "https://a.example/").unwrap();
    assert_eq!(short.name, "Sh");
}

#[test]
fn the_record_takes_the_person_name_then_manifest_then_title_then_host() {
    let a = from_page(&page(), Some("  My shop "), &[]).unwrap();
    assert_eq!((a.name.as_str(), a.start_url.as_str()), ("My shop", "https://www.shop.example/home?utm=1"));
    assert_eq!(a.icon_url, "https://www.shop.example/i/192.png");
    assert_eq!(a.origins, vec!["https://www.shop.example".to_string()]);
    let mut p = page();
    p.manifest = None;
    let a = from_page(&p, None, &["https://login.example".into()]).unwrap();
    assert_eq!(a.name, "Shop - deals");
    assert_eq!(a.icon_url, "https://www.shop.example/fav.ico", "the favicon is the fallback");
    assert_eq!(a.start_url, "https://www.shop.example/a/b?q=1");
    assert_eq!(a.origins.len(), 2);
    p.title.clear();
    assert_eq!(from_page(&p, None, &[]).unwrap().name, "www.shop.example");
    p.url = "http://shop.example/".into();
    assert!(from_page(&p, None, &[]).is_err());
    assert!(from_page(&page(), None, &["ftp://x".into()]).is_err());
}

#[test]
fn a_manifest_start_on_another_origin_is_ignored() {
    let mut p = page();
    p.manifest = Some(r#"{"name":"X","start_url":"https://evil.example/"}"#.into());
    assert_eq!(from_page(&p, None, &[]).unwrap().start_url, "https://www.shop.example/a/b?q=1");
}

#[test]
fn links_stay_in_the_tile_only_for_allowed_origins() {
    let a = from_page(&page(), None, &["https://login.example".into()]).unwrap();
    assert_eq!(a.route("https://www.shop.example/cart"), Route::InTile);
    assert_eq!(a.route("https://login.example/auth"), Route::InTile);
    assert_eq!(a.route("https://other.example/"), Route::BrowserTile);
    assert_eq!(a.route("http://www.shop.example/"), Route::BrowserTile);
    assert_eq!(a.route("javascript:alert(1)"), Route::BrowserTile);
}

#[test]
fn the_store_adds_replaces_caps_renames_and_removal_deletes_the_profile() {
    let data = scratch("siteapps-store");
    let mut st = Store::default();
    let a = from_page(&page(), None, &[]).unwrap();
    st.add(a.clone()).unwrap();
    st.add(a.clone()).unwrap();
    assert_eq!(st.apps.len(), 1, "the same site is replaced");
    assert!(st.find("SHOP").is_some() && st.find(&a.id).is_some());
    st.rename(&a.id, "Deals").unwrap();
    assert!(st.rename(&a.id, "  ").is_err());
    let p = profile_dir(&data, "u", &a.id);
    std::fs::create_dir_all(p.join("cookies")).unwrap();
    std::fs::write(p.join("cookies/c"), vec![0u8; 100]).unwrap();
    assert_eq!(profile_size(&p), 100);
    assert!(profile_size(&p) < PROFILE_CAP_BYTES);
    assert!(st.remove(&data, "u", &a.id).unwrap());
    assert!(!p.exists(), "sign-ins are gone");
    assert!(st.remove(&data, "u", &a.id).is_err());
    for i in 0..MAX_APPS {
        let mut b = a.clone();
        b.id = format!("x{i}");
        st.add(b).unwrap();
    }
    let mut b = a;
    b.id = "one-more".into();
    assert!(st.add(b).is_err());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn the_profile_is_not_the_browser_tiles_and_audit_names_the_origin_only() {
    let p = profile_dir(Path::new("/d"), "u", "id1");
    assert!(p.starts_with("/d/web-profile") && p != Path::new("/d/web-profile/u"), "beside the Browser tile's profile, never the same folder");
    assert_ne!(profile_user("ada", "id1"), profile_user("adb", "id1"));
    assert_ne!(profile_dir(Path::new("/d"), "ada", "x"), profile_dir(Path::new("/d"), "ada", "y"));
    // the longest id (a 40 character host and its hash) still fits the 64 characters the engine profile name keeps, so the hash of the person is never cut off
    let long = from_page(&PageInfo { url: format!("https://{}.example/", "h".repeat(240)), title: String::new(), ..Default::default() }, None, &[]).unwrap();
    assert!(profile_user("anyone", &long.id).len() <= 64, "{}", profile_user("anyone", &long.id).len());
    assert_eq!(origin_of("https://a.example:443/x").as_deref(), Some("https://a.example"), "the default port is not part of the origin");
    assert_eq!(origin_of("https://a.example:8443/x").as_deref(), Some("https://a.example:8443"));
    let a = from_page(&page(), None, &[]).unwrap();
    let line = a.audit("installed");
    assert!(line.contains("https://www.shop.example") && !line.contains("/a/b") && !line.contains("Shop"));
    assert!(a.capabilities().iter().any(|c| c.contains("only when the site asks")));
}

#[test]
fn a_codec_site_gets_the_install_card_until_the_addon_is_ready_and_youtube_never() {
    let mut p = page();
    p.url = "https://www.tiktok.com/".into();
    p.manifest = None;
    let tt = from_page(&p, None, &[]).unwrap();
    assert!(codec_card(&tt, &Addon::Absent).unwrap().contains("add-on"));
    assert!(codec_card(&tt, &Addon::NotInImage).unwrap().contains("does not include"));
    assert!(codec_card(&tt, &Addon::Ready).is_none());
    p.url = "https://www.youtube.com/".into();
    assert!(codec_card(&from_page(&p, None, &[]).unwrap(), &Addon::Absent).is_none());
}

#[test]
fn protected_video_is_told_plainly_with_no_way_around() {
    assert!(protected_site("www.netflix.com") && protected_site("primevideo.com") && !protected_site("youtube.com") && !protected_site("notnetflix.com"));
    assert_eq!(protection_message("netflix.com", None), Some(PROTECTED_MESSAGE));
    assert_eq!(protection_message("example.org", Some("com.widevine.alpha")), Some(PROTECTED_MESSAGE));
    assert_eq!(protection_message("example.org", Some("org.w3.clearkey")), None);
    assert_eq!(protection_message("netflix.com", Some("org.w3.clearkey")), None);
    for k in ["com.microsoft.playready.recommendation", "com.apple.fps.1_0"] {
        assert!(key_system_needs_module(k));
    }
    let m = PROTECTED_MESSAGE.to_ascii_lowercase();
    assert!(m.contains("does not include or download") && !m.contains("install") && !m.contains("enable"));
}

#[test]
fn a_profile_over_the_cap_gets_a_plain_note_and_one_under_it_none() {
    assert!(size_note(0).is_none() && size_note(PROFILE_CAP_BYTES).is_none());
    let t = size_note(PROFILE_CAP_BYTES + 1).unwrap();
    assert!(t.contains("512 MB") && t.contains("signs the person out"), "{t}");
}

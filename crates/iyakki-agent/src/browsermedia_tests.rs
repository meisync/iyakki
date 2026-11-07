use super::*;
use std::sync::Mutex as M;

fn media(p: Policy) -> (Arc<BrowserMedia>, Arc<M<Vec<String>>>) {
    let log = Arc::new(M::new(Vec::new()));
    let l = log.clone();
    (BrowserMedia::new(p, Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))), None), log)
}

fn tagged(ch: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![ch];
    v.extend_from_slice(body);
    v
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

#[test]
fn everything_is_off_until_switched_on_and_a_late_chunk_is_dropped() {
    let (m, _) = media(Policy::allow_all());
    assert_eq!(m.sources(), Sources::default());
    assert_eq!(m.binary(&tagged(CH_MIC, &[0, 0, 1, 1])), Accepted::Dropped("microphone is off"));
    assert_eq!(m.binary(&tagged(CH_CAMERA, &[0xFF, 0xD8, 1, 2, 3])), Accepted::Dropped("camera is off"));
    m.set(Kind::Mic, true).unwrap();
    assert_eq!(m.binary(&tagged(CH_MIC, &[1, 0, 2, 0])), Accepted::Mic(4));
    assert_eq!(m.take_mic(), vec![1, 0, 2, 0]);
    m.set(Kind::Mic, false).unwrap();
    assert_eq!(m.binary(&tagged(CH_MIC, &[1, 0])), Accepted::Dropped("microphone is off"));
    assert!(m.take_mic().is_empty(), "nothing kept after a stop");
}

#[test]
fn policy_denies_and_the_denial_is_audited_without_content() {
    let (m, log) = media(Policy::deny_all());
    assert_eq!(m.set(Kind::Mic, true), Err(Denied::Policy));
    assert_eq!(m.set(Kind::Camera, true), Err(Denied::Policy));
    assert_eq!(m.set(Kind::ClipRead, true), Err(Denied::Policy));
    assert!(!m.is_on(Kind::Mic) && !m.is_on(Kind::Camera));
    assert_eq!(m.binary(&tagged(CH_CLIP_IMAGE, PNG)), Accepted::Dropped("clipboard is not allowed"));
    assert!(m.clips().is_empty());
    m.set_audio_out(true);
    assert!(!m.sources().audio_out && !m.push_audio(48_000, 2, &[1, 2]));
    let l = log.lock().unwrap().clone();
    assert!(l.contains(&"browser.media.denied mic".to_string()) && l.contains(&"browser.media.denied camera".to_string()) && l.contains(&"browser.media.denied audio".to_string()));
    assert!(m.state_json().contains("\"allowed\":{\"audio\":false,\"camera\":false,\"clipread\":false,\"mic\":false}"));
    assert_eq!(Policy::from_allow(Allow::default()), Policy::deny_all(), "tenants default to deny");
    assert_eq!(Policy::from_env_value(Some("allow"), Policy::deny_all()), Policy::allow_all());
    assert_eq!(Policy::from_env_value(Some("bogus"), Policy::deny_all()), Policy::deny_all());
}

#[test]
fn turning_on_and_off_is_audited_by_kind_only() {
    let (m, log) = media(Policy::allow_all());
    m.set(Kind::Camera, true).unwrap();
    m.binary(&tagged(CH_CAMERA, &[0xFF, 0xD8, 0x42, 0x42]));
    m.set(Kind::Camera, true).unwrap(); // again: no second line
    m.set(Kind::Camera, false).unwrap();
    let l = log.lock().unwrap().clone();
    assert_eq!(l, vec!["browser.media.on camera", "browser.media.off camera"]);
    assert!(l.iter().all(|s| !s.contains("66") && !s.contains("B")), "no content in the audit lines");
    assert!(m.camera().is_none(), "the last frame is forgotten when the camera goes off");
}

#[test]
fn microphone_framing_and_the_five_minute_limit() {
    let (m, log) = media(Policy::allow_all());
    m.set(Kind::Mic, true).unwrap();
    assert_eq!(m.binary(&tagged(CH_MIC, &[1, 2, 3])), Accepted::Dropped("bad microphone chunk"), "odd length");
    assert_eq!(m.binary(&tagged(CH_MIC, &[])), Accepted::Dropped("bad microphone chunk"));
    assert_eq!(m.binary(&tagged(CH_MIC, &vec![0u8; MAX_MIC_CHUNK + 2])), Accepted::Dropped("bad microphone chunk"));
    let second = vec![0u8; MIC_BYTES_PER_SEC];
    let mut last = Accepted::Mic(0);
    for _ in 0..MIC_LIMIT_SECS {
        last = m.binary(&tagged(CH_MIC, &second[..MIC_BYTES_PER_SEC.min(MAX_MIC_CHUNK)]));
        m.take_mic();
    }
    assert_eq!(last, Accepted::MicLimit, "the 300th second switches it off");
    assert!(!m.is_on(Kind::Mic));
    assert_eq!(m.binary(&tagged(CH_MIC, &[0, 0])), Accepted::Dropped("microphone is off"));
    assert!(log.lock().unwrap().iter().any(|l| l == "browser.media.limit mic"));
    let msgs = m.take_messages();
    assert!(msgs.iter().any(|s| s.contains("media_limit")) && msgs.iter().any(|s| s.contains("media_state")));
    // switching it on again starts a new five minutes
    m.set(Kind::Mic, true).unwrap();
    assert_eq!(m.binary(&tagged(CH_MIC, &[0, 0])), Accepted::Mic(2));
}

#[test]
fn the_microphone_buffer_keeps_only_the_newest_sound() {
    let (m, _) = media(Policy::allow_all());
    m.set(Kind::Mic, true).unwrap();
    for i in 0..20u8 {
        m.binary(&tagged(CH_MIC, &vec![i; 32_000]));
    }
    let got = m.take_mic();
    assert_eq!(got.len(), MIC_BUFFER);
    assert_eq!(*got.last().unwrap(), 19);
}

#[test]
fn camera_frames_are_jpeg_small_and_slow() {
    let (m, _) = media(Policy::allow_all());
    m.set(Kind::Camera, true).unwrap();
    assert_eq!(m.binary(&tagged(CH_CAMERA, b"GIF89a....")), Accepted::Dropped("not a JPEG frame"));
    assert_eq!(m.binary(&tagged(CH_CAMERA, &vec![0xFF; 3])), Accepted::Dropped("not a JPEG frame"));
    let mut big = vec![0xFF, 0xD8];
    big.resize(MAX_CAMERA_FRAME + 1, 0);
    assert_eq!(m.binary(&tagged(CH_CAMERA, &big)), Accepted::Dropped("not a JPEG frame"));
    assert_eq!(m.binary(&tagged(CH_CAMERA, &[0xFF, 0xD8, 9, 9])), Accepted::Camera);
    assert_eq!(m.binary(&tagged(CH_CAMERA, &[0xFF, 0xD8, 8, 8])), Accepted::Dropped("too fast"));
    assert_eq!(m.camera().unwrap(), vec![0xFF, 0xD8, 9, 9]);
    assert_eq!(m.camera_frames(), 1);
    std::thread::sleep(MIN_CAMERA_GAP + Duration::from_millis(20));
    assert_eq!(m.binary(&tagged(CH_CAMERA, &[0xFF, 0xD8, 7, 7])), Accepted::Camera);
}

#[test]
fn the_camera_frame_is_exported_for_other_jobs_and_removed_on_stop() {
    let dir = crate::testdir::unique("bm", "export");
    let d = session_dir(&dir, "../a/b c");
    assert!(d.ends_with("iyakki-browser-media/abc"), "{d:?}");
    let m = BrowserMedia::new(Policy::allow_all(), Arc::new(|_, _| {}), Some(d.clone()));
    m.set(Kind::Camera, true).unwrap();
    m.binary(&tagged(CH_CAMERA, &[0xFF, 0xD8, 5, 5]));
    assert_eq!(std::fs::read(d.join("camera.jpg")).unwrap(), vec![0xFF, 0xD8, 5, 5]);
    assert!(std::fs::read_to_string(d.join("sources.json")).unwrap().contains("\"camera\":true"));
    m.connection(false);
    assert!(!d.join("camera.jpg").exists());
    assert!(std::fs::read_to_string(d.join("sources.json")).unwrap().contains("\"camera\":false"));
}

#[test]
fn rich_clipboard_items_are_checked_limited_and_audited() {
    let (m, log) = media(Policy::allow_all());
    assert_eq!(m.binary(&tagged(CH_CLIP_IMAGE, PNG)), Accepted::Clip(ClipKind::Image));
    assert_eq!(m.binary(&tagged(CH_CLIP_IMAGE, b"JFIF")), Accepted::Dropped("not a PNG or too big"));
    let mut big = PNG.to_vec();
    big.resize(MAX_CLIP_IMAGE + 1, 0);
    assert_eq!(m.binary(&tagged(CH_CLIP_IMAGE, &big)), Accepted::Dropped("not a PNG or too big"));
    assert_eq!(m.binary(&tagged(CH_CLIP_HTML, "<b>h\u{e9}</b>".as_bytes())), Accepted::Clip(ClipKind::Html));
    assert_eq!(m.binary(&tagged(CH_CLIP_HTML, &[0xFF, 0xFE])), Accepted::Dropped("bad HTML"));
    assert_eq!(m.binary(&tagged(CH_CLIP_HTML, &vec![b'a'; MAX_CLIP_HTML + 1])), Accepted::Dropped("bad HTML"));
    assert_eq!(m.clips().len(), 2);
    for _ in 0..30 {
        m.binary(&tagged(CH_CLIP_HTML, b"<i>x</i>"));
    }
    assert_eq!(m.clips().len(), CLIP_ITEMS, "the history is bounded");
    assert!(log.lock().unwrap().iter().all(|l| l == "browser.clip.in image" || l == "browser.clip.in html"));
    assert_eq!(m.binary(&[]), Accepted::Dropped("empty"));
    assert_eq!(m.binary(&[0x77, 1]), Accepted::Dropped("unknown channel"));
}

#[test]
fn total_size_of_the_clipboard_history_is_bounded() {
    let (m, _) = media(Policy::allow_all());
    let mut img = PNG.to_vec();
    img.resize(MAX_CLIP_IMAGE, 1);
    for _ in 0..5 {
        m.binary(&tagged(CH_CLIP_IMAGE, &img));
    }
    assert!(m.clips().iter().map(|c| c.bytes.len()).sum::<usize>() <= CLIP_TOTAL);
}

#[test]
fn html_becomes_plain_text() {
    assert_eq!(BrowserMedia::html_as_text("<p>Hello <b>world</b></p><p>a &amp; b</p>"), "Hello world\na & b");
    assert_eq!(BrowserMedia::html_as_text("<style>p{}</style>x<script>alert(1)</script>y"), "xy");
    assert_eq!(BrowserMedia::html_as_text("caf\u{e9} <i>\u{e9}</i>"), "caf\u{e9} \u{e9}");
}

#[test]
fn sound_out_is_framed_chunked_and_bounded() {
    let (m, _) = media(Policy::allow_all());
    assert!(!m.push_audio(48_000, 2, &[1, 2]), "no page, no sound is queued");
    m.set_audio_out(true);
    assert!(!m.push_audio(1, 2, &[1, 2]) && !m.push_audio(48_000, 3, &[1, 2]));
    assert!(m.push_audio(48_000, 2, &vec![7i16; 20_000]));
    let out = m.take_audio();
    assert_eq!(out.len(), 3, "40000 bytes in 16 KiB chunks");
    let first = &out[0];
    assert_eq!(first[0], OUT_AUDIO);
    assert_eq!(first[1], 2);
    assert_eq!(u32::from_le_bytes(first[2..6].try_into().unwrap()), 48_000);
    assert_eq!(u32::from_le_bytes(first[6..10].try_into().unwrap()), 0);
    assert_eq!(first.len(), 10 + AUDIO_CHUNK_BYTES);
    assert_eq!(u32::from_le_bytes(out[2][6..10].try_into().unwrap()), 2, "sequence numbers count up");
    assert_eq!(&first[10..12], &7i16.to_le_bytes());
    assert!(m.take_audio().is_empty());
    m.push_audio(48_000, 2, &vec![0i16; 400_000]);
    assert!(m.take_audio().iter().map(|c| c.len() - 10).sum::<usize>() <= AUDIO_QUEUE_BYTES);
    assert!(m.audio_dropped() > 0, "old sound is dropped, not delayed");
    m.set_audio_out(false);
    assert!(!m.push_audio(48_000, 2, &[1, 2]));
}

#[test]
fn clip_out_messages_are_tagged_and_limited() {
    assert_eq!(BrowserMedia::clip_out(ClipKind::Image, PNG).unwrap()[0], OUT_CLIP_IMAGE);
    assert_eq!(BrowserMedia::clip_out(ClipKind::Html, b"<b>").unwrap()[0], OUT_CLIP_HTML);
    assert!(BrowserMedia::clip_out(ClipKind::Html, &vec![0; MAX_CLIP_HTML + 1]).is_none());
    assert!(BrowserMedia::clip_out(ClipKind::Image, b"").is_none());
}

#[test]
fn a_connection_that_goes_away_switches_everything_off_and_is_registered_per_person() {
    let (m, log) = media(Policy::allow_all());
    m.set(Kind::Mic, true).unwrap();
    m.set(Kind::Camera, true).unwrap();
    m.set_audio_out(true);
    assert_eq!(m.indicator().len(), 2);
    m.connection(false);
    assert_eq!(m.sources(), Sources::default());
    assert!(m.indicator().is_empty());
    assert!(log.lock().unwrap().iter().filter(|l| l.starts_with("browser.media.off")).count() == 2);
    register("zed-test-user", m.clone());
    assert!(for_user("zed-test-user").is_some() && for_user("nobody").is_none());
    unregister("zed-test-user");
    assert!(for_user("zed-test-user").is_none());
    assert!(BrowserMedia::denial_json(Kind::Camera).contains("media_denied"));
    assert_eq!(Kind::parse("clipread"), Some(Kind::ClipRead));
    assert_eq!(Kind::parse("screen"), None);
}

#[test]
fn a_tenant_denies_browser_media_by_default_and_the_pod_is_told() {
    use crate::cloudtenant::{manifests, TenantConfig};
    let c = TenantConfig::example("acme");
    assert_eq!(c.media, Allow::Deny, "default deny for tenants");
    let y = manifests(&c).unwrap();
    assert!(y.contains("IYAKKI_BROWSER_MEDIA, value: deny"), "{y}");
    let mut allowed = c.clone();
    allowed.media = Allow::Allow;
    assert!(manifests(&allowed).unwrap().contains("IYAKKI_BROWSER_MEDIA, value: allow"));
    let text = serde_json::to_string(&allowed).unwrap();
    assert_eq!(TenantConfig::parse(&text).unwrap().media, Allow::Allow);
    let without = text.replace(",\"media\":\"allow\"", "");
    assert_eq!(TenantConfig::parse(&without).unwrap().media, Allow::Deny, "a configuration without the setting denies");
    assert!(TenantConfig::parse(&text.replace("\"allow\"", "\"maybe\"")).is_err());
    let mut cloud = c.clone();
    cloud.profile = crate::cloudtenant::Profile::Cloud;
    cloud.domain = Some("acme.example.org".into());
    cloud.tls_secret = Some("acme-tls".into());
    if let Ok(y) = manifests(&cloud) {
        assert!(y.contains("IYAKKI_BROWSER_MEDIA, value: deny") && y.matches("env:").count() >= 1, "{y}");
    }
}

// ---- where the session's sound plays ("Play sound on")

fn later(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

#[test]
fn the_default_route_is_the_browser_and_the_machine_is_silent_only_while_the_sound_is_on() {
    let (m, log) = media(Policy::allow_all());
    assert_eq!(m.sink_mode(), SinkMode::Browser);
    assert!(!muted_by(&[m.clone()], later(0)), "sound off: the machine plays");
    m.set_audio_out(true);
    assert!(muted_by(&[m.clone()], later(0)), "sound on in the browser: the machine is silent");
    assert!(m.wants_audio() && m.push_audio(48_000, 2, &[1, 2]));
    m.set_audio_out(false);
    assert!(!muted_by(&[m.clone()], later(0)), "switched off: restored at once");
    assert!(!log.lock().unwrap().iter().any(|l| l.starts_with("browser.media.route")), "nothing audited until the person chooses");
}

#[test]
fn both_and_the_mac_only_never_silence_the_machine_and_the_mac_only_sends_nothing() {
    let (m, log) = media(Policy::allow_all());
    m.set_audio_out(true);
    m.set_sink_mode(SinkMode::Both);
    assert!(!muted_by(&[m.clone()], later(0)) && m.wants_audio() && m.push_audio(48_000, 2, &[1, 2]), "both: the machine plays and the page gets a copy");
    m.set_sink_mode(SinkMode::Mac);
    assert!(!muted_by(&[m.clone()], later(0)));
    assert!(!m.wants_audio() && !m.push_audio(48_000, 2, &[1, 2]) && m.take_audio().is_empty(), "the Mac only: nothing is queued for the page");
    m.set_sink_mode(SinkMode::Browser);
    assert!(muted_by(&[m.clone()], later(0)));
    let l = log.lock().unwrap().clone();
    assert_eq!(l.iter().filter(|x| x.starts_with("browser.media.route")).cloned().collect::<Vec<_>>(), ["browser.media.route both", "browser.media.route mac", "browser.media.route browser"]);
    assert!(m.take_messages().iter().any(|s| s.contains("\"sink\":\"browser\"")), "the page is told the route");
}

#[test]
fn a_dropped_connection_or_a_hung_one_never_leaves_the_machine_muted() {
    let (m, _) = media(Policy::allow_all());
    m.connection(true);
    m.set_audio_out(true);
    assert!(muted_by(&[m.clone()], later(0)));
    // the connection keeps asking for sound while it lives
    m.take_audio();
    assert!(muted_by(&[m.clone()], later(3_000)));
    // it hangs or its thread died without telling anyone: after the liveness window the machine plays again
    assert!(!muted_by(&[m.clone()], later(5_000)), "no pull for longer than SINK_ALIVE");
    // and a page that goes away says so
    m.take_audio();
    assert!(muted_by(&[m.clone()], later(0)));
    m.connection(false);
    assert!(!muted_by(&[m.clone()], later(0)), "disconnect restores");
    assert!(!m.sources().audio_out);
}

#[test]
fn only_the_controlling_session_decides_and_other_sessions_are_not_touched() {
    let (a, _) = media(Policy::allow_all());
    let (b, _) = media(Policy::allow_all());
    a.set_audio_out(true);
    std::thread::sleep(Duration::from_millis(5));
    b.set_audio_out(true);
    b.set_sink_mode(SinkMode::Both);
    assert_eq!(a.sink_mode(), SinkMode::Browser, "another session's choice is not changed");
    assert!(!muted_by(&[a.clone(), b.clone()], later(0)), "b switched its sound on last: its Both applies, the machine plays");
    b.set_audio_out(false);
    assert!(muted_by(&[a.clone(), b.clone()], later(0)), "b is gone: a is controlling again");
    // a session that is not playing at all never mutes
    let (c, _) = media(Policy::allow_all());
    assert!(!muted_by(&[c], later(0)));
    // both pages get their copy by their own choice
    b.set_audio_out(true);
    a.set_sink_mode(SinkMode::Mac);
    assert!(!a.wants_audio() && b.wants_audio());
}

#[test]
fn a_policy_that_forbids_sound_keeps_the_machine_playing() {
    let (m, log) = media(Policy::deny_all());
    m.set_audio_out(true);
    assert!(!muted_by(&[m], later(0)));
    assert!(log.lock().unwrap().contains(&"browser.media.denied audio".to_string()));
}

#[test]
fn the_camera_the_desktop_uses_is_the_one_that_has_it_on_else_the_last_page_that_may() {
    // (the registry is process wide: this test only looks at names it registered itself, in order)
    let (a, _) = media(Policy::allow_all());
    let (b, _) = media(Policy::allow_all());
    let (c, _) = media(Policy { camera: false, ..Policy::allow_all() });
    register("cs-user-a", a.clone());
    register("cs-user-b", b.clone());
    register("cs-user-c", c.clone());
    let pick = camera_session().unwrap();
    assert!(Arc::ptr_eq(&pick, &c) == false, "a page whose policy forbids the camera is never offered");
    a.set(Kind::Camera, true).unwrap();
    assert!(Arc::ptr_eq(&camera_session().unwrap(), &a), "the page with the camera on comes first");
    for u in ["cs-user-a", "cs-user-b", "cs-user-c"] {
        unregister(u);
    }
}

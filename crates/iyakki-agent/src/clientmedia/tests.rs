//! The media half of the client endpoint against fakes: a fake sound card, fake cards for consent, a fake USB camera. No real device was used.

use super::*;
use crate::mixer::sink::{FakeSink, Sink};
use crate::mixer::tee::TeeSink;
use iyakki_core_api::broker::Caller;

fn routes() -> Arc<AudioRoutes> {
    let r = Arc::new(AudioRoutes::new());
    r.set_audible(|p| p == "ann");
    r
}

/// The same wiring as `TeeSink::browser`, with these routes and no browser pages.
fn tee(card: &FakeSink, r: &Arc<AudioRoutes>) -> TeeSink {
    let (a, b, c, d) = (r.clone(), r.clone(), r.clone(), r.clone());
    TeeSink::new(Box::new(card.clone()), Box::new(move |rate, ch, p| a.tap(rate, ch, p)), Box::new(move || b.listening()))
        .with_mute(Box::new(move || c.machine_muted()))
        .with_on_stop(Box::new(move || d.flush_all()))
}

/// 20 ms of stereo that is not silence.
fn period(v: i16) -> Vec<i16> {
    vec![v; 1920]
}

// ------------------------------------------------------------------------------------------------ sound

#[test]
fn a_session_has_one_route_and_one_side_and_changing_it_never_opens_both_by_accident() {
    let r = routes();
    assert_eq!(r.open(7, "ann", Side::Client), Opened::New);
    assert_eq!(r.open(7, "ann", Side::Client), Opened::Same);
    assert_eq!(r.sessions(), 1);
    assert!(r.machine_muted(), "client only: the machine's speakers stay quiet");
    assert_eq!(r.open(7, "ann", Side::Both), Opened::Switched);
    assert_eq!((r.sessions(), r.side_of(7)), (1, Some(Side::Both)), "still one route, now the other side");
    assert!(!r.machine_muted(), "both: the machine plays too");
    assert!(r.listening());
    assert_eq!(r.open(7, "ann", Side::Client), Opened::Switched);
    assert!(r.machine_muted());
}

#[test]
fn the_default_for_a_remote_client_is_the_client_and_the_machine_is_quiet() {
    let card = FakeSink::new();
    let r = routes();
    let mut t = tee(&card, &r);
    t.open(48_000, 2).unwrap();
    r.open(1, "ann", Side::parse(None).unwrap());
    t.write(&period(500)).unwrap();
    assert!(card.lock().samples.iter().all(|s| *s == 0), "silence of the same length on the machine");
    assert_eq!(card.lock().samples.len(), 1920);
    let p = r.pull(1);
    assert_eq!(p.chunks.len(), 1);
    let (seq, pcm) = media::decode_audio(&p.chunks[0][..], 2).unwrap();
    assert_eq!((seq, pcm.len(), pcm[0]), (0, 1920, 500), "the client got the real sound");
}

#[test]
fn both_sides_is_an_explicit_choice_and_each_gets_the_sound() {
    let card = FakeSink::new();
    let r = routes();
    let mut t = tee(&card, &r);
    t.open(48_000, 2).unwrap();
    r.open(1, "ann", Side::Both);
    t.write(&period(300)).unwrap();
    assert_eq!(card.lock().samples[0], 300, "the machine plays");
    assert_eq!(r.pull(1).chunks.len(), 1, "and so does the client");
}

#[test]
fn the_queue_never_holds_more_than_100_ms_and_keeps_the_newest() {
    let r = routes();
    r.open(1, "ann", Side::Client);
    for i in 0..12 {
        assert!(r.tap(48_000, 2, &period(i)));
    }
    assert_eq!(r.dropped(1), 7);
    let p = r.pull(1);
    assert_eq!(p.chunks.len(), 5, "5 x 20 ms");
    let seqs: Vec<u32> = p.chunks.iter().map(|c| media::decode_audio(c, 2).unwrap().0).collect();
    assert_eq!(seqs, vec![7, 8, 9, 10, 11], "the gap is visible to the client by the sequence numbers");
    assert!(r.pull(1).chunks.is_empty());
}

#[test]
fn a_period_of_any_length_is_cut_into_whole_20_ms_chunks() {
    let r = routes();
    r.open(1, "ann", Side::Client);
    r.tap(48_000, 2, &vec![1; 1000]);
    assert!(r.pull(1).chunks.is_empty(), "less than a chunk waits");
    r.tap(48_000, 2, &vec![1; 3000]);
    let p = r.pull(1);
    assert_eq!(p.chunks.len(), 2);
    assert!(p.chunks.iter().all(|c| c.len() == 4 + 3840));
}

#[test]
fn mono_is_made_stereo_and_other_rates_are_not_sent() {
    let r = routes();
    r.open(1, "ann", Side::Client);
    assert!(!r.tap(44_100, 2, &period(1)), "only 48 kHz is carried");
    assert!(!r.tap(48_000, 6, &period(1)));
    assert!(r.tap(48_000, 1, &vec![9; 960]));
    let (_, pcm) = media::decode_audio(&r.pull(1).chunks[0], 2).unwrap();
    assert_eq!((pcm[0], pcm[1], pcm.len()), (9, 9, 1920));
}

#[test]
fn a_stop_or_pause_flushes_the_queue_and_tells_the_client() {
    let card = FakeSink::new();
    let r = routes();
    let mut t = tee(&card, &r);
    t.open(48_000, 2).unwrap();
    r.open(1, "ann", Side::Client);
    t.write(&period(100)).unwrap();
    t.write(&period(100)).unwrap();
    t.write(&vec![5; 500]).unwrap();
    t.close(); // the mixer closes its output when the last source ends or pauses
    let p = r.pull(1);
    assert!(p.flush, "the client must drop what it holds");
    assert!(p.chunks.is_empty(), "nothing queued plays on after the stop");
    // and the part chunk is gone too: the next sound starts clean
    t.open(48_000, 2).unwrap();
    t.write(&vec![5; 1420]).unwrap();
    assert!(r.pull(1).chunks.is_empty(), "500 + 1420 would have made a chunk if the stop had kept the rest");
    t.abort();
    assert!(r.pull(1).flush, "abort flushes too");
    assert!(!r.pull(1).flush, "told once");
}

#[test]
fn disconnect_gives_the_machines_speakers_back() {
    let card = FakeSink::new();
    let r = routes();
    let mut t = tee(&card, &r);
    t.open(48_000, 2).unwrap();
    r.open(3, "ann", Side::Client);
    t.write(&period(77)).unwrap();
    assert_eq!(card.lock().samples[0], 0);
    assert!(r.close(3), "the session ended");
    t.write(&period(77)).unwrap();
    assert_eq!(card.lock().samples[1920], 77, "the machine plays again from the next period");
    assert!(!r.machine_muted() && !r.listening());
    assert!(r.pull(3).chunks.is_empty(), "nothing is kept for a session that is gone");
}

#[test]
fn a_hung_connection_that_stops_pulling_does_not_keep_the_machine_silent() {
    let r = AudioRoutes::with_alive(Duration::from_millis(40));
    r.set_audible(|_| true);
    r.open(1, "ann", Side::Client);
    assert!(r.machine_muted());
    std::thread::sleep(Duration::from_millis(80));
    assert!(!r.machine_muted(), "nobody pulled for longer than the liveness window");
    assert!(!r.tap(48_000, 2, &period(1)), "and nothing is queued for it");
    r.pull(1);
    assert!(r.machine_muted(), "a pull brings it back");
}

#[test]
fn nobody_gets_the_sound_until_the_machine_says_whose_it_is() {
    let r = AudioRoutes::new();
    r.open(1, "ann", Side::Client);
    assert!(!r.tap(48_000, 2, &period(1)), "fail closed");
    assert!(!r.machine_muted() && !r.listening());
    let r = routes();
    r.open(1, "bob", Side::Client);
    assert!(!r.tap(48_000, 2, &period(1)), "another person's client does not hear ann's sound");
    assert!(!r.machine_muted());
}

#[test]
fn the_mixer_plays_into_a_client_route_end_to_end() {
    use crate::mixer::{EngineCfg, Mixer, MixerCfg, SourceKind as Kind};
    let card = FakeSink::new();
    let r = routes();
    r.open(1, "ann", Side::Client);
    // The mixer is paced like a card and the tone lasts 3 s. A mixer that mixes as fast as it can finishes a short tone in a few
    // milliseconds, closes its output and the tee then flushes every queued chunk (the `with_on_stop` above): a test thread that was
    // not scheduled for a moment found nothing left to pull.
    let m = Mixer::start(MixerCfg { raise_priority: false, engine: EngineCfg::default().no_prefill(), ..MixerCfg::default() }, Box::new(tee(&card, &r)));
    let w = m.open_source(Kind::Media, 48_000, 2).unwrap();
    let tone: Vec<i16> = (0..288_000).map(|i| ((i % 100) as i16 - 50) * 100).collect();
    w.write_i16(&tone);
    let end = Instant::now() + Duration::from_secs(5);
    let mut got = 0;
    while got < 3 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        got += r.pull(1).chunks.len();
    }
    assert!(got >= 3, "the mixed sound reached the client route");
    assert!(card.lock().samples.iter().all(|s| *s == 0), "and the machine stayed quiet");
    m.shutdown();
}

// ------------------------------------------------------------------------------------------------ consent

struct Inner(CardAnswer, Mutex<u32>);
impl CardAsk for Inner {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        *self.1.lock().unwrap() += 1;
        self.0
    }
}

fn client(device: &str) -> Caller {
    Caller { app: "client".into(), kind: CallerKind::Client { device_id: device.into() }, person: "ann".into(), device: "Maya's laptop".into() }
}

#[test]
fn the_press_of_the_clients_button_answers_the_clipboard_once_and_nothing_else() {
    let inner = Arc::new(Inner(CardAnswer::Deny, Mutex::new(0)));
    let c = ClientConsent::new(Some(inner.clone()));
    c.note("dev-1", "clipboard.client");
    assert_eq!(c.ask(&client("dev-1"), "clipboard.client", ""), CardAnswer::Allow);
    assert_eq!(c.ask(&client("dev-1"), "clipboard.client", ""), CardAnswer::Deny, "one press, one use");
    assert_eq!(*inner.1.lock().unwrap(), 1, "the machine's card was asked the second time");
    // a press for one device does not cover another, and a camera or microphone is never answered by the client's say-so
    c.note("dev-1", "clipboard.client");
    assert_eq!(c.ask(&client("dev-2"), "clipboard.client", ""), CardAnswer::Deny);
    c.note("dev-1", "client.camera.offer");
    c.note("dev-1", "client.microphone.offer");
    assert_eq!(c.ask(&client("dev-1"), "client.camera.offer", ""), CardAnswer::Deny);
    assert_eq!(c.ask(&client("dev-1"), "client.microphone.offer", ""), CardAnswer::Deny);
    // forgetting an unused press
    c.forget("dev-1", "clipboard.client");
    assert_eq!(c.ask(&client("dev-1"), "clipboard.client", ""), CardAnswer::Deny);
    // no machine card: unavailable, never a silent yes
    assert_eq!(ClientConsent::new(None).ask(&client("dev-1"), "client.camera.offer", ""), CardAnswer::Unavailable);
}

// ------------------------------------------------------------------------------------------------ leases

fn lease(session: u64, who: &str, exclusive: bool, local: bool) -> Lease {
    Lease { session, who: who.into(), exclusive, local }
}

#[test]
fn a_camera_can_be_shared_for_viewing_but_an_exclusive_device_has_one_holder() {
    let l = Leases::new();
    assert!(matches!(l.acquire("usb:cam", lease(1, "Maya's laptop", false, false)), Taken::Granted { .. }));
    assert!(matches!(l.acquire("usb:cam", lease(2, "Ben's phone", false, false)), Taken::Granted { .. }), "viewers share");
    assert_eq!(l.holders("usb:cam").len(), 2);
    // a microphone: one holder, a second remote request is refused with the holder's name
    assert!(matches!(l.acquire("usb:mic", lease(1, "Maya's laptop", true, false)), Taken::Granted { .. }));
    assert_eq!(l.acquire("usb:mic", lease(2, "Ben's phone", true, false)), Taken::InUse("Maya's laptop".into()));
    assert_eq!(l.acquire("usb:mic", lease(2, "Ben's phone", false, false)), Taken::InUse("Maya's laptop".into()), "even a shared look is refused while it is exclusive");
    // a holder asking again is the same holder
    assert!(matches!(l.acquire("usb:mic", lease(1, "Maya's laptop", true, false)), Taken::Granted { .. }));
    assert_eq!(l.holders("usb:mic").len(), 1);
}

#[test]
fn the_person_at_the_machine_wins_and_a_remote_client_never_pushes_a_local_one_out() {
    let l = Leases::new();
    let here = local_session();
    assert!(matches!(l.acquire("usb:mic", lease(1, "Maya's laptop", true, false)), Taken::Granted { .. }));
    match l.acquire("usb:mic", lease(here, "the person at the machine", true, true)) {
        Taken::Granted { preempted } => assert_eq!(preempted.iter().map(|p| p.session).collect::<Vec<_>>(), vec![1], "the remote holder is told it lost the device"),
        other => panic!("{other:?}"),
    }
    assert!(!l.holds("usb:mic", 1) && l.holds("usb:mic", here));
    assert_eq!(l.acquire("usb:mic", lease(1, "Maya's laptop", true, false)), Taken::InUse("the person at the machine".into()));
    // two local holders of one exclusive device are refused too
    assert_eq!(l.acquire("usb:mic", lease(local_session(), "an app", true, true)), Taken::InUse("the person at the machine".into()));
    assert_eq!(l.exclusive_holder("usb:mic"), Some("the person at the machine".into()));
}

#[test]
fn a_lease_is_released_on_disconnect() {
    let l = Leases::new();
    l.acquire("usb:cam", lease(5, "Maya's laptop", false, false));
    l.acquire("usb:mic", lease(5, "Maya's laptop", true, false));
    l.acquire("usb:mic2", lease(6, "Ben's phone", true, false));
    l.release_session(5);
    assert!(l.holders("usb:cam").is_empty() && l.holders("usb:mic").is_empty());
    assert_eq!(l.holders("usb:mic2").len(), 1);
    assert!(matches!(l.acquire("usb:mic", lease(9, "Ben's phone", true, false)), Taken::Granted { .. }), "free again");
}

// ------------------------------------------------------------------------------------------------ the client's own devices

#[test]
fn virtual_sources_are_named_after_the_device_and_are_per_tenant() {
    let v = VirtualSources::new();
    let a = v.offer(SourceKind::Camera, 1, "dev-1", "Maya's laptop", "ann", "acme").unwrap();
    let b = v.offer(SourceKind::Microphone, 1, "dev-1", "Maya's laptop", "ann", "acme").unwrap();
    assert_eq!((a.name.as_str(), b.name.as_str()), ("Maya's laptop camera", "Maya's laptop microphone"));
    assert_eq!(a.id, "client/dev-1/camera");
    // two devices with one name in one tenant get different names; another tenant is separate and sees none of these
    let c = v.offer(SourceKind::Camera, 2, "dev-2", "Maya's laptop", "ann", "acme").unwrap();
    assert_eq!(c.name, "Maya's laptop camera (2)");
    let d = v.offer(SourceKind::Camera, 3, "dev-3", "Maya's laptop", "carl", "globex").unwrap();
    assert_eq!(d.name, "Maya's laptop camera", "the name is free in another tenant");
    assert_eq!(v.list("acme").len(), 3);
    assert_eq!(v.list("globex").iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Maya's laptop camera"]);
    assert!(v.list("initech").is_empty(), "a third tenant sees nothing");
}

#[test]
fn virtual_sources_vanish_on_disconnect_and_a_device_cannot_offer_twice() {
    let v = VirtualSources::new();
    v.offer(SourceKind::Camera, 1, "dev-1", "Laptop", "ann", "").unwrap();
    v.offer(SourceKind::Microphone, 1, "dev-1", "Laptop", "ann", "").unwrap();
    // the same device from a second connection (a second tab) is refused: one holder
    assert_eq!(v.offer(SourceKind::Camera, 2, "dev-1", "Laptop", "ann", ""), Err(OfferErr::Already("Laptop camera".into())));
    // another kind of the same device from the second connection is fine
    assert_eq!(v.offer(SourceKind::Microphone, 1, "dev-1", "Laptop", "ann", "").unwrap().kind, SourceKind::Microphone);
    assert_eq!(v.list("").len(), 2, "re-offering from the same session replaces, not duplicates");
    let gone = v.vanish(1);
    assert_eq!(gone.len(), 2);
    assert!(v.list("").is_empty());
    assert!(v.offer(SourceKind::Camera, 2, "dev-1", "Laptop", "ann", "").is_ok(), "free once the first connection is gone");
    assert!(v.close(SourceKind::Camera, 2).is_some() && v.close(SourceKind::Camera, 2).is_none());
}

#[test]
fn frames_from_a_client_are_checked_and_dropped_because_nothing_consumes_them_yet() {
    let v = VirtualSources::new();
    let jpeg = [0xFF, 0xD8, 1, 2, 0xFF, 0xD9];
    assert_eq!(v.accept(1, SourceKind::Camera, &jpeg), Accepted::Refused("not_open"), "no offer, no frames");
    v.offer(SourceKind::Camera, 1, "dev-1", "Laptop", "ann", "").unwrap();
    v.offer(SourceKind::Microphone, 1, "dev-1", "Laptop", "ann", "").unwrap();
    assert_eq!(v.accept(1, SourceKind::Camera, &jpeg), Accepted::Dropped);
    assert_eq!(v.accept(1, SourceKind::Camera, b"not a jpeg"), Accepted::Refused("bad_data"));
    assert_eq!(v.accept(1, SourceKind::Camera, &vec![0xFF; media::MAX_CAMERA_FRAME + 1]), Accepted::Refused("too_big"));
    assert_eq!(v.accept(1, SourceKind::Microphone, &[1, 2, 3, 4]), Accepted::Dropped);
    assert_eq!(v.accept(1, SourceKind::Microphone, &[1, 2, 3]), Accepted::Refused("bad_data"));
    assert_eq!(v.accept(2, SourceKind::Microphone, &[1, 2]), Accepted::Refused("not_open"), "another session's frames are not accepted");
    assert_eq!(v.counts(1, SourceKind::Camera), Some((1, 1)));
    assert_eq!(v.counts(1, SourceKind::Microphone), Some((1, 1)));
}

#[test]
fn the_indicator_names_the_sources_in_use_and_clears_when_they_vanish() {
    let v = VirtualSources::new();
    assert_eq!(v.indicator().text(), None);
    v.offer(SourceKind::Camera, 1, "dev-1", "Maya's laptop", "ann", "").unwrap();
    assert_eq!(v.indicator().text().unwrap(), "Maya's laptop camera is in use by this machine");
    v.offer(SourceKind::Microphone, 1, "dev-1", "Maya's laptop", "ann", "").unwrap();
    let i = v.indicator();
    assert!(i.active() && i.cameras.len() == 1 && i.microphones.len() == 1);
    assert_eq!(i.text().unwrap(), "Maya's laptop camera and Maya's laptop microphone are in use by this machine");
    v.vanish(1);
    assert!(!v.indicator().active());
}

// ------------------------------------------------------------------------------------------------ cameras through the hub

mod hub {
    use super::*;
    use crate::cameras::*;
    use std::sync::atomic::AtomicBool;

    struct SolidFeed(u8);
    impl Feed for SolidFeed {
        fn next(&mut self) -> Result<Picture, FeedErr> {
            std::thread::sleep(Duration::from_millis(20));
            Ok(Picture::Rgba { width: 8, height: 4, data: [self.0, 100, 50, 255].repeat(32) })
        }
    }
    struct FakeUsb {
        busy: Arc<AtomicBool>,
    }
    impl UsbProvider for FakeUsb {
        fn list(&self) -> Vec<UsbCam> {
            vec![UsbCam { id: "video0".into(), name: "HD Webcam".into(), builtin: false }]
        }
        fn open(&self, _: &str) -> Result<Box<dyn Feed>, FeedErr> {
            if self.busy.load(std::sync::atomic::Ordering::SeqCst) {
                Err(FeedErr::Busy("Recorder".into()))
            } else {
                Ok(Box::new(SolidFeed(200)))
            }
        }
    }
    struct NoSecrets;
    impl CamSecrets for NoSecrets {
        fn get(&self, _: &str, _: &str) -> Result<zeroize::Zeroizing<String>, String> {
            Err("none".into())
        }
        fn set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn has(&self, _: &str, _: &str) -> bool {
            false
        }
    }

    fn feed(tag: &str, busy: bool) -> (Arc<Hub>, HubFeed) {
        let dir = std::env::temp_dir().join(format!("iyakki-clientmedia-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let env = Env {
            data: dir.clone(),
            secrets: Arc::new(NoSecrets),
            people: Arc::new(|| vec!["ann".into()]),
            usb: Arc::new(FakeUsb { busy: Arc::new(AtomicBool::new(busy)) }),
            h264: Arc::new(NoH264),
            encode: Arc::new(|w, h, d| {
                let mut v = vec![0xFF, 0xD8, b'E'];
                v.extend_from_slice(&w.to_be_bytes());
                v.extend_from_slice(&h.to_be_bytes());
                v.push(d[0]);
                v.extend_from_slice(&[0xFF, 0xD9]);
                v
            }),
            after_save: Arc::new(|_| {}),
            audit: Arc::new(|_, _, _| {}),
            backoff_ms: 30,
            idle: Duration::from_millis(4000),
        };
        let hub = Arc::new(Hub::new(env));
        let home = dir.join("home");
        let f = HubFeed { hub: hub.clone(), who: Arc::new(move |u| Who { user: u.into(), manage: false, usb_ok: true, home: home.clone(), may_write: Arc::new(|_| true) }) };
        (hub, f)
    }

    #[test]
    fn a_client_view_gets_jpeg_frames_and_the_machines_own_view_keeps_going() {
        let (hub, f) = feed("fanout", false);
        let me = Who { user: "ann".into(), manage: false, usb_ok: true, home: std::env::temp_dir(), may_write: Arc::new(|_| true) };
        // the machine's own view starts the capture
        let end = Instant::now() + Duration::from_secs(5);
        while hub.view(&me, "usb:video0", false).unwrap().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        // the client looks at the same camera
        let (seq, jpeg) = loop {
            if let Some(x) = f.jpeg("ann", "usb:video0", 0).unwrap() {
                break x;
            }
            assert!(Instant::now() < end, "no frame for the client");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(media::looks_like_jpeg(&jpeg) && seq >= 1);
        assert_eq!(hub.running(), 1, "one capture serves both viewers");
        // asking again with the same number gives nothing new until a newer picture exists
        assert!(f.jpeg("ann", "usb:video0", u64::MAX - 1).unwrap().is_none());
        // the client lets go; the machine's view is untouched
        drop(f);
        assert!(hub.view(&me, "usb:video0", false).unwrap().is_some(), "the machine still sees the camera");
        assert_eq!(hub.running(), 1);
        hub.stop_all();
    }

    #[test]
    fn a_camera_another_program_holds_is_reported_with_the_holders_name() {
        let (hub, f) = feed("busy", true);
        let end = Instant::now() + Duration::from_secs(5);
        let mut state = f.state("ann", "usb:video0");
        // the first look starts the stream, whose open then fails as busy
        while !matches!(state, CamState::Busy(_)) && Instant::now() < end {
            let _ = f.jpeg("ann", "usb:video0", 0);
            std::thread::sleep(Duration::from_millis(30));
            state = f.state("ann", "usb:video0");
        }
        assert!(matches!(state, CamState::Busy(ref h) if h.contains("Recorder") || !h.is_empty()), "{state:?}");
        assert_eq!(f.state("ann", "usb:nothing"), CamState::Missing);
        hub.stop_all();
    }
}

use super::*;
use crate::audiostream::{FRAME_SAMPLES, RATE};
use crate::cloudstream::Gate;
use crate::voiceassist::AudioIn;
use iyakki_shared::wakelisten::Mic;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::sync::Mutex;

fn tone(n: usize, seed: i32) -> Vec<i16> {
    (0..n).map(|i| (seed + (i as i32 * 7) % 4000) as i16).collect()
}

/// A microphone that hands out what the test puts in it.
#[derive(Clone, Default)]
struct FakeMic {
    inner: Arc<Mutex<(Vec<i16>, bool, u32)>>,
}
impl FakeMic {
    fn feed(&self, pcm: &[i16]) {
        self.inner.lock().unwrap().0.extend_from_slice(pcm);
    }
    fn opens(&self) -> u32 {
        self.inner.lock().unwrap().2
    }
}
impl Mic for FakeMic {
    fn open(&mut self) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        g.1 = true;
        g.2 += 1;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.inner.lock().unwrap().1
    }
    fn drain(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.inner.lock().unwrap().0)
    }
    fn alive(&mut self) -> bool {
        true
    }
    fn close(&mut self) {
        let mut g = self.inner.lock().unwrap();
        g.1 = false;
        g.0.clear();
    }
}

fn allowed_gate() -> Gate {
    let mut g = Gate::new(true, "tenant-a");
    g.press(true);
    g
}

#[test]
fn the_microphone_goes_through_the_gate_and_the_pinned_link_to_the_assistant_and_the_answer_comes_back() {
    // the cloud machine: its certificate for the device's sound, and a listener
    let cloud_id = crate::audiotls::Identity::make().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cloud_addr = listener.local_addr().unwrap();
    let cloud = CloudEnd::new();
    let mut ear = cloud.audio;
    let sink = cloud.sink;
    let scfg = crate::audiotls::server_config(&cloud_id).unwrap();
    let rx = std::thread::spawn(move || crate::audiotls::receive_frames(&listener, scfg, &mut |f| sink.send(f).unwrap()));
    ear.open().unwrap();
    // the device: the certificate arrived in the session
    let offer = CertOffer { machine: "tenant-a".into(), cert_der: cloud_id.cert_der.clone() };
    let offer = CertOffer::from_json(&offer.to_json()).unwrap();
    let mic = FakeMic::default();
    let mut dev = DeviceClient::new(allowed_gate(), Box::new(mic.clone()), cloud_addr, &offer.cert_der).unwrap();
    assert_eq!(dev.step(), Step::Gathering, "the microphone is opened and nothing is there yet");
    assert_eq!(mic.opens(), 1);
    let said = tone(FRAME_SAMPLES * BATCH_FRAMES, 100);
    mic.feed(&said);
    assert_eq!(dev.step(), Step::Sent(BATCH_FRAMES));
    assert_eq!(rx.join().unwrap(), Ok(BATCH_FRAMES as u32));
    assert_eq!(ear.take().unwrap(), said, "My IYAKKI on the cloud machine hears what the microphone heard");
    // the answer: the device listens with a certificate of its own
    let dev_id = crate::audiotls::Identity::make().unwrap();
    let dev_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dev_addr = dev_listener.local_addr().unwrap();
    let dcfg = crate::audiotls::server_config(&dev_id).unwrap();
    let rx = std::thread::spawn(move || {
        let mut p = AnswerPlayer::new();
        let n = p.receive(&dev_listener, dcfg).unwrap();
        (n, p.take())
    });
    let answer = tone(FRAME_SAMPLES * 12, 900);
    assert_eq!(send_answer(&answer, dev_addr, &dev_id.cert_der).unwrap(), 12);
    let (n, got) = rx.join().unwrap();
    assert_eq!((n, got), (12, answer), "the answer's sound arrives whole and in order");
    assert_eq!(RATE, 16_000);
}

#[test]
fn a_gate_that_says_no_closes_the_microphone_and_sends_nothing_even_when_sound_is_waiting() {
    struct Never;
    impl Link for Never {
        fn send(&mut self, _: &[Frame]) -> Result<(), String> {
            panic!("a refused gate must send nothing")
        }
    }
    let mic = FakeMic::default();
    // off: nothing is ever opened
    let mut dev = DeviceClient::with_link(Gate::new(false, "m"), Box::new(mic.clone()), Box::new(Never));
    assert!(matches!(dev.step(), Step::Refused(w) if w.contains("off")));
    assert_eq!(mic.opens(), 0);
    // the card is waiting: still nothing
    let mut dev = DeviceClient::with_link(Gate::new(true, "m"), Box::new(mic.clone()), Box::new(Never));
    assert!(matches!(dev.step(), Step::Refused(w) if w.contains("Waiting")));
    assert_eq!(mic.opens(), 0, "no capture while the question is open");
    // allowed, sound waiting, then mute: the microphone is closed and what was waiting is dropped
    dev.up.gate.press(true);
    assert_eq!(dev.step(), Step::Gathering);
    mic.feed(&tone(FRAME_SAMPLES * 2, 5));
    dev.up.gate.set_muted(true);
    assert!(matches!(dev.step(), Step::Refused(w) if w.contains("muted")));
    assert!(!mic.is_open() && mic.inner.lock().unwrap().0.is_empty());
    dev.up.gate.set_muted(false);
    assert_eq!(dev.step(), Step::Gathering, "after the mute it opens again and starts from nothing");
}

#[test]
fn a_dead_link_loses_the_batch_backs_off_and_gives_up_after_eight_failures_in_a_row() {
    struct Dead;
    impl Link for Dead {
        fn send(&mut self, _: &[Frame]) -> Result<(), String> {
            Err("could not reach the cloud machine".into())
        }
    }
    let mic = FakeMic::default();
    let mut dev = DeviceClient::with_link(allowed_gate(), Box::new(mic.clone()), Box::new(Dead));
    dev.step();
    mic.feed(&tone(FRAME_SAMPLES * BATCH_FRAMES, 1));
    assert!(matches!(dev.step(), Step::LinkDown(w) if w.contains("could not reach")), "the first failure");
    assert_eq!(dev.failures(), 1);
    for _ in 1..MAX_FAILURES {
        mic.feed(&tone(FRAME_SAMPLES * BATCH_FRAMES, 3));
        assert!(matches!(dev.step(), Step::LinkDown(_)));
    }
    assert_eq!(dev.failures(), MAX_FAILURES);
    // at the limit the loop ends after its first step and lets the microphone go
    let stop = AtomicBool::new(false);
    assert!(matches!(dev.run(&stop, Duration::from_millis(0)), Step::Gathering | Step::LinkDown(_)));
    assert!(!mic.is_open(), "the microphone was let go");
}

#[test]
fn a_certificate_offer_is_read_with_its_limits_and_the_wrong_certificate_never_gets_a_frame() {
    let id = crate::audiotls::Identity::make().unwrap();
    let ok = CertOffer { machine: "tenant-a".into(), cert_der: id.cert_der.clone() };
    assert_eq!(CertOffer::from_json(&ok.to_json()).unwrap(), ok);
    for bad in ["not json", r#"{"machine":"","cert":"AAAA"}"#, r#"{"machine":"m\u0007","cert":"AAAA"}"#, r#"{"machine":"m"}"#, r#"{"machine":"m","cert":"AAAA"}"#] {
        assert!(CertOffer::from_json(bad).is_err(), "{bad}");
    }
    let mut too_long = id.cert_der.clone();
    too_long.extend(vec![0u8; 5000]);
    assert!(CertOffer::from_json(&CertOffer { machine: "m".into(), cert_der: too_long }.to_json()).is_err());
    let mut cut = id.cert_der.clone();
    cut.truncate(cut.len() - 3);
    assert!(CertOffer::from_json(&CertOffer { machine: "m".into(), cert_der: cut }.to_json()).is_err(), "a DER sequence whose length does not match");
    // a device that was given another certificate refuses the link and not one frame is read
    let real = crate::audiotls::Identity::make().unwrap();
    let other = crate::audiotls::Identity::make().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let scfg = crate::audiotls::server_config(&real).unwrap();
    let got = Arc::new(Mutex::new(0usize));
    let g2 = got.clone();
    let rx = std::thread::spawn(move || crate::audiotls::receive_frames(&listener, scfg, &mut |_| *g2.lock().unwrap() += 1));
    let mic = FakeMic::default();
    let mut dev = DeviceClient::new(allowed_gate(), Box::new(mic.clone()), addr, &other.cert_der).unwrap();
    dev.step();
    mic.feed(&tone(FRAME_SAMPLES * BATCH_FRAMES, 8));
    assert!(matches!(dev.step(), Step::LinkDown(w) if w.contains("refused")), "the secure link was refused in the handshake");
    assert!(rx.join().unwrap().is_err() || *got.lock().unwrap() == 0);
    assert_eq!(*got.lock().unwrap(), 0, "nothing was delivered");
}

#[test]
fn the_listener_thread_offers_its_certificate_only_while_the_switch_is_on_and_takes_the_frames_only_then() {
    use std::sync::atomic::Ordering;
    let on = Arc::new(AtomicBool::new(true));
    let on2 = on.clone();
    let l = CloudListener::start("127.0.0.1:0", "", "tenant-a", Arc::new(move || on2.load(Ordering::SeqCst))).unwrap();
    let (addr, offer) = l.offer().unwrap();
    assert_eq!(addr, l.local_addr().to_string(), "nothing advertised: the bound address");
    let offer = CertOffer::from_json(&offer).unwrap();
    assert_eq!(offer.machine, "tenant-a");
    let mut ear = l.take_audio().unwrap();
    assert!(l.take_audio().is_none(), "the assistant's microphone is handed out once");
    ear.open().unwrap();
    // a device sends a batch while the switch is on: it is heard
    let mic = FakeMic::default();
    let mut dev = DeviceClient::new(allowed_gate(), Box::new(mic.clone()), l.local_addr(), &offer.cert_der).unwrap();
    dev.step();
    let first = tone(FRAME_SAMPLES * BATCH_FRAMES, 50);
    mic.feed(&first);
    assert_eq!(dev.step(), Step::Sent(BATCH_FRAMES));
    iyakki_testkit::wait::wait_until(Duration::from_secs(30), || l.served() >= 1);
    assert_eq!(ear.take().unwrap(), first);
    // the switch goes off: no offer, and a batch that still arrives is dropped
    on.store(false, Ordering::SeqCst);
    assert!(l.offer().unwrap_err().contains("off"));
    mic.feed(&tone(FRAME_SAMPLES * BATCH_FRAMES, 70));
    assert_eq!(dev.step(), Step::Sent(BATCH_FRAMES));
    iyakki_testkit::wait::wait_until(Duration::from_secs(30), || l.served() >= 2);
    assert!(ear.take().map_or(true, |v| v.is_empty()), "nothing was heard while the switch was off");
    // a peer that does not speak TLS does not stop the listener
    drop(std::net::TcpStream::connect(l.local_addr()).unwrap());
    iyakki_testkit::wait::wait_until(Duration::from_secs(30), || l.served() >= 3);
    // and the listener still takes a device after that: the switch on again, the offer is made again
    on.store(true, Ordering::SeqCst);
    assert!(l.offer().is_ok());
    drop(l);
}

#[test]
fn the_cloud_assistant_answers_through_the_devices_speaker_only_with_the_switch_on_and_a_return_link() {
    use crate::voiceassist::Speaker;
    use std::sync::atomic::Ordering;
    let on = Arc::new(AtomicBool::new(true));
    let on2 = on.clone();
    let l = Arc::new(CloudListener::start("127.0.0.1:0", "", "tenant-a", Arc::new(move || on2.load(Ordering::SeqCst))).unwrap());
    let dev_id = crate::audiotls::Identity::make().unwrap();
    let dev_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dev_addr = dev_listener.local_addr().unwrap();
    let dcfg = crate::audiotls::server_config(&dev_id).unwrap();
    let wav = crate::voice::encode_wav(&tone(FRAME_SAMPLES * 5, 300), 16_000);
    let l2 = l.clone();
    let mut mouth = CloudSpeaker::new(Arc::new(move |_| Ok(wav.clone())), Arc::new(move || l2.return_target()));
    assert!(mouth.say("hello").unwrap_err().contains("not offered"), "no return link yet: said in words, nothing sent");
    let offer = CertOffer { machine: "my-laptop".into(), cert_der: dev_id.cert_der.clone() }.to_json();
    assert!(l.set_return("not-an-address", &offer).is_err());
    assert!(l.set_return(&dev_addr.to_string(), "{}").is_err());
    on.store(false, Ordering::SeqCst);
    assert!(l.set_return(&dev_addr.to_string(), &offer).unwrap_err().contains("off"));
    assert!(l.return_target().is_none());
    on.store(true, Ordering::SeqCst);
    l.set_return(&dev_addr.to_string(), &offer).unwrap();
    let rx = std::thread::spawn(move || {
        let mut p = AnswerPlayer::new();
        p.receive(&dev_listener, dcfg).unwrap();
        p.take()
    });
    mouth.say("hello").unwrap();
    let got = rx.join().unwrap();
    assert_eq!(got.len(), FRAME_SAMPLES * 5, "the answer's sound reached the device's listener");
    iyakki_testkit::wait::wait_until(Duration::from_secs(30), || !mouth.busy());
    on.store(false, Ordering::SeqCst);
    assert!(mouth.say("again").is_err(), "the switch off: the return link is closed at once");
}

#[test]
fn the_machines_offer_needs_the_switch_and_an_address_then_starts_the_listener_once() {
    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    let d = crate::testdir::unique("cloudvoice", "offer");
    let svc = crate::services::Services::open(&d, Arc::new(Off), Arc::new(Off)).unwrap();
    let set = |on: bool| svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_cloud_stream = on).unwrap();
    // the switch is off by default: no offer, nothing listens
    assert!(svc.cloud_voice_offer().unwrap_err().contains("off"));
    assert!(svc.cloud_voice.lock().unwrap().is_none());
    set(true);
    assert!(svc.cloud_voice_offer_at("no-port-here").unwrap_err().contains("no audio address"));
    assert!(svc.cloud_voice.lock().unwrap().is_none(), "no address: still nothing listens");
    let (addr, offer) = svc.cloud_voice_offer_at("127.0.0.1:0").unwrap();
    let first = CertOffer::from_json(&offer).unwrap();
    assert!(addr.starts_with("127.0.0.1:") && !first.machine.is_empty());
    let (_, again) = svc.cloud_voice_offer_at("127.0.0.1:0").unwrap();
    assert_eq!(CertOffer::from_json(&again).unwrap().cert_der, first.cert_der, "one listener, one certificate");
    set(false);
    assert!(svc.cloud_voice_offer().unwrap_err().contains("off"), "off again: the offer is withdrawn");
}

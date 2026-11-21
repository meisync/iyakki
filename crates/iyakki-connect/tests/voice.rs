//! The connector's voice end (F-VOICE-61): consent first, then the microphone over a pinned TLS link, mute stops it at the source, the answer
//! comes back to a certificate of this computer. No agent code is involved: the connector crate builds without it.

use std::net::TcpListener;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use iyakki_audiolink::audiostream::{Frame, FRAME_SAMPLES};
use iyakki_audiolink::audiotls::{self, Identity};
use iyakki_audiolink::certoffer::CertOffer;
use iyakki_audiolink::device::{Link, BATCH_FRAMES};
use iyakki_audiolink::gate::Verdict;
use iyakki_audiolink::Mic;
use iyakki_connect::voice::DeviceVoice;

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
    fn open_now(&self) -> bool {
        self.inner.lock().unwrap().1
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

struct ChannelLink(Sender<Vec<Frame>>);

impl Link for ChannelLink {
    fn send(&mut self, frames: &[Frame]) -> Result<(), String> {
        self.0.send(frames.to_vec()).map_err(|e| e.to_string())
    }
}

fn tone(n: usize) -> Vec<i16> {
    (0..n).map(|i| ((i % 200) as i16 - 100) * 90).collect()
}

#[test]
fn nothing_starts_before_the_person_answered_the_card_and_a_no_stays_a_no() {
    let mic = FakeMic::default();
    let mut v = DeviceVoice::new(true, "tenant-a").unwrap();
    let (tx, _rx) = channel();
    v.connect_with(Box::new(ChannelLink(tx)));
    assert!(matches!(v.verdict(), Verdict::Card(t) if t.contains("tenant-a")));
    assert!(v.start(Box::new(mic.clone())).unwrap_err().contains("allow"));
    assert_eq!(mic.opens(), 0, "the microphone never opened");
    v.press(false);
    assert!(v.start(Box::new(mic.clone())).unwrap_err().contains("no"));
    let mut off = DeviceVoice::new(false, "tenant-a").unwrap();
    assert!(matches!(off.verdict(), Verdict::Off));
    off.press(true);
    assert!(off.start(Box::new(mic.clone())).is_err(), "the instance's switch is off: a press does not change that");
    assert!(!v.running() && !off.running());
}

#[test]
fn an_allowed_session_streams_the_microphone_and_mute_stops_it_at_the_source() {
    let mic = FakeMic::default();
    let mut v = DeviceVoice::new(true, "tenant-a").unwrap();
    let (tx, rx) = channel();
    v.connect_with(Box::new(ChannelLink(tx)));
    v.press(true);
    mic.feed(&tone(FRAME_SAMPLES * BATCH_FRAMES));
    v.start(Box::new(mic.clone())).unwrap();
    assert!(v.start(Box::new(mic.clone())).is_err(), "a second start is refused");
    let frames = rx.recv().unwrap();
    assert_eq!(frames.len(), BATCH_FRAMES, "the first batch arrives whole");
    assert!(mic.opens() >= 1);
    let end = v.mute();
    assert!(end.is_some());
    assert!(!mic.open_now(), "mute closed the microphone itself");
    assert!(!v.running());
    // sending again needs a new answer to the card
    assert!(matches!(v.verdict(), Verdict::Muted | Verdict::Card(_)));
    assert!(v.start(Box::new(FakeMic::default())).is_err());
}

#[test]
fn the_pinned_tls_link_carries_the_sound_and_the_answer_comes_back_to_this_computers_certificate() {
    // the cloud machine's side, with the certificate it sent in the session
    let cloud_id = Identity::make().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let scfg = audiotls::server_config(&cloud_id).unwrap();
    let got = Arc::new(Mutex::new(Vec::<i16>::new()));
    let g2 = got.clone();
    let rx = std::thread::spawn(move || {
        let mut jitter = iyakki_audiolink::audiostream::Jitter::new(3);
        audiotls::receive_frames(&listener, scfg, &mut |f| {
            jitter.push(f);
            while let Some(p) = jitter.pop() {
                g2.lock().unwrap().extend(p);
            }
        })
        .unwrap();
        g2.lock().unwrap().extend(jitter.drain());
    });
    let mut v = DeviceVoice::new(true, "tenant-a").unwrap();
    let offer = CertOffer { machine: "tenant-a".into(), cert_der: cloud_id.cert_der.clone() }.to_json();
    assert_eq!(v.connect(addr, &offer).unwrap(), "tenant-a");
    assert!(v.connect(addr, "{\"machine\":\"x\",\"cert\":\"AAAA\"}").is_err(), "a certificate that is not one is refused");
    v.press(true);
    let mic = FakeMic::default();
    let said = tone(FRAME_SAMPLES * BATCH_FRAMES);
    mic.feed(&said);
    v.start(Box::new(mic)).unwrap();
    rx.join().unwrap();
    v.mute();
    assert_eq!(*got.lock().unwrap(), said, "the cloud machine heard what the microphone heard");
    // the answer: this computer's certificate, pinned by the cloud machine
    let back = TcpListener::bind("127.0.0.1:0").unwrap();
    let back_addr = back.local_addr().unwrap();
    let offer = CertOffer::from_json(&v.answer_offer("this computer").to_json()).unwrap();
    let answer = tone(FRAME_SAMPLES * 7);
    let a2 = answer.clone();
    let tx = std::thread::spawn(move || {
        let frames = iyakki_audiolink::audiostream::cut(&a2, 0, 0);
        audiotls::send_frames(back_addr, audiotls::client_config(&offer.cert_der).unwrap(), &frames)
    });
    assert_eq!(v.hear_answer(&back).unwrap(), answer);
    tx.join().unwrap().unwrap();
}

struct ChannelSink(std::sync::mpsc::Sender<Vec<i16>>);
impl iyakki_connect::voicecapture::Sink for ChannelSink {
    fn play(&mut self, samples: &[i16]) -> Result<(), String> {
        self.0.send(samples.to_vec()).map_err(|e| e.to_string())
    }
}

fn cloud_end() -> (std::net::SocketAddr, String, std::thread::JoinHandle<()>) {
    let id = Identity::make().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = audiotls::server_config(&id).unwrap();
    let offer = CertOffer { machine: "tenant-a".into(), cert_der: id.cert_der.clone() }.to_json();
    (addr, offer, std::thread::spawn(move || {
        let _ = audiotls::receive_frames(&listener, cfg, &mut |_| {});
    }))
}

#[test]
fn the_answer_comes_back_to_this_computer_only_after_a_yes_and_plays_through_the_sink() {
    use iyakki_audiolink::audiostream::cut;
    use iyakki_connect::voice::{session_with_return, Return, ReturnEnd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    let (caddr, coffer, cloud) = cloud_end();
    let end = Arc::new(ReturnEnd::new("127.0.0.1:0", "127.0.0.1:0").unwrap());
    let (offer_tx, offer_rx) = channel::<(String, String)>();
    let (play_tx, play_rx) = channel::<Vec<i16>>();
    let told = Arc::new(Mutex::new(0u32));
    let t2 = told.clone();
    let ret = Return {
        end: end.clone(),
        sink: Box::new(ChannelSink(play_tx)),
        tell: Box::new(move |a, o| {
            *t2.lock().unwrap() += 1;
            offer_tx.send((a.to_string(), o.to_string())).map_err(|e| e.to_string())
        }),
    };
    // a "no" at the card: the instance is told nothing and nothing listens
    let never = AtomicBool::new(true);
    let no = Return { end: end.clone(), sink: Box::new(ChannelSink(channel().0)), tell: Box::new(|_, _| panic!("told before a yes")) };
    let e = session_with_return((caddr.to_string(), coffer.clone()), &mut std::io::Cursor::new("n\n"), &mut Vec::new(), Box::new(FakeMic::default()), &never, Some(no)).unwrap_err();
    assert!(e.contains("did not allow"));
    // a "y": the offer goes to the instance, the answer arrives at the advertised address and is played
    let stop = AtomicBool::new(false);
    let mic = FakeMic::default();
    let said = tone(FRAME_SAMPLES * BATCH_FRAMES);
    mic.feed(&said);
    let mut out = Vec::new();
    std::thread::scope(|sc| {
        let runner = sc.spawn(|| session_with_return((caddr.to_string(), coffer), &mut std::io::Cursor::new("y\n"), &mut out, Box::new(mic.clone()), &stop, Some(ret)));
        let (addr, offer) = offer_rx.recv().unwrap();
        assert_eq!(addr, end.local_addr().unwrap().to_string(), "the advertised port is the bound one");
        let o = CertOffer::from_json(&offer).unwrap();
        assert_eq!(o.machine, "tenant-a");
        let answer = tone(FRAME_SAMPLES * 4);
        audiotls::send_frames(addr.parse().unwrap(), audiotls::client_config(&o.cert_der).unwrap(), &cut(&answer, 0, 0)).unwrap();
        assert_eq!(play_rx.recv().unwrap(), answer, "the answer's sound reaches the sink whole");
        stop.store(true, Ordering::SeqCst);
        runner.join().unwrap().unwrap();
    });
    assert!(*told.lock().unwrap() == 1);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("will play on this computer") && text.contains("Stopped"), "{text}");
    drop(cloud);
}

#[test]
fn an_instance_that_refuses_the_return_offer_leaves_the_answer_as_words_and_the_microphone_still_goes() {
    use iyakki_connect::voice::{session_with_return, Return, ReturnEnd};
    use std::sync::atomic::AtomicBool;
    let (caddr, coffer, cloud) = cloud_end();
    let end = Arc::new(ReturnEnd::new("127.0.0.1:0", "127.0.0.1:0").unwrap());
    let ret = Return { end, sink: Box::new(ChannelSink(std::sync::mpsc::channel().0)), tell: Box::new(|_, _| Err("no cloud voice session is listening on this machine".into())) };
    let mic = FakeMic::default();
    mic.feed(&tone(FRAME_SAMPLES * BATCH_FRAMES));
    let stop = AtomicBool::new(false);
    let mut out = Vec::new();
    std::thread::scope(|sc| {
        let r = sc.spawn(|| session_with_return((caddr.to_string(), coffer), &mut std::io::Cursor::new("y\n"), &mut out, Box::new(mic.clone()), &stop, Some(ret)));
        // the cloud end took the batch and closed: the link gives up and the session ends by itself
        cloud.join().unwrap();
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        r.join().unwrap().unwrap();
    });
    assert!(String::from_utf8(out).unwrap().contains("The answer will stay words: no cloud voice session"));
}

#[test]
fn the_return_address_must_be_an_ip_address_and_port() {
    use iyakki_connect::voice::ReturnEnd;
    for bad in ["", "host.example:1", "1.2.3.4", "1.2.3.4:x"] {
        assert!(ReturnEnd::new("127.0.0.1:0", bad).is_err(), "{bad}");
    }
    assert!(ReturnEnd::new("127.0.0.1:0", "127.0.0.1:9").is_ok());
}

// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! `iyakki-connect voice` (F-VOICE-38, F-VOICE-61): the connector asks the signed-in instance for the audio address and the one-link
//! certificate over a fake tenant on real TLS, puts the card to the person, and only after a "y" sends a fake microphone over a pinned TLS
//! link to a fake cloud end. It proves the connector's handling, not a microphone or two machines.

mod common;

use std::io::Cursor;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_audiolink::audiostream::FRAME_SAMPLES;
use iyakki_audiolink::audiotls::{self, Identity};
use iyakki_audiolink::certoffer::CertOffer;
use iyakki_audiolink::Mic;
use iyakki_connect::{pins::Store, run, voice, Ctx, Fail};
use std::time::Duration;

#[derive(Clone, Default)]
struct FakeMic {
    inner: Arc<Mutex<(Vec<i16>, bool, u32)>>,
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
        self.inner.lock().unwrap().1 = false;
    }
}

fn go(c: &Ctx, input: &str, args: &[&str]) -> (Result<(), Fail>, String) {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let r = run(&a, c, &mut Cursor::new(input.to_string()), &mut out);
    (r, String::from_utf8(out).unwrap())
}

#[test]
fn the_voice_command_asks_the_instance_for_the_offer_then_the_person_and_only_then_sends_sound() {
    let r = common::start_remote();
    let d = std::env::temp_dir().join(format!("iyakki-connect-test-voice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let c = Ctx { store: Store::new(&d), timeout: Duration::from_secs(5), interactive: false };
    let addr = &r.target.addr;
    // not pinned: nothing is asked; no offer on the server: a plain sentence
    assert!(matches!(go(&c, "right\n", &["voice", addr, common::USER]).0, Err(Fail::Error(m)) if m.contains("not pinned")));
    go(&c, "", &["pin", addr, "--yes"]).0.unwrap();
    let (res, _) = go(&c, "right\ny\n", &["voice", addr, common::USER]);
    assert!(matches!(res, Err(Fail::Error(ref m)) if m.contains("does not take a microphone")), "the server has no offer: {res:?}");
    assert!(matches!(go(&c, "wrong\n", &["voice", addr, common::USER]).0, Err(Fail::Error(m)) if m.contains("refused")));

    // the fake cloud end: a certificate for the link and a listener that counts what arrives
    let id = Identity::make().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cloud = listener.local_addr().unwrap();
    let cfg = audiotls::server_config(&id).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (stop2, got) = (stop.clone(), Arc::new(Mutex::new(0u32)));
    let got2 = got.clone();
    let rx = std::thread::spawn(move || {
        let n = audiotls::receive_frames(&listener, cfg, &mut |_| *got2.lock().unwrap() += 1).unwrap();
        stop2.store(true, Ordering::SeqCst); // the batch is in: the session may end
        n
    });
    let offer = CertOffer { machine: "tenant-a".into(), cert_der: id.cert_der.clone() };
    *common::VOICE_OFFER.lock().unwrap() = serde_json::json!({"addr": cloud.to_string(), "offer": offer.to_json()}).to_string();

    // a "no" (or no answer) at the card: the microphone is never opened and nothing is sent
    let mic = FakeMic::default();
    let target = iyakki_link::cloudclient::Target { addr: addr.clone(), pin: c.store.get(addr).unwrap().unwrap() };
    let api = iyakki_link::rawlink::FilesApi::login(&target, common::USER, common::PASSWORD).unwrap();
    let offer_pair = api.voice_offer().unwrap();
    assert_eq!(offer_pair.0, cloud.to_string());
    let mut out = Vec::new();
    let never = AtomicBool::new(true);
    let e = voice::session(offer_pair.clone(), &mut Cursor::new("n\n"), &mut out, Box::new(mic.clone()), &never).unwrap_err();
    assert!(e.contains("did not allow") && mic.inner.lock().unwrap().2 == 0, "{e}");
    assert!(String::from_utf8(out).unwrap().contains("tenant-a"), "the card names the machine");
    let e = voice::session(offer_pair.clone(), &mut Cursor::new(""), &mut Vec::new(), Box::new(mic.clone()), &never).unwrap_err();
    assert!(e.contains("did not allow") && mic.inner.lock().unwrap().2 == 0, "no answer is a no: {e}");

    // a "y": the sound goes over the pinned link until the cloud end has it
    mic.inner.lock().unwrap().0.extend(vec![100i16; FRAME_SAMPLES * 5]);
    let mut out = Vec::new();
    voice::session(offer_pair, &mut Cursor::new("y\n"), &mut out, Box::new(mic.clone()), &stop).unwrap();
    assert!(rx.join().unwrap() >= 5);
    assert!(*got.lock().unwrap() >= 5);
    assert!(!mic.is_open(), "the microphone is closed at the end");
    assert!(String::from_utf8(out).unwrap().contains("Stopped"));
}

use super::*;
use std::net::SocketAddr;
use crate::audiostream::{FRAME_SAMPLES, RATE};
use crate::voiceassist::AudioIn;
use std::net::TcpListener;

fn tone(n: usize) -> Vec<i16> {
    (0..n).map(|i| (0x1234 + (i as i32 * 5) % 3000) as i16).collect()
}

#[test]
fn nothing_leaves_until_the_switch_is_on_the_card_is_pressed_and_the_microphone_is_not_muted() {
    let mut g = Gate::new(false, "tenant-a");
    assert_eq!(g.verdict(), Verdict::Off, "off by default: no card either");
    g.press(true);
    assert_eq!(g.verdict(), Verdict::Off, "a press with the switch off answers nothing");
    g.set_enabled(true);
    match g.verdict() {
        Verdict::Card(t) => assert!(t.contains("tenant-a") && t.contains("leaves this device") && t.contains("tap or a key") && t.contains("saying yes does not"), "{t}"),
        v => panic!("{v:?}"),
    }
    let mut up = Uplink::new(g);
    assert!(up.push_pcm(&tone(FRAME_SAMPLES)).unwrap_err().contains("Waiting for you"));
    up.gate.press(true);
    assert_eq!(up.gate.verdict(), Verdict::Allowed);
    assert_eq!(up.push_pcm(&tone(FRAME_SAMPLES * 3)).unwrap(), 3);
    // mute stops it at the source: nothing is queued, and what was queued is dropped at the next flush
    up.gate.set_muted(true);
    assert!(up.push_pcm(&tone(FRAME_SAMPLES)).unwrap_err().contains("muted"));
    let dead: SocketAddr = "127.0.0.1:9".parse().unwrap();
    let id = crate::audiotls::Identity::make().unwrap();
    assert!(up.flush(dead, crate::audiotls::client_config(&id.cert_der).unwrap()).unwrap_err().contains("muted"));
    up.gate.set_muted(false);
    assert_eq!(up.gate.verdict(), Verdict::Allowed, "the answer stands for the session");
    // a new session asks again; so does switching it off and on
    up.gate.new_session();
    assert!(matches!(up.gate.verdict(), Verdict::Card(_)));
    up.gate.press(false);
    assert_eq!(up.gate.verdict(), Verdict::Denied);
    assert!(up.push_pcm(&tone(FRAME_SAMPLES)).unwrap_err().contains("said no"));
    up.gate.press(true);
    assert_eq!(up.gate.verdict(), Verdict::Denied, "the first answer of a session stands");
    up.gate.set_enabled(false);
    up.gate.set_enabled(true);
    assert!(matches!(up.gate.verdict(), Verdict::Card(_)));
}

#[test]
fn the_gate_has_no_way_in_for_words() {
    // the only way to answer is `press(bool)`, called by the screen; a spoken yes is not an input of this module at all
    let src = include_str!("cloudstream.rs");
    let code = src.split("#[cfg(test)]").next().unwrap();
    assert!(!code.contains("transcript") && !code.contains("Heard") && !code.contains("spoken_approvals"));
}

#[test]
fn microphone_sound_goes_from_the_device_through_tls_to_the_cloud_machines_assistant_and_is_heard_whole() {
    let id = crate::audiotls::Identity::make().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let scfg = crate::audiotls::server_config(&id).unwrap();
    // the cloud machine: frames into a microphone that My IYAKKI reads
    let (mut mic, sink) = DownlinkAudio::new();
    let rx = std::thread::spawn(move || crate::audiotls::receive_frames(&listener, scfg, &mut |f| sink.send(f).unwrap()));
    mic.open().unwrap();
    // the device: the person allowed it
    let mut up = Uplink::new(Gate::new(true, "tenant-a"));
    up.gate.press(true);
    let pcm = tone(FRAME_SAMPLES * 9);
    assert_eq!(up.push_pcm(&pcm).unwrap(), 9);
    assert_eq!(up.flush(addr, crate::audiotls::client_config(&id.cert_der).unwrap()).unwrap(), 9);
    assert_eq!(rx.join().unwrap(), Ok(9));
    let heard = mic.take().unwrap();
    assert_eq!(heard, pcm, "the assistant hears what the device sent, nothing added, nothing lost");
    assert!(!mic.is_open() && mic.take().unwrap().is_empty(), "taken once");
    assert_eq!(RATE, 16_000);
}

#[test]
fn a_slow_link_keeps_only_the_newest_200_milliseconds_and_counts_the_rest() {
    let mut up = Uplink::new(Gate::new(true, "m"));
    up.gate.press(true);
    up.push_pcm(&tone(FRAME_SAMPLES * 30)).unwrap();
    assert_eq!(up.dropped(), 20, "30 frames of 20 ms, 10 kept");
}

#[test]
fn a_cloud_microphone_that_is_discarded_forgets_what_it_heard() {
    let (mut mic, sink) = DownlinkAudio::new();
    mic.open().unwrap();
    for f in crate::audiostream::cut(&tone(FRAME_SAMPLES * 6), 0, 0) {
        sink.send(f).unwrap();
    }
    assert!(mic.level() > 0.0);
    mic.discard();
    assert!(!mic.is_open());
    mic.open().unwrap();
    assert!(mic.take().unwrap().is_empty());
}

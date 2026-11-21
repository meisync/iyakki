use super::*;
use crate::audiostream::{cut, Jitter, FRAME_SAMPLES};
use std::io::{Read, Write};
use std::sync::Mutex;

fn tone(n: usize) -> Vec<i16> {
    // a recognisable, never repeating pattern: 0x7A31 + step, so its bytes can be searched for on the wire
    (0..n).map(|i| (0x7A31i32 + (i as i32 * 7) % 4000) as i16).collect()
}

fn receiver(id: &Identity) -> (SocketAddr, std::thread::JoinHandle<(Result<u32, String>, Vec<Frame>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = server_config(id).unwrap();
    let h = std::thread::spawn(move || {
        let mut got = vec![];
        let r = receive_frames(&listener, cfg, &mut |f| got.push(f));
        (r, got)
    });
    (addr, h)
}

#[test]
fn sound_crosses_a_pinned_tls_link_in_order_and_comes_out_as_it_went_in() {
    let id = Identity::make().unwrap();
    let (addr, rx) = receiver(&id);
    let pcm = tone(FRAME_SAMPLES * 25);
    let frames = cut(&pcm, 40, 0);
    send_frames(addr, client_config(&id.cert_der).unwrap(), &frames).unwrap();
    let (r, got) = rx.join().unwrap();
    assert_eq!(r, Ok(25));
    let mut j = Jitter::new(3);
    let mut out = vec![];
    for f in got {
        j.push(f);
        while let Some(p) = j.pop() {
            out.extend(p);
        }
    }
    out.extend(j.drain());
    assert_eq!(out, pcm);
}

#[test]
fn the_bytes_on_the_wire_are_encrypted_and_the_sound_is_not_in_them() {
    let id = Identity::make().unwrap();
    let (server_addr, rx) = receiver(&id);
    // a relay between the two that records everything the device sent
    let relay = TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_addr = relay.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let seen2 = seen.clone();
    let rl = std::thread::spawn(move || {
        let (mut c, _) = relay.accept().unwrap();
        let mut s = TcpStream::connect(server_addr).unwrap();
        let mut s2 = s.try_clone().unwrap();
        let mut c2 = c.try_clone().unwrap();
        let back = std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            while let Ok(n) = s2.read(&mut b) {
                if n == 0 || c2.write_all(&b[..n]).is_err() {
                    break;
                }
            }
        });
        let mut b = [0u8; 4096];
        while let Ok(n) = c.read(&mut b) {
            if n == 0 {
                break;
            }
            seen2.lock().unwrap().extend_from_slice(&b[..n]);
            if s.write_all(&b[..n]).is_err() {
                break;
            }
        }
        let _ = s.shutdown(std::net::Shutdown::Write);
        let _ = back.join();
    });
    let pcm = tone(FRAME_SAMPLES * 20);
    let frames = cut(&pcm, 0, 0);
    send_frames(relay_addr, client_config(&id.cert_der).unwrap(), &frames).unwrap();
    let (r, _) = rx.join().unwrap();
    assert_eq!(r, Ok(20));
    rl.join().unwrap();
    let wire = seen.lock().unwrap().clone();
    assert!(wire.len() > 20 * 640, "the frames did cross: {} bytes", wire.len());
    // the plain frame bytes of any frame, and even 12 sound bytes in a row, appear nowhere on the wire
    for f in frames.iter().take(5) {
        let plain = f.encode();
        assert!(!wire.windows(24).any(|w| plain.windows(24).any(|p| p == w)), "a piece of a plain frame is on the wire");
    }
    let sample_bytes: Vec<u8> = pcm[..6].iter().flat_map(|s| s.to_le_bytes()).collect();
    assert!(!wire.windows(sample_bytes.len()).any(|w| w == sample_bytes.as_slice()), "sound is visible on the wire");
}

#[test]
fn a_certificate_that_is_not_the_pinned_one_is_refused_before_any_frame_leaves() {
    let real = Identity::make().unwrap();
    let other = Identity::make().unwrap();
    let (addr, rx) = receiver(&real);
    let frames = cut(&tone(FRAME_SAMPLES * 5), 0, 0);
    // the device was given ANOTHER certificate (a different machine, or a man in the middle answering): the real server's is not trusted
    let e = send_frames(addr, client_config(&other.cert_der).unwrap(), &frames).unwrap_err();
    assert!(e.contains("refused") || e.contains("secure link"), "{e}");
    let (r, got) = rx.join().unwrap();
    assert!(r.is_err() && got.is_empty(), "the server read no frame: {r:?}");
}

#[test]
fn a_peer_that_does_not_speak_tls_never_has_a_frame_read() {
    let id = Identity::make().unwrap();
    let (addr, rx) = receiver(&id);
    let frames = cut(&tone(FRAME_SAMPLES * 5), 0, 0);
    let mut tcp = TcpStream::connect(addr).unwrap();
    for f in &frames {
        let _ = write_frame(&mut tcp, f);
    }
    drop(tcp);
    let (r, got) = rx.join().unwrap();
    assert!(r.is_err() && got.is_empty(), "plain frames are not accepted: {r:?}");
}

#[test]
fn a_stream_cut_without_a_proper_end_is_an_error_not_a_clean_end() {
    let id = Identity::make().unwrap();
    let (addr, rx) = receiver(&id);
    let cfg = client_config(&id.cert_der).unwrap();
    let tcp = TcpStream::connect(addr).unwrap();
    let mut tls = StreamOwned::new(ClientConnection::new(cfg, ServerName::try_from(NAME).unwrap()).unwrap(), tcp);
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).unwrap();
    }
    let f = &cut(&tone(FRAME_SAMPLES), 0, 0)[0];
    write_frame(&mut tls, f).unwrap();
    tls.flush().unwrap();
    drop(tls); // the TCP connection ends with no TLS close
    let (r, got) = rx.join().unwrap();
    assert!(r.is_err(), "a cut stream is not mistaken for a finished one: {r:?}");
    assert_eq!(got.len(), 1, "what arrived before the cut stays delivered");
}

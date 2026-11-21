use super::*;
use crate::avs::{recognize_event, build_recognize, parse_multipart, interpret, Session, Tokens, Http};
use iyakki_proxy::h2client::Dialed;
use iyakki_proxy::hpack::{Decoder, Encoder};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;

fn read_frame<S: Read>(s: &mut S) -> Option<(u8, u8, u32, Vec<u8>)> {
    let mut h = [0u8; 9];
    s.read_exact(&mut h).ok()?;
    let n = (h[0] as usize) << 16 | (h[1] as usize) << 8 | h[2] as usize;
    let mut p = vec![0u8; n];
    s.read_exact(&mut p).ok()?;
    Some((h[3], h[4], u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff, p))
}

/// What the fake gateway saw: (method, path, authorization, content type, body length) of each request.
type Seen = Arc<Mutex<Vec<(String, String, String, String, usize)>>>;

fn speak_answer(boundary: &str) -> Vec<u8> {
    let mut b = Vec::new();
    let mut p = |h: &str, body: &[u8]| {
        b.extend_from_slice(format!("--{boundary}\r\n{h}\r\n\r\n").as_bytes());
        b.extend_from_slice(body);
        b.extend_from_slice(b"\r\n");
    };
    p("Content-Type: application/json; charset=UTF-8", br#"{"directive":{"header":{"namespace":"SpeechSynthesizer","name":"Speak","messageId":"a"},"payload":{"url":"cid:audio1","token":"tok-1","format":"AUDIO_MPEG"}}}"#);
    p("Content-Type: application/octet-stream\r\nContent-ID: <audio1>", &[0xff, 0xfb, 0x90, 0x00, 1, 2, 3]);
    b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    b
}

/// A gateway written with the codecs of the proxy crate: the downchannel is held open and never answered, an event gets a Speak answer.
fn serve(l: TcpListener, seen: Seen) {
    serve_with(l, seen, false)
}

/// `answer_down`: the downchannel is answered with a Speak directive instead of being held open.
fn serve_with(l: TcpListener, seen: Seen, answer_down: bool) {
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut pre = [0u8; 24];
                if s.read_exact(&mut pre).is_err() || pre != *iyakki_proxy::h2::PREFACE {
                    return;
                }
                let mut o = Vec::new();
                iyakki_proxy::h2::frame(&mut o, 4, 0, 0, &[]);
                s.write_all(&o).unwrap();
                let (mut dec, mut enc) = (Decoder::new(4096), Encoder::default());
                let mut pending: Vec<(u32, Vec<(String, String)>, usize, bool)> = Vec::new();
                while let Some((kind, flags, id, p)) = read_frame(&mut s) {
                    match kind {
                        4 if flags & 1 == 0 => {
                            let mut o = Vec::new();
                            iyakki_proxy::h2::frame(&mut o, 4, 1, 0, &[]);
                            s.write_all(&o).unwrap();
                        }
                        1 => pending.push((id, dec.decode(&p).unwrap(), 0, flags & 1 != 0)),
                        0 => {
                            if let Some(r) = pending.iter_mut().find(|r| r.0 == id) {
                                r.2 += p.len();
                                r.3 = flags & 1 != 0;
                            }
                        }
                        _ => {}
                    }
                    let mut i = 0;
                    while i < pending.len() {
                        if !pending[i].3 {
                            i += 1;
                            continue;
                        }
                        let (id, hs, len, _) = pending.remove(i);
                        let get = |n: &str| hs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap_or_default();
                        seen.lock().unwrap().push((get(":method"), get(":path"), get("authorization"), get("content-type"), len));
                        if get(":path") == DIRECTIVES_PATH && !answer_down {
                            continue; // the downchannel: held open, no answer
                        }
                        let body = speak_answer("gw");
                        let block = enc.encode(&[(":status".into(), "200".into()), ("content-type".into(), "multipart/related; boundary=gw".into()), ("content-length".into(), body.len().to_string())]);
                        let mut o = Vec::new();
                        iyakki_proxy::h2::frame(&mut o, 1, 0x4, id, &block);
                        iyakki_proxy::h2::frame(&mut o, 0, 1, id, &body);
                        s.write_all(&o).unwrap();
                    }
                }
            });
        }
    });
}

fn dialer(port: u16) -> Dialer {
    Arc::new(move |_h: &str, _p: u16| {
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        s.set_nodelay(true).ok();
        Ok(Dialed::H2(Box::new(s)))
    })
}

fn cfg(port: u16) -> AvsConfig {
    let mut c = AvsConfig::parse(r#"{"client_id":"amzn1.application-oa2-client.x","product_id":"iyakki-test"}"#).unwrap();
    c.port = port;
    c
}

struct NoHttp;
impl Http for NoHttp {
    fn post_form(&self, _: &str, _: &[(&str, &str)]) -> Result<(u16, String), String> {
        Err("no network".into())
    }
}

#[test]
fn an_event_goes_out_over_http2_with_the_bearer_token_and_the_speak_answer_comes_back_through_the_session() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    serve(l, seen.clone());
    let mut t = H2Events::new(&cfg(port), dialer(port));
    t.open_downchannel("Atza|good");
    let (ct, body) = build_recognize(&recognize_event("m1", "d1"), &[5u8; 3200]).unwrap();
    let (status, rct, rbody) = t.post_event("Atza|good", &ct, &body).unwrap();
    assert_eq!(status, 200);
    let turn = interpret(&parse_multipart(&rct, &rbody).unwrap()).unwrap();
    assert_eq!(turn.speech.len(), 1, "the spoken answer reaches the provider");
    let s = seen.lock().unwrap().clone();
    let ev = s.iter().find(|r| r.1 == EVENTS_PATH).expect("the event arrived");
    assert_eq!((ev.0.as_str(), ev.2.as_str()), ("POST", "Bearer Atza|good"));
    assert!(ev.3.starts_with("multipart/form-data; boundary=") && ev.4 == body.len(), "{ev:?}");
    // a whole turn through the Session type, as the voice seam runs it
    let mut sess = Session { client_id: "cid".into(), tokens: Tokens { access: "Atza|good".into(), refresh: "r".into(), expires_in: 3600 }, age_secs: 0, http: &NoHttp };
    let turn = sess.turn(&mut t, &[1u8; 640]).unwrap();
    assert_eq!(turn.speech.len(), 1);
    t.close();
}

#[test]
fn the_downchannel_is_opened_first_once_with_the_bearer_token_and_never_answered_by_the_event() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    serve(l, seen.clone());
    let mut t = H2Events::new(&cfg(port), dialer(port));
    t.open_downchannel("Atza|d");
    t.open_downchannel("Atza|d");
    assert!(t.downchannel_open(), "a second open does not start a second stream");
    // `open_downchannel` returns at once and the request leaves from a background thread: wait until the gateway has the downchannel before
    // the event goes out (the order the product promises to its caller), instead of hoping the thread was scheduled first
    iyakki_testkit::wait_until(Duration::from_secs(60), || seen.lock().unwrap().iter().any(|r| r.1 == DIRECTIVES_PATH));
    // the event's answer proves the gateway has handled everything sent before it on the connection
    let (ct, body) = build_recognize(&recognize_event("m", "d"), &[1u8; 640]).unwrap();
    t.post_event("Atza|d", &ct, &body).unwrap();
    let s = seen.lock().unwrap().clone();
    let down: Vec<_> = s.iter().filter(|r| r.1 == DIRECTIVES_PATH).collect();
    assert_eq!(down.len(), 1);
    assert_eq!((down[0].0.as_str(), down[0].2.as_str()), ("GET", "Bearer Atza|d"));
    assert!(t.downchannel_open(), "still waiting for directives");
}

#[test]
fn without_the_owners_client_id_nothing_is_set_up_and_nothing_is_sent() {
    let d = std::env::temp_dir().join(format!("iyakki-avs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(AvsConfig::load(&d).unwrap_err(), NO_CLIENT_ID);
    for bad in [r#"{"client_id":"","product_id":"p"}"#, r#"{"client_id":"c"}"#, r#"{"client_id":"a b","product_id":"p"}"#, "not json", r#"{"client_id":"c","product_id":"p","gateway":"evil host/x"}"#] {
        assert!(AvsConfig::parse(bad).is_err(), "{bad}");
    }
    std::fs::write(d.join(CONFIG_FILE), r#"{"client_id":"amzn1.x","product_id":"p"}"#).unwrap();
    let c = AvsConfig::load(&d).unwrap();
    assert_eq!((c.gateway.as_str(), c.port), (GATEWAY, 443));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_gateway_that_does_not_speak_http2_is_an_error_in_words() {
    let mut t = H2Events::new(&cfg(1), Arc::new(|_, _| Ok(Dialed::NotH2)));
    let e = t.post_event("x", "multipart/form-data; boundary=b", b"x").unwrap_err();
    assert!(e.contains("HTTP/2"), "{e}");
    let mut dead = H2Events::new(&cfg(1), Arc::new(|_, _| Err("no route".to_string())));
    assert!(dead.post_event("x", "multipart/form-data; boundary=b", b"x").is_err());
}

#[test]
fn a_directive_that_arrives_on_the_downchannel_is_interpreted_and_kept_and_its_speech_becomes_a_wav() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    serve_with(l, seen.clone(), true);
    let t = H2Events::new(&cfg(port), dialer(port));
    assert!(t.take_directives().is_empty());
    t.open_downchannel("Atza|good");
    let turns = iyakki_testkit::wait::wait_for_state(Duration::from_secs(30), || {
        let v = t.take_directives();
        if v.is_empty() { Err("nothing yet".to_string()) } else { Ok(v) }
    });
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].speech.len(), 1, "the Speak directive's audio came with it");
    assert_eq!(turns[0].report.len(), 2, "speech started and finished are ready to be reported");
    // a piece that is not sound is an error in words; a real MP3 comes out as a WAV
    let e = turns[0].speech_wav().unwrap_err();
    assert!(e.contains("could not be played"), "{e}");
    let mp3 = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/samples/Music/IYAKKI Sample Beats.mp3")).unwrap();
    let turn = crate::avs::Turn { speech: vec![mp3[..24_000].to_vec()], ..Default::default() };
    let wav = turn.speech_wav().unwrap();
    assert_eq!(&wav[0][..4], b"RIFF");
    let already = crate::avs::Turn { speech: vec![wav[0].clone()], ..Default::default() };
    assert_eq!(already.speech_wav().unwrap(), wav, "a WAV is passed through");
}

/// A TLS server with a fresh self-signed certificate that speaks no HTTP/2: only the handshake is of interest.
fn self_signed_server() -> (u16, Vec<u8>) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let der = ck.cert.der().to_vec();
    let key = PrivateKeyDer::try_from(ck.key_pair.serialize_der()).unwrap();
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(der.clone())], key).unwrap();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    let cfg = Arc::new(cfg);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let cfg = cfg.clone();
            std::thread::spawn(move || {
                let mut s = s;
                if let Ok(mut c) = rustls::ServerConnection::new(cfg) {
                    let mut t = rustls::Stream::new(&mut c, &mut s);
                    let mut b = [0u8; 16];
                    let _ = t.read(&mut b);
                }
            });
        }
    });
    (port, der)
}

#[test]
fn the_real_dialer_trusts_the_public_roots_only_and_a_gateway_with_a_self_made_certificate_is_refused() {
    let (port, der) = self_signed_server();
    // the public roots do not know this certificate: refused, in words, nothing sent
    let e = match real_dialer()("localhost", port) {
        Err(e) => e,
        Ok(_) => panic!("a certificate no public authority signed must be refused"),
    };
    assert!(!e.is_empty(), "{e}");
    // the same dialer with the certificate in the roots reaches h2: the dialer itself works
    let mut roots = rustls::RootCertStore::empty();
    roots.add(rustls::pki_types::CertificateDer::from(der)).unwrap();
    assert!(matches!(real_dialer_with(roots)("localhost", port), Ok(Dialed::H2(_))));
    assert!(iyakki_proxy::certview::public_roots().roots.len() > 100, "the public roots are the browser's list");
}

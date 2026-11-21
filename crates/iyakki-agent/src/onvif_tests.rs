//! The ONVIF client against a fake camera (F-SDK-167): a recording transport for the SOAP, and one loopback socket for the HTTP.

use super::*;
use std::sync::{Arc, Mutex};

struct Fixed;
fn unb64(s: &str) -> Vec<u8> {
    let t = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut n = 0;
    let mut out = Vec::new();
    for c in s.trim_end_matches('=').chars() {
        bits = bits << 6 | t.find(c).unwrap() as u32;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
            bits &= (1 << n) - 1;
        }
    }
    out
}
impl Source for Fixed {
    fn created(&self) -> String {
        "2010-09-16T07:50:45Z".into()
    }
    fn nonce(&self) -> [u8; 16] {
        unb64("LKqI6G/AikKCQrN0zqZFlg==").try_into().unwrap()
    }
}

/// A camera: answers by the action in the URL it is asked at and remembers every request.
#[derive(Clone, Default)]
struct FakeCam {
    log: Arc<Mutex<Vec<(String, String, String)>>>,
    has_ptz: bool,
    refuse: bool,
}

const CAPS: &str = "<s:Envelope xmlns:s=\"x\"><s:Body><tds:GetCapabilitiesResponse><tds:Capabilities><tt:Media><tt:XAddr>http://cam/onvif/media</tt:XAddr></tt:Media><tt:PTZ><tt:XAddr>http://cam/onvif/ptz</tt:XAddr></tt:PTZ></tds:Capabilities></tds:GetCapabilitiesResponse></s:Body></s:Envelope>";
const PROFILES: &str = "<s:Envelope xmlns:s=\"x\"><s:Body><trt:GetProfilesResponse><trt:Profiles token=\"fixed1\" fixed=\"true\"><tt:Name>main</tt:Name></trt:Profiles><trt:Profiles token=\"mover\" fixed=\"false\"><tt:Name>ptz</tt:Name><tt:PTZConfiguration token=\"p\"/></trt:Profiles></trt:GetProfilesResponse></s:Body></s:Envelope>";

impl Transport for FakeCam {
    fn post(&self, url: &str, action: &str, body: &str) -> Result<String, String> {
        self.log.lock().unwrap().push((url.into(), action.into(), body.into()));
        if self.refuse {
            return Ok("<s:Envelope><s:Body><s:Fault><s:Reason><s:Text xml:lang=\"en\">Sender not authorized</s:Text></s:Reason></s:Fault></s:Body></s:Envelope>".into());
        }
        Ok(if action.ends_with("GetCapabilities") {
            if self.has_ptz { CAPS.to_string() } else { CAPS.replace("<tt:PTZ><tt:XAddr>http://cam/onvif/ptz</tt:XAddr></tt:PTZ>", "") }
        } else if action.ends_with("GetProfiles") {
            if self.has_ptz { PROFILES.to_string() } else { PROFILES.replace("<tt:PTZConfiguration token=\"p\"/>", "") }
        } else if action.ends_with("GetPresets") {
            "<s:Envelope><s:Body><tptz:GetPresetsResponse><tptz:Preset token=\"1\"><tt:Name>Front door</tt:Name></tptz:Preset><tptz:Preset token=\"7\"/></tptz:GetPresetsResponse></s:Body></s:Envelope>".to_string()
        } else {
            "<s:Envelope><s:Body/></s:Envelope>".to_string()
        })
    }
}

fn client(cam: &FakeCam) -> Onvif {
    Onvif::new(Box::new(cam.clone()), Box::new(Fixed), "admin", "userpassword")
}

#[test]
fn the_password_digest_matches_the_one_in_the_onvif_specification() {
    let h = client(&FakeCam::default()).security_header();
    assert!(h.contains("tuOSpGlFlIXsozq4HFNeeGeFLEI="), "{h}");
    assert!(h.contains("LKqI6G/AikKCQrN0zqZFlg=="));
    assert!(!h.contains("userpassword"), "the password never leaves");
}

#[test]
fn a_camera_with_ptz_is_found_and_moved_and_stopped() {
    let cam = FakeCam { has_ptz: true, ..Default::default() };
    let p = PtzCamera::probe(client(&cam), "http://cam/onvif/device_service").unwrap().expect("a camera that moves");
    p.command(PtzCommand::Move { pan: 0.5, tilt: -2.0, zoom: 0.0 }).unwrap();
    p.command(PtzCommand::Stop).unwrap();
    let log = cam.log.lock().unwrap();
    let urls: Vec<&str> = log.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(urls, ["http://cam/onvif/device_service", "http://cam/onvif/media", "http://cam/onvif/ptz", "http://cam/onvif/ptz"]);
    assert!(log[2].2.contains("<ProfileToken>mover</ProfileToken>"), "the profile with the PTZ configuration, not the first one");
    assert!(log[2].2.contains("x=\"0.500\" y=\"-1.000\""), "speeds are clamped to the range");
    assert!(log[3].1.ends_with("/Stop") && log[3].2.contains("<Stop "));
}

#[test]
fn a_place_is_sent_as_one_absolute_move_inside_the_range_and_a_bad_place_never_leaves() {
    let cam = FakeCam { has_ptz: true, ..Default::default() };
    let p = PtzCamera::probe(client(&cam), "http://cam/d").unwrap().unwrap();
    p.command(PtzCommand::Absolute { pan: -3.0, tilt: 0.25, zoom: 2.0 }).unwrap();
    let last = cam.log.lock().unwrap().last().cloned().unwrap();
    assert!(last.1.ends_with("/AbsoluteMove") && last.2.contains("<AbsoluteMove ") && last.2.contains("<Position>"), "{last:?}");
    assert!(last.2.contains("x=\"-1.000\" y=\"0.250\"") && last.2.contains("<Zoom x=\"1.000\""), "clamped: pan to -1, zoom to 1: {}", last.2);
    let n = cam.log.lock().unwrap().len();
    assert!(p.command(PtzCommand::Absolute { pan: f32::NAN, tilt: 0.0, zoom: 0.0 }).is_err());
    assert_eq!(cam.log.lock().unwrap().len(), n);
}

#[test]
fn a_camera_without_ptz_is_a_plain_none_and_a_refusal_is_a_sentence() {
    let cam = FakeCam::default();
    assert!(PtzCamera::probe(client(&cam), "http://cam/onvif/device_service").unwrap().is_none());
    let refuse = FakeCam { refuse: true, ..Default::default() };
    assert_eq!(PtzCamera::probe(client(&refuse), "http://cam/onvif/device_service").err().unwrap(), "Sender not authorized");
}

#[test]
fn a_bad_command_never_reaches_the_camera() {
    let cam = FakeCam { has_ptz: true, ..Default::default() };
    let p = PtzCamera::probe(client(&cam), "http://cam/d").unwrap().unwrap();
    let n = cam.log.lock().unwrap().len();
    assert!(p.command(PtzCommand::Move { pan: f32::NAN, tilt: 0.0, zoom: 0.0 }).is_err());
    assert_eq!(cam.log.lock().unwrap().len(), n);
}

#[test]
fn xml_helpers_and_addresses() {
    assert_eq!(tag_text("<a:b x=\"1\">hi </a:b>", "b").as_deref(), Some("hi"));
    assert_eq!(tag_attr("<p:c token=\"t9\" k=\"v\"/>", "c", "token").as_deref(), Some("t9"));
    assert_eq!(device_url("rtsp://cam.lan:554/stream1", None).as_deref(), Some("http://cam.lan/onvif/device_service"));
    assert_eq!(device_url("rtsp://user@10.0.0.9/s", Some(8080)).as_deref(), Some("http://10.0.0.9:8080/onvif/device_service"));
    assert_eq!(device_url("nonsense", None), None);
    assert_eq!(split_http("http://h/p").unwrap(), ("h:80".to_string(), "/p".to_string()));
    assert!(split_http("https://h/p").is_err());
    assert!(split_http("http://u:p@h/p").is_err());
    assert!(SystemSource.created().ends_with('Z') && SystemSource.created().len() == 20);
}

#[test]
fn the_http_transport_speaks_to_a_socket_that_stays_open() {
    use std::io::{Read, Write};
    // the listener is bound before the client is told its port and lives until the answer is read: nothing to race
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut got = vec![0u8; 4096];
        let n = s.read(&mut got).unwrap();
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\npong").unwrap();
        String::from_utf8_lossy(&got[..n]).to_string()
    });
    let out = HttpTransport::default().post(&format!("http://127.0.0.1:{port}/onvif/ptz"), "act", "<ping/>").unwrap();
    assert_eq!(out, "pong");
    let req = server.join().unwrap();
    assert!(req.starts_with("POST /onvif/ptz HTTP/1.1") && req.contains("action=\"act\"") && req.ends_with("<ping/>"));
}

#[test]
fn a_401_is_a_sentence_without_the_address() {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut b = [0u8; 2048];
        let _ = s.read(&mut b);
        s.write_all(b"HTTP/1.1 401 Unauthorized\r\n\r\n").unwrap();
    });
    let e = HttpTransport::default().post(&format!("http://127.0.0.1:{port}/x"), "a", "b").unwrap_err();
    server.join().unwrap();
    assert!(e.contains("user name or password") && !e.contains("127.0.0.1"));
}

#[test]
fn saved_positions_are_listed_and_one_is_reached_by_name_and_a_wrong_name_sends_nothing() {
    let cam = FakeCam { has_ptz: true, ..Default::default() };
    let p = PtzCamera::probe(client(&cam), "http://cam/d").unwrap().unwrap();
    assert_eq!(p.presets().unwrap(), [("1".to_string(), "Front door".to_string()), ("7".to_string(), "position 7".to_string())]);
    assert_eq!(p.go_to("front DOOR").unwrap(), "Front door");
    let sent = cam.log.lock().unwrap().last().cloned().unwrap();
    assert!(sent.1.ends_with("/GotoPreset") && sent.2.contains("<PresetToken>1</PresetToken>") && sent.2.contains("<ProfileToken>mover</ProfileToken>"));
    let n = cam.log.lock().unwrap().len();
    assert!(p.go_to("kitchen").unwrap_err().contains("no saved position"));
    assert_eq!(cam.log.lock().unwrap().len(), n + 1, "only the list was asked");
    assert!(p.client.goto_preset("http://cam/ptz", "mover", "1</PresetToken><x/>").is_err());
}

#[test]
fn the_ports_to_try_start_with_the_one_the_stream_address_names() {
    assert_eq!(candidate_ports("rtsp://cam:554/s"), [80, 8000, 8080, 2020]);
    assert_eq!(candidate_ports("http://u:p@cam:8080/s"), [8080, 80, 8000, 2020]);
    assert_eq!(candidate_ports("http://[fe80::1]/s"), [80, 8000, 8080, 2020]);
    assert_eq!(candidate_ports("http://cam:9000/x")[0], 9000);
}

#[test]
fn an_https_address_is_split_and_the_endpoints_to_try_put_the_encrypted_ones_where_the_address_says() {
    assert_eq!(split_url("https://cam/p").unwrap(), (true, "cam:443".to_string(), "/p".to_string()));
    assert_eq!(split_url("https://cam:8443/p").unwrap().1, "cam:8443");
    assert_eq!(split_url("http://cam/p").unwrap().0, false);
    assert!(split_url("ftp://cam").is_err() && split_url("https://u:p@cam/").is_err());
    assert_eq!(device_url_for("rtsp://cam.lan/s", true, 443).as_deref(), Some("https://cam.lan/onvif/device_service"));
    assert_eq!(device_url_for("rtsp://cam.lan/s", true, 8443).as_deref(), Some("https://cam.lan:8443/onvif/device_service"));
    assert_eq!(candidate_endpoints("rtsp://cam:554/s"), [(false, 80), (false, 8000), (false, 8080), (false, 2020), (true, 443), (true, 8443)]);
    assert_eq!(candidate_endpoints("https://cam:9443/s")[0], (true, 9443));
    assert_eq!(candidate_endpoints("https://cam/s")[0], (true, 443));
}

/// A TLS server with a fresh self-signed certificate that answers every request with a SOAP reply. Its listener is bound before the port is known.
fn tls_camera() -> u16 {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let key = PrivateKeyDer::try_from(ck.key_pair.serialize_der()).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(ck.cert.der().to_vec())], key).unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let cfg = Arc::new(cfg);
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let cfg = cfg.clone();
            std::thread::spawn(move || {
                let mut s = s;
                let Ok(mut c) = rustls::ServerConnection::new(cfg) else { return };
                let mut t = rustls::Stream::new(&mut c, &mut s);
                let mut got = vec![0u8; 8192];
                let _ = t.read(&mut got);
                let _ = t.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\npong");
                let _ = t.flush();
            });
        }
    });
    port
}

#[test]
fn an_https_control_address_waits_for_the_persons_yes_on_the_certificate_then_speaks_through_the_pin() {
    let port = tls_camera();
    let d = crate::testdir::outside_project("onvif", "tls");
    let url = format!("https://localhost:{port}/onvif/device_service");
    // without the pin store an encrypted address is refused outright
    assert!(HttpTransport::default().post(&url, "a", "<x/>").unwrap_err().contains("pinned"));
    // first contact: the certificate is put in front of the person, with the camera's own approval card, and nothing is sent
    let t = HttpTransport::with_tls(d.clone(), "cam-onvif");
    let e = t.post(&url, "a", "<x/>").unwrap_err();
    assert!(e.contains("camera_trust_set") && e.contains("fingerprint"), "{e}");
    // the person approves it; the same call now works and the answer is read to its end
    crate::camtls::TrustStore::change(&d, |st| st.approve("cam-onvif").map(|_| ())).unwrap();
    assert_eq!(t.post(&url, "a", "<x/>").unwrap(), "pong");
}

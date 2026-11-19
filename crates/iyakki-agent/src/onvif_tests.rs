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

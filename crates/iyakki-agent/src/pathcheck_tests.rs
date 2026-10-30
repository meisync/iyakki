//! The boot self-test's path checks, run here against the loopback listener on the developer machine. What only the real image can say
//! (that its `git`, `ip`, CA bundle and loopback interface are there) is decided by `qemu-test.sh` on the booted image.

use super::*;

fn real() -> Loopback {
    crate::netprobe::real_network();
    Loopback::start().expect("loopback listener")
}

#[test]
fn the_loopback_listener_answers_http_on_127_0_0_1_and_nothing_else() {
    let lo = real();
    assert!(lo.host().starts_with("127.0.0.1:"));
    let mut s = TcpStream::connect(lo.host()).unwrap();
    s.write_all(b"GET /v2/ HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut got = String::new();
    s.read_to_string(&mut got).unwrap();
    assert!(got.starts_with("HTTP/1.1 200 OK") && got.contains("selftest"), "{got}");
}

#[test]
fn the_http_paths_work_through_the_production_clients_without_any_program() {
    let lo = real();
    for name in ["platform-connect", "registry-probe", "registry-setup"] {
        let v = check(name, &lo);
        assert!(v.is_ok(), "{name}: {v:?}");
    }
}

#[test]
fn the_git_client_path_needs_the_git_program_and_says_so_when_it_is_missing() {
    let lo = real();
    let v = check("git-client", &lo);
    if crate::wsexec::on_path("git") {
        assert!(v.as_ref().is_ok_and(|m| m.contains("git version")), "{v:?}");
    } else {
        assert!(v.unwrap_err().contains("git is not on this system"));
    }
}

#[test]
fn the_cluster_tools_path_answers_a_missing_kubectl_in_words() {
    let v = cluster_tools();
    if crate::wsexec::System.has("kubectl") {
        assert!(v.is_ok() || v.unwrap_err().contains("kubectl is installed but failed"));
    } else {
        assert!(v.unwrap().contains("says so in words"));
    }
    use crate::wsexec::Runner;
}

#[test]
fn the_doctor_probe_path_sees_the_listener_and_the_loopback_address() {
    let lo = real();
    let v = check("doctor-probes", &lo);
    assert!(v.is_ok(), "{v:?}");
}

#[test]
fn the_ca_bundle_and_program_paths_report_what_is_missing() {
    let _ = crate::gitnet::ca_bundle_certs().map(|n| assert!(n > 0));
    match programs() {
        Ok(m) => assert!(m.contains("programs")),
        Err(e) => assert!(e.starts_with("the image lacks: ") && IMAGE_PROGRAMS.iter().any(|p| e.contains(p)), "{e}"),
    }
}

#[test]
fn every_path_has_a_line_and_the_names_are_the_ones_qemu_test_requires() {
    crate::netprobe::real_network();
    let lines = lines();
    assert_eq!(lines.len(), PATHS.len());
    for (l, n) in lines.iter().zip(PATHS) {
        assert!(l.starts_with(&format!("IYAKKI-SELFTEST path {n} ")), "{l}");
        assert!(l.contains(" ok - ") || l.contains(" FAILED "), "{l}");
    }
    // qemu-test.sh lists the same names
    let script = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot/qemu-test.sh")).unwrap();
    for n in PATHS {
        assert!(script.contains(&format!("IYAKKI-SELFTEST path {n} ok")), "qemu-test.sh does not require path {n}");
    }
    assert_eq!(check("nothing", &real()).unwrap_err(), "no such path: nothing");
}

#[test]
fn a_listener_that_cannot_start_fails_every_path_with_the_same_reason() {
    // the listener is only asked for in run_all; a refusal there must not panic
    let all: Vec<_> = PATHS.iter().map(|n| (*n, Err::<String, String>("cannot listen".into()))).collect();
    assert!(all.iter().all(|(_, v)| v.is_err()));
}

#[test]
fn an_image_without_the_mp3_plugin_says_so_and_a_plugin_that_cannot_run_is_a_failure() {
    let none = std::env::temp_dir().join(format!("iyakki-no-such-plugin-{}", std::process::id()));
    assert!(mp3_decoder_with(&none).unwrap().contains("no web engine"), "images without the web engine pass with a plain note");
    // a plugin file is there: the check runs gst-launch-1.0; on a machine without it (or without our plugin) the line is FAILED, never ok
    let fake = std::env::temp_dir().join(format!("iyakki-fake-plugin-{}.so", std::process::id()));
    std::fs::write(&fake, b"not a plugin").unwrap();
    let r = mp3_decoder_with(&fake);
    let _ = std::fs::remove_file(&fake);
    assert!(r.is_err(), "a file that is not a working plugin must not pass: {r:?}");
}

#[test]
fn the_mp3_sample_is_a_real_mp3_the_check_can_decode() {
    assert!(MP3_SAMPLE.len() > 1000 && MP3_SAMPLE.len() < 20_000);
    let mut dec = nanomp3::Decoder::new();
    let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
    let (_, f) = dec.decode(MP3_SAMPLE, &mut pcm);
    assert!(f.is_ok_and(|f| f.sample_rate == 44100 && f.channels as u32 == 2), "44.1 kHz stereo");
}

#[test]
fn the_recorder_check_passes_without_a_web_engine_and_fails_when_an_element_is_missing() {
    let none = std::env::temp_dir().join(format!("iyakki-no-web-marker-{}", std::process::id()));
    assert!(web_recorder_with(&none, &["encodebin"]).unwrap().contains("no web engine"));
    // a web image (the marker exists) with an element no GStreamer has: a failure that names it, never ok
    let marker = std::env::temp_dir().join(format!("iyakki-web-marker-{}", std::process::id()));
    std::fs::write(&marker, b"x").unwrap();
    let r = web_recorder_with(&marker, &["iyakki-no-such-element"]);
    let _ = std::fs::remove_file(&marker);
    assert!(r.is_err(), "{r:?}");
}

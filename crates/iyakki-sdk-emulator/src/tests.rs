use crate::emulator::{Emulator, Input, Mode};
use crate::golden::{compare, Outcome, Tolerance};
use crate::project::{Config, Project};
use crate::services::{iso, FakeServices, CLOCK_START_MS};
use crate::{cli, png, script, sha256::sha256_hex, watch::Watcher};
use iyakki_tile_abi::{Color, DisplayList, Op, Rect};
use iyakki_tile_host::ServiceBackend;
use std::path::{Path, PathBuf};

fn tmp(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("iyakki-sdk-emu-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_tile(dir: &Path, wat: &str, caps: &str) {
    std::fs::write(dir.join("tile.wat"), wat).unwrap();
    std::fs::write(dir.join("iyakki-app.json"), format!(r#"{{"kind":"tile","name":"t","version":"1.2.3","tile":{{"abi":1,"name":"demo","caps":[{caps}]}}}}"#)).unwrap();
}

fn example(tag: &str) -> (PathBuf, Project) {
    let d = tmp(tag);
    write_tile(&d, iyakki_tile_example::TILE_WAT, "");
    let p = Project::load(&d).unwrap();
    (d, p)
}

/// A tile that makes one service call on every event and draws one text.
fn caller_wat(domain: &str, method: &str, json: &str) -> String {
    let mut l = DisplayList::new();
    l.push(Op::Text { x: 4, y: 4, face: 0, size: 14, color: Color(255, 255, 255, 255), text: "READY".into() });
    l.push(Op::Hit { id: 1, r: Rect::new(0, 0, 100, 100) });
    let body = l.encode().unwrap();
    let hex = |b: &[u8]| b.iter().map(|x| format!("\\{x:02x}")).collect::<String>();
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend(body);
    format!(
        r#"(module
  (import "iyakki_tile" "call" (func $call (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "{}")
  (data (i32.const 900) "{domain}") (data (i32.const 920) "{method}") (data (i32.const 940) "{}")
  (func (export "iyakki_tile_abi") (result i32) (i32.const 1))
  (func (export "iyakki_alloc") (param i32) (result i32) (i32.const 8192))
  (func (export "iyakki_tile_init") (param i32 i32 i32) (result i32) (i32.const 0))
  (func (export "iyakki_tile_event") (param i32 i32) (result i32)
    (drop (call $call (i32.const 900) (i32.const {}) (i32.const 920) (i32.const {}) (i32.const 940) (i32.const {}) (i32.const 3000) (i32.const 512)))
    (i32.const 1))
  (func (export "iyakki_tile_frame") (result i32) (i32.const 1024)))"#,
        hex(&frame),
        json.replace('\\', "\\5c").replace('"', "\\22"),
        domain.len(),
        method.len(),
        json.len()
    )
}

fn svc(config: Config) -> (PathBuf, FakeServices) {
    let d = tmp("svc");
    (d.clone(), FakeServices::new(d.join("scratch"), config))
}

#[test]
fn sha256_matches_the_known_answers() {
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}

#[test]
fn a_picture_survives_the_png_round_trip_and_flat_pictures_are_small() {
    let (w, h) = (64u32, 48u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (i, p) in rgba.chunks_mut(4).enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        p.copy_from_slice(&if x < 20 { [(x * 9) as u8, (y * 5) as u8, 7, 255] } else { [30, 60, 90, 255] });
    }
    let png = png::encode(w, h, &rgba);
    assert_eq!(png::decode(&png).unwrap(), (w, h, rgba.clone()));
    let flat = vec![200u8; (800 * 480 * 4) as usize];
    let small = png::encode(800, 480, &flat);
    assert!(small.len() < 20_000, "{}", small.len());
    assert_eq!(png::decode(&small).unwrap().2, flat);
    assert!(png::decode(b"not a png").is_err());
}

#[test]
fn a_golden_compare_honours_the_channel_and_percent_tolerance() {
    let (w, h) = (10u32, 10u32);
    let a = vec![100u8; 400];
    let g = png::encode(w, h, &a);
    assert_eq!(compare(w, h, &a, &g, Tolerance::default()), Outcome::Same);
    let mut b = a.clone();
    b[0] += 3;
    assert!(matches!(compare(w, h, &b, &g, Tolerance::default()), Outcome::Different { pixels: 1, .. }));
    assert!(matches!(compare(w, h, &b, &g, Tolerance { channel: 2, percent: 0.0 }), Outcome::Different { .. }));
    assert!(matches!(compare(w, h, &b, &g, Tolerance { channel: 3, percent: 0.0 }), Outcome::Within { pixels: 0, max: 3 }));
    assert!(matches!(compare(w, h, &b, &g, Tolerance { channel: 0, percent: 1.0 }), Outcome::Within { pixels: 1, .. }));
    assert!(matches!(compare(5, 5, &a[..100], &g, Tolerance::default()), Outcome::SizeMismatch { .. }));
    assert!(matches!(compare(w, h, &a, b"junk", Tolerance::default()), Outcome::Unreadable(_)));
}

#[test]
fn the_fake_clock_starts_at_a_fixed_time_and_moves_with_ticks() {
    assert_eq!(iso(CLOCK_START_MS), "2026-01-01T00:00:00Z");
    assert_eq!(iso(CLOCK_START_MS + 86_400_000 * 59 + 3_661_000), "2026-03-01T01:01:01Z");
    let (d, s) = svc(Config::default());
    assert!(s.call("clock", "now", "{}").unwrap().contains("2026-01-01T00:00:00Z"));
    s.advance(5000);
    assert!(s.call("clock", "now", "").unwrap().contains("2026-01-01T00:00:05Z"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn files_stay_in_the_scratch_folder() {
    let (d, s) = svc(Config::default());
    assert!(s.call("files", "write", r#"{"path":"a/b.txt","text":"hi"}"#).is_err(), "no parent folder yet");
    s.call("files", "write", r#"{"path":"b.txt","text":"hi"}"#).unwrap();
    assert!(s.call("files", "read", r#"{"path":"b.txt"}"#).unwrap().contains("hi"));
    assert!(s.call("files", "list", "{}").unwrap().contains("b.txt"));
    for bad in ["../x", "/etc/passwd", "a/../../x", "", "a\\b"] {
        let r = s.call("files", "read", &format!(r#"{{"path":{}}}"#, serde_json::json!(bad)));
        assert!(r.is_err(), "{bad:?} must be refused");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/", d.join("scratch/out")).unwrap();
        assert!(s.call("files", "read", r#"{"path":"out/etc/hostname"}"#).is_err(), "a link out of the scratch folder is refused");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn secrets_are_redacted_in_the_log_and_the_person_is_asked() {
    let cfg = Config::parse(r#"{"secrets":{"api":"hunter2"},"net":{"https://x.test/a":{"status":200,"body":"{\"ok\":1}"}},"deny":["camera"]}"#).unwrap();
    let (d, s) = svc(cfg);
    assert!(s.call("vault", "get", r#"{"name":"api"}"#).unwrap().contains("hunter2"), "the tile gets the value");
    s.call("net", "fetch", r#"{"url":"https://x.test/a","headers":{"Authorization":"Bearer abc123"}}"#).unwrap();
    assert!(s.call("net", "fetch", r#"{"url":"https://x.test/missing"}"#).unwrap_err().contains("no canned answer"));
    assert!(s.call("camera", "capture", "{}").unwrap_err().contains("said no"));
    assert!(s.call("audio", "play", "{}").unwrap().contains("emulated"));
    assert!(s.call("nothing", "x", "{}").unwrap_err().contains("not emulated"));
    let log = format!("{:?}", s.calls());
    assert!(!log.contains("hunter2") && !log.contains("abc123"), "{log}");
    assert!(log.contains("[redacted]"));
    let p = s.prompts();
    assert!(p.iter().any(|x| x.domain == "vault" && x.granted && x.words.contains("vault")));
    assert!(p.iter().any(|x| x.domain == "camera" && !x.granted));
    assert_eq!(p.iter().filter(|x| x.domain == "net").count(), 1, "asked once per call kind");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn storage_is_kept_and_capped() {
    let (d, s) = svc(Config::default());
    s.call("storage", "put", r#"{"key":"k","value":{"n":1}}"#).unwrap();
    assert!(s.call("storage", "get", r#"{"key":"k"}"#).unwrap().contains("\"n\":1"));
    let big = "x".repeat(300 * 1024);
    assert!(s.call("storage", "put", &format!(r#"{{"key":"big","value":"{big}"}}"#)).unwrap_err().contains("256 KB"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_project_loads_and_the_review_text_has_the_cards_layout() {
    let d = tmp("review");
    write_tile(&d, iyakki_tile_example::TILE_WAT, r#""storage.get","vault.get","weird""#);
    let p = Project::load(&d).unwrap();
    let r = p.review_text();
    assert!(r.starts_with("tile demo 1.2.3 (new)\npermissions: "), "{r}");
    assert!(r.contains("keep its own saved data") && r.contains("use a vault entry") && r.contains("[only get]"), "{r}");
    assert!(r.contains("warning: vault.get asks you again each time"), "{r}");
    assert!(r.contains("warning: weird is not a service"), "{r}");
    let (_d2, p2) = example("review2");
    assert!(p2.review_text().contains("permissions: none"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_bad_manifest_is_refused_with_a_reason() {
    let d = tmp("badmanifest");
    write_tile(&d, iyakki_tile_example::TILE_WAT, r#""Bad Cap""#);
    assert!(Project::load(&d).unwrap_err().contains("not a capability"));
    std::fs::write(d.join("iyakki-app.json"), r#"{"kind":"tile"}"#).unwrap();
    assert!(Project::load(&d).unwrap_err().contains("no \"tile\" section"));
    std::fs::remove_file(d.join("iyakki-app.json")).unwrap();
    assert!(Project::load(&d).unwrap_err().contains("no iyakki-app.json"));
    std::fs::write(d.join("x.iyp"), "IYAKKI-PKG\nname: a\n---\nbody").unwrap();
    assert!(Project::load(&d.join("x.iyp")).unwrap_err().contains("unpacked tile folder"));
    assert!(Project::load(&d.join("nothing")).is_err());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_tile_built_for_another_abi_is_refused_with_the_devices_sentence() {
    let (d, p) = example("abi");
    std::fs::write(d.join("iyakki-app.json"), r#"{"tile":{"abi":2,"name":"demo"}}"#).unwrap();
    let p2 = Project::load(&d).unwrap();
    let e = Emulator::new(p2, d.join("s"), (800, 480), Mode::Touch, false).err().unwrap();
    assert!(e.contains("version 2") && e.contains("Update"), "{e}");
    drop(p);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn taps_work_in_touch_and_mouse_mode_and_the_picture_changes() {
    for mode in [Mode::Touch, Mode::Mouse] {
        let (d, p) = example("tap");
        let mut e = Emulator::new(p, d.join("s"), (800, 480), mode, true).unwrap();
        let before = e.image().rgba;
        assert_eq!(e.texts()[2], "0");
        e.apply(Input::Tap(30, 80)).unwrap();
        e.apply(Input::Tap(30, 80)).unwrap();
        assert_eq!(e.texts()[2], "2", "{mode:?}");
        assert_ne!(e.image().rgba, before);
        e.apply(Input::Text("hi".into())).unwrap();
        assert!(e.texts().iter().any(|t| t == "hi"));
        e.apply(Input::Tick(16)).unwrap();
        e.apply(Input::Resize(1024, 600)).unwrap();
        e.apply(Input::Theme(false)).unwrap();
        assert_eq!((e.image().w, e.image().h), (1024, 600));
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_tile_that_uses_a_capability_it_did_not_declare_is_refused_and_stopped() {
    let d = tmp("refuse");
    write_tile(&d, &caller_wat("vault", "get", r#"{"name":"api"}"#), "");
    let p = Project::load(&d).unwrap();
    let mut e = Emulator::new(p, d.join("s"), (800, 480), Mode::Touch, false).unwrap();
    let err = e.apply(Input::Tap(10, 10)).unwrap_err();
    assert!(err.starts_with("REFUSED") && err.contains("vault.get") && err.contains("tile.caps"), "{err}");
    assert!(e.refused().is_some());
    assert!(e.apply(Input::Tick(1)).is_err(), "nothing more is delivered");
    let c = e.services.calls();
    assert!(c.len() == 1 && c[0].refused && !c[0].ok);
    assert!(e.services.prompts().is_empty(), "a refused call never asks the person");
    // declared: it runs, the person is asked, the secret is not in the log
    write_tile(&d, &caller_wat("vault", "get", r#"{"name":"api"}"#), r#""vault.get""#);
    let mut e = Emulator::new(Project::load(&d).unwrap(), d.join("s"), (800, 480), Mode::Touch, false).unwrap();
    e.apply(Input::Tap(10, 10)).unwrap();
    assert_eq!(e.services.prompts().len(), 1);
    assert!(!format!("{:?}", e.services.calls()).contains("emulated-secret"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn reload_keeps_the_tile_where_it_was() {
    let (d, p) = example("reload");
    let mut e = Emulator::new(p, d.join("s"), (800, 480), Mode::Touch, false).unwrap();
    for _ in 0..3 {
        e.apply(Input::Tap(30, 80)).unwrap();
    }
    let mut w = Watcher::new(e.project.watched());
    assert!(!w.changed());
    let wat = std::fs::read_to_string(d.join("tile.wat")).unwrap();
    std::fs::write(d.join("tile.wat"), format!("{wat}\n;; edited")).unwrap();
    assert!(w.changed() && !w.changed());
    let msg = e.reload().unwrap();
    assert!(msg.contains("3 earlier input"), "{msg}");
    assert_eq!(e.texts()[2], "3");
    std::fs::write(d.join("tile.wat"), "(module").unwrap();
    assert!(e.reload().is_err(), "a broken edit is reported, the running tile stays");
    assert_eq!(e.texts()[2], "3");
    let _ = std::fs::remove_dir_all(d);
}

fn run_script(p: &Project, d: &Path, name: &str, text: &str, update: bool) -> script::ScriptReport {
    script::run(p, name, text, &d.join("golden"), update, &d.join("scratch"))
}

#[test]
fn scripts_compare_shots_with_golden_pictures() {
    let (d, p) = example("script");
    let good = r#"{"size":[800,480],"steps":[{"shot":"start"},{"expect_text":"TILE DEMO"},{"tap":[30,80]},{"expect_text":"1"},{"shot":"one"}]}"#;
    let missing = run_script(&p, &d, "count", good, false);
    assert!(!missing.passed() && missing.failures[0].contains("no golden picture"), "{:?}", missing.failures);
    let w = run_script(&p, &d, "count", good, true);
    assert!(w.passed() && w.shots.len() == 2, "{:?}", w.failures);
    let again = run_script(&p, &d, "count", good, false);
    assert!(again.passed(), "{:?}", again.failures);
    assert!(again.shots[0].contains("sha256") && again.shots[0].contains("same"));
    // a different run gives a different picture: the second shot is taken before the tap
    let other = r#"{"steps":[{"shot":"start"},{"shot":"one"}]}"#;
    let r = run_script(&p, &d, "count", other, false);
    assert!(!r.passed() && r.failures[0].contains("\"one\" differs"), "{:?}", r.failures);
    assert!(d.join("golden/_actual/count--one.png").is_file(), "the actual picture is kept");
    // a tolerance wide enough accepts it
    let loose = r#"{"tolerance":{"channel":255,"percent":0},"steps":[{"shot":"start"},{"shot":"one"}]}"#;
    assert!(run_script(&p, &d, "count", loose, false).passed());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn script_mistakes_fail_before_anything_runs() {
    let (d, p) = example("scriptbad");
    for (text, want) in [
        ("{", "not valid JSON"),
        (r#"{"steps":[{"jump":1}]}"#, "unknown step"),
        (r#"{"steps":[{"tap":[1,2],"text":"a"}]}"#, "one action per step"),
        (r#"{"size":[10,10],"steps":[]}"#, "size"),
        (r#"{"mode":"pen","steps":[]}"#, "mode"),
        (r#"{"steps":[{"shot":"../evil"}]}"#, "letters, digits"),
        (r#"{"steps":[{"tap":[1]}]}"#, "tap takes"),
        (r#"{"steps":[{"expect_text":"NOPE"}]}"#, "expected the picture to show"),
    ] {
        let r = run_script(&p, &d, "bad", text, false);
        assert!(r.failures.iter().any(|f| f.contains(want)), "{text}: {:?}", r.failures);
    }
    assert!(!run_script(&p, &d, "../bad", r#"{"steps":[]}"#, false).passed());
    assert!(!d.join("golden").exists(), "nothing was written");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn scripts_can_expect_calls_prompts_and_refusals() {
    let d = tmp("scriptcalls");
    write_tile(&d, &caller_wat("net", "fetch", r#"{"url":"https://x.test/a"}"#), r#""net.fetch""#);
    std::fs::write(d.join("iyakki-emulator.json"), r#"{"net":{"https://x.test/a":{"status":200,"body":"hi"}}}"#).unwrap();
    let p = Project::load(&d).unwrap();
    let r = run_script(&p, &d, "net", r#"{"steps":[{"tap":[5,5]},{"expect_call":"net.fetch"},{"expect_prompt":"net.fetch"},{"expect_text":"READY"}]}"#, false);
    assert!(r.passed(), "{:?}", r.failures);
    write_tile(&d, &caller_wat("net", "fetch", r#"{"url":"https://x.test/a"}"#), "");
    let p = Project::load(&d).unwrap();
    let r = run_script(&p, &d, "net", r#"{"steps":[{"tap":[5,5]},{"expect_refused":"net.fetch"}]}"#, false);
    assert!(r.passed(), "a script that expects the refusal passes: {:?}", r.failures);
    let r = run_script(&p, &d, "net", r#"{"steps":[{"tap":[5,5]}]}"#, false);
    assert!(!r.passed() && r.failures[0].contains("REFUSED"), "{:?}", r.failures);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_commands_print_the_review_and_exit_with_the_result() {
    let (d, _) = example("cli");
    let go = |args: &[&str]| {
        let mut out = Vec::new();
        let code = cli::run(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &mut out);
        (code, String::from_utf8(out).unwrap())
    };
    let t = d.to_str().unwrap();
    let (c, o) = go(&["check", t]);
    assert_eq!(c, 0);
    assert!(o.contains("permissions: none") && o.contains("starts at 800x480"), "{o}");
    let (c, o) = go(&["test", t]);
    assert_eq!(c, 1);
    assert!(o.contains("no scripts"), "{o}");
    std::fs::create_dir_all(d.join("tests/emulator")).unwrap();
    std::fs::write(d.join("tests/emulator/count.json"), r#"{"steps":[{"tap":[30,80]},{"shot":"one"}]}"#).unwrap();
    let (c, o) = go(&["test", t]);
    assert_eq!(c, 1, "{o}");
    let (c, o) = go(&["test", t, "--update"]);
    assert_eq!(c, 0, "{o}");
    let (c, o) = go(&["test", t]);
    assert_eq!(c, 0, "{o}");
    assert!(o.contains("ok   count") && o.contains("picture one: sha256"), "{o}");
    let png = d.join("shot.png");
    let (c, _) = go(&["shot", t, "--out", png.to_str().unwrap(), "--size", "1024x600", "--dark"]);
    assert_eq!(c, 0);
    assert_eq!(png::decode(&std::fs::read(&png).unwrap()).unwrap().0, 1024);
    assert_eq!(go(&["check", t, "--size", "5x5"]).0, 1);
    assert_eq!(go(&[]).0, 1);
    let _ = std::fs::remove_dir_all(d);
}

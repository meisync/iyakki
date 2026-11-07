//! The audio and video add-on: detection, the engine's environment with and without it (fake folders), the helper's words, and the
//! page-side media probe run in IYAKKI's own JavaScript engine against a stand-in page.

use crate::avaddon::*;
use std::path::{Path, PathBuf};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-avaddon-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A fake image plugin folder and a libraries folder holding `libs`.
fn fake(name: &str, plugin: bool, libs: &[&str]) -> (PathBuf, PathBuf, PathBuf) {
    let d = temp(name);
    let (p, l) = (d.join("plugins"), d.join("lib"));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::create_dir_all(&l).unwrap();
    if plugin {
        std::fs::write(p.join("libgstlibav.so"), b"x").unwrap();
    }
    for f in libs {
        std::fs::write(l.join(f), b"lib").unwrap();
    }
    (d, p, l)
}

#[test]
fn the_abi_file_lists_sonames_and_falls_back_to_the_default_set() {
    let t = "# written by web-post-build.sh\nlibavcodec.so.60\nlibavutil.so.58  # glue\n\nnot a library\n../../etc/passwd\n";
    assert_eq!(required_sonames(t), names(&["libavcodec.so.60", "libavutil.so.58"]));
    assert_eq!(required_sonames("").len(), DEFAULT_SONAMES.len());
    assert_eq!(required_sonames("# nothing\n"), names(&DEFAULT_SONAMES));
}

#[test]
fn detection_tells_not_in_image_absent_incomplete_and_ready() {
    let s = names(&["libavcodec.so.60", "libavutil.so.58"]);
    let (d, p, l) = fake("nobridge", false, &["libavcodec.so.60", "libavutil.so.58"]);
    assert_eq!(detect_at(&p, &l, &s), Addon::NotInImage, "no plugin in the image: the libraries alone do nothing");
    std::fs::remove_dir_all(&d).ok();
    let (d, p, l) = fake("absent", true, &[]);
    assert_eq!(detect_at(&p, &l, &s), Addon::Absent);
    assert_eq!(detect_at(&p, &d.join("no/such/dir"), &s), Addon::Absent);
    std::fs::remove_dir_all(&d).ok();
    let (d, p, l) = fake("some", true, &["libavcodec.so.60"]);
    assert_eq!(detect_at(&p, &l, &s), Addon::Incomplete(names(&["libavutil.so.58"])));
    std::fs::remove_dir_all(&d).ok();
    let (d, p, l) = fake("ready", true, &["libavcodec.so.60", "libavutil.so.58"]);
    assert_eq!(detect_at(&p, &l, &s), Addon::Ready);
    std::fs::remove_dir_all(&d).ok();
}

fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
    env.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

#[test]
fn without_the_add_on_the_engines_environment_is_untouched() {
    let (d, p, l) = fake("noenv", true, &[]);
    for a in [Addon::NotInImage, Addon::Absent, Addon::Incomplete(names(&["libavutil.so.58"]))] {
        assert!(engine_env(&a, &p, &l, &d, "box").is_empty(), "{a:?}");
    }
    let mut env = vec![("GST_REGISTRY_1_0".to_string(), "/p/cache/gst-registry.bin".to_string())];
    let before = env.clone();
    merge_env(&mut env, engine_env(&Addon::Absent, &p, &l, &d, "box"));
    assert_eq!(env, before);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn with_the_add_on_the_engine_finds_the_plugin_the_libraries_and_a_registry_of_its_own() {
    let (d, p, l) = fake("env", true, &["libavcodec.so.60"]);
    let profile = d.join("web-profile/alice");
    let env = engine_env(&Addon::Ready, &p, &l, &profile, "box");
    assert_eq!(get(&env, "GST_PLUGIN_PATH_1_0"), Some(p.to_str().unwrap()));
    assert_eq!(get(&env, "LD_LIBRARY_PATH"), Some(l.to_str().unwrap()));
    let reg = get(&env, "GST_REGISTRY_1_0").unwrap();
    assert!(Path::new(reg).starts_with(profile.join("cache")) && reg.contains("gst-registry-av-"), "{reg}");
    assert_ne!(Path::new(reg).file_name().unwrap(), "gst-registry.bin", "never the registry used without the add-on");
    assert!(get(&env, "GST_PLUGIN_FEATURE_RANK").unwrap().contains("avdec_h264:300"));
    // another machine, or another set of libraries, another registry
    let other = engine_env(&Addon::Ready, &p, &l, &profile, "other-box");
    assert_ne!(get(&other, "GST_REGISTRY_1_0"), Some(reg));
    std::fs::write(l.join("libavutil.so.58"), b"more").unwrap();
    assert_ne!(get(&engine_env(&Addon::Ready, &p, &l, &profile, "box"), "GST_REGISTRY_1_0"), Some(reg));
    // merging replaces the base registry instead of adding a second one
    let mut base = vec![("GST_REGISTRY_1_0".to_string(), "/x/gst-registry.bin".to_string()), ("HOME".to_string(), "/h".to_string())];
    merge_env(&mut base, env.clone());
    assert_eq!(base.iter().filter(|(k, _)| k == "GST_REGISTRY_1_0").count(), 1);
    assert_eq!(get(&base, "GST_REGISTRY_1_0"), Some(reg));
    assert_eq!(get(&base, "HOME"), Some("/h"));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_library_folder_can_be_named_by_the_environment_and_defaults_under_the_data_folder() {
    // (the variable is only read, never set here: tests run in parallel)
    if std::env::var_os(LIB_DIR_ENV).is_none() {
        assert_eq!(lib_dir(Path::new("/var/lib/iyakki")), Path::new("/var/lib/iyakki/addons/ffmpeg/lib"));
    }
    assert_eq!(data_of_profile(Path::new("/var/lib/iyakki/web-profile/alice")), Some(Path::new("/var/lib/iyakki")));
}

/// An unpacked add-on archive: `lib/<soname>` for each of `libs`, and the legal files unless `legal` is false.
fn fake_archive(name: &str, libs: &[&str], legal: bool) -> PathBuf {
    let d = temp(name).join("ffmpeg-libs-6.1.5");
    std::fs::create_dir_all(d.join("lib")).unwrap();
    for l in libs {
        std::fs::write(d.join("lib").join(l), vec![7u8; 4096]).unwrap();
    }
    if legal {
        for f in LEGAL_FILES {
            std::fs::write(d.join(f), "text").unwrap();
        }
    }
    d
}

#[test]
fn the_default_set_is_the_six_lgpl_libraries_and_has_no_postproc() {
    // libpostproc exists only in a GPL build of FFmpeg, and gst-libav does not link it
    assert_eq!(DEFAULT_SONAMES.len(), 6);
    assert!(DEFAULT_SONAMES.iter().all(|s| !s.contains("postproc")));
    assert!(DEFAULT_SONAMES.contains(&"libavcodec.so.60") && DEFAULT_SONAMES.contains(&"libswscale.so.7"));
    assert_eq!(addon_dir(Path::new("/var/lib/iyakki")), Path::new("/var/lib/iyakki/addons/ffmpeg"));
    assert_eq!(addon_dir(Path::new("/d")).join("lib"), lib_dir_default(Path::new("/d")));
}

fn lib_dir_default(data: &Path) -> PathBuf {
    data.join("addons").join("ffmpeg").join("lib")
}

#[test]
fn an_archive_is_checked_for_every_soname_the_licence_files_and_plain_files() {
    let s = names(&DEFAULT_SONAMES);
    let all: Vec<&str> = DEFAULT_SONAMES.to_vec();
    let ok = fake_archive("chk-ok", &all, true);
    assert_eq!(check_unpacked(&ok, &s), Ok(()));
    // another FFmpeg version: its sonames differ, and the message names what is missing
    let other = fake_archive("chk-other", &["libavcodec.so.61", "libavformat.so.61"], true);
    let e = check_unpacked(&other, &s).unwrap_err();
    assert!(e.contains("another FFmpeg version") && e.contains("libavcodec.so.60"), "{e}");
    // no licence texts or source offer: refused
    let nolic = fake_archive("chk-nolic", &all, false);
    assert!(check_unpacked(&nolic, &s).unwrap_err().contains("COPYING.LGPLv2.1"));
    // a link or an empty file is not a library
    let linked = fake_archive("chk-link", &all[1..], true);
    std::os::unix::fs::symlink("/etc/passwd", linked.join("lib/libavcodec.so.60")).unwrap();
    assert!(check_unpacked(&linked, &s).unwrap_err().contains("not a plain library"));
    let tiny = fake_archive("chk-tiny", &all[1..], true);
    std::fs::write(tiny.join("lib/libavcodec.so.60"), b"x").unwrap();
    assert!(check_unpacked(&tiny, &s).unwrap_err().contains("not a plain library"));
}

#[test]
fn an_add_on_is_put_in_place_whole_replaced_whole_and_detected_ready_by_the_engines_own_test() {
    let s = names(&DEFAULT_SONAMES);
    let all: Vec<&str> = DEFAULT_SONAMES.to_vec();
    let a = fake_archive("put", &all, true);
    let data = a.parent().unwrap().join("data");
    let dest = addon_dir(&data);
    install_unpacked(&a, &dest, &s).unwrap();
    let plugins = data.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(plugins.join("libgstlibav.so"), b"x").unwrap();
    assert_eq!(detect_at(&plugins, &dest.join("lib"), &s), Addon::Ready, "the folder the installer made passes the detection the engine makes");
    // a bad archive does not touch the installed add-on
    let bad = fake_archive("put-bad", &all[2..], true);
    assert!(install_unpacked(&bad, &dest, &s).is_err());
    assert_eq!(detect_at(&plugins, &dest.join("lib"), &s), Addon::Ready);
    // a new one replaces it as a whole and leaves no helper folders
    std::fs::write(dest.join("lib/stale-file"), b"old").unwrap();
    install_unpacked(&a, &dest, &s).unwrap();
    assert!(!dest.join("lib/stale-file").exists() && !data.join("addons/.ffmpeg-new").exists() && !data.join("addons/.ffmpeg-old").exists());
    remove_installed(&dest);
    assert_eq!(detect_at(&plugins, &dest.join("lib"), &s), Addon::Absent);
    std::fs::remove_dir_all(a.parent().unwrap()).ok();
}

#[test]
fn the_helpers_words_and_the_bar_texts() {
    assert_eq!(Need::parse("video"), Some(Need::Video));
    assert_eq!(Need::parse(" audio\n"), Some(Need::Audio));
    assert_eq!(Need::parse("both"), Some(Need::Both));
    assert_eq!(Need::parse("<script>"), None);
    assert!(bar_text(&Addon::Absent).contains("/local/packages") && bar_text(&Addon::Absent).contains("does not provide"));
    assert!(bar_text(&Addon::NotInImage).contains("does not include") && bar_text(&Addon::NotInImage).contains("/local/packages"));
    assert!(bar_text(&Addon::Ready).contains("still cannot"));
}

// ------------------------------------------------------------------------------------------------ the page-side probe

/// A stand-in page: `can_h264` / `can_aac` say what canPlayType answers; events are fired by the test through `fire`.
fn run_probe(can_h264: bool, can_aac: bool, steps: &str) -> String {
    let src = format!(
        r#"
var posts = [], handlers = {{}}, timers = [];
var window = this; window.window = window;
window.webkit = {{ messageHandlers: {{ iyakkiMedia: {{ postMessage: function (m) {{ posts.push(m); }} }} }} }};
window.setTimeout = function (f) {{ timers.push(f); }};
var setTimeout = window.setTimeout;
var document = {{
  addEventListener: function (n, f, c) {{ handlers[n] = f; }},
  createElement: function () {{ return {{ canPlayType: function (t) {{ return /avc1/.test(t) ? {h} : /mp4a/.test(t) ? {a} : 'maybe'; }} }}; }}
}};
function HTMLMediaElement() {{}}
HTMLMediaElement.prototype.play = function () {{ return this.result; }};
window.HTMLMediaElement = HTMLMediaElement;
function MediaSource() {{}}
MediaSource.prototype.addSourceBuffer = function (t) {{ if (/avc/.test(t)) throw new Error('NotSupportedError'); return {{ ok: t }}; }};
window.MediaSource = MediaSource;
var fire = function (n, target) {{ handlers[n]({{ target: target }}); }};
var runTimers = function () {{ var t = timers; timers = []; t.forEach(function (f) {{ f(); }}); }};
{probe}
{steps}
JSON.stringify(posts);
"#,
        h = if can_h264 { "'probably'" } else { "''" },
        a = if can_aac { "'probably'" } else { "''" },
        probe = PROBE_JS,
        steps = steps,
    );
    let mut e = iyakki_js::JsEngine::new(iyakki_js::Limits::default(), vec![]).expect("the JavaScript engine");
    e.eval("probe-test", &src).unwrap_or_else(|er| panic!("probe test script failed: {er:?}"))
}

#[test]
fn the_probe_reports_a_media_error_for_a_missing_codec_once() {
    let steps = "var v = { tagName: 'VIDEO', error: { code: 4 } }; fire('error', v); fire('error', v);";
    assert_eq!(run_probe(false, false, steps), r#"["both"]"#, "one word per page, even when the player errors twice");
    assert_eq!(run_probe(false, true, steps), r#"["video"]"#);
    assert_eq!(run_probe(true, false, steps), r#"["audio"]"#);
}

#[test]
fn the_probe_stays_quiet_when_the_engine_can_play_the_codec_or_the_error_is_another_one() {
    // the add-on is installed: canPlayType says yes, so a failing file is the file's trouble, not a missing codec
    assert_eq!(run_probe(true, true, "fire('error', { tagName: 'VIDEO', error: { code: 4 } });"), "[]");
    // a network error (2), an aborted load (1) and an error on a non-media element are not codec trouble
    assert_eq!(run_probe(false, false, "fire('error', { tagName: 'VIDEO', error: { code: 2 } }); fire('error', { tagName: 'VIDEO', error: { code: 1 } }); fire('error', { tagName: 'IMG' });"), "[]");
}

#[test]
fn the_probe_sees_a_source_with_nothing_to_play_a_blank_picture_a_refused_play_and_a_refused_buffer() {
    // <source> error with the media element out of sources (NETWORK_NO_SOURCE = 3)
    assert_eq!(run_probe(false, true, "fire('error', { tagName: 'SOURCE', parentNode: { tagName: 'VIDEO', networkState: 3 } });"), r#"["video"]"#);
    assert_eq!(run_probe(false, true, "fire('error', { tagName: 'SOURCE', parentNode: { tagName: 'VIDEO', networkState: 2 } });"), "[]");
    // metadata but no picture size after a moment
    assert_eq!(run_probe(false, true, "var v = { tagName: 'VIDEO', readyState: 1, videoWidth: 0, duration: 12 }; fire('loadedmetadata', v); runTimers();"), r#"["video"]"#);
    assert_eq!(run_probe(false, true, "var v = { tagName: 'VIDEO', readyState: 4, videoWidth: 540, duration: 12 }; fire('loadedmetadata', v); runTimers();"), "[]");
    // play() rejected with NotSupportedError (a thenable stands in for the promise)
    let play = "var m = new HTMLMediaElement(); m.result = { then: function (ok, bad) { bad({ name: 'NotSupportedError' }); } }; var r = m.play(); posts.push(typeof r.then);";
    assert_eq!(run_probe(false, false, play), r#"["both","function"]"#, "the page still gets its own promise back");
    // Media Source: the page's addSourceBuffer throws for avc1 and the probe lets the page see the error
    let mse = "var ms = new MediaSource(); var threw = false; try { ms.addSourceBuffer('video/mp4; codecs=\"avc1.42E01E\"'); } catch (e) { threw = true; } posts.push('threw:' + threw);";
    assert_eq!(run_probe(false, true, mse), r#"["video","threw:true"]"#);
    let ok = "var ms = new MediaSource(); posts.push(JSON.stringify(ms.addSourceBuffer('audio/webm; codecs=\"opus\"')));";
    assert_eq!(run_probe(false, true, ok), r#"["{\"ok\":\"audio/webm; codecs=\\\"opus\\\"\"}"]"#);
}

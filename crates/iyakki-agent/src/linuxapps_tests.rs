// #[cfg(test)] -- this whole file is test code (lib.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Tests of Linux apps: the strict root file system reader, the downloads (a fake transport with tiny tarballs and a SHA256SUMS file),
//! the commands built for the sandbox (golden argument arrays, no shell), package names, mounts, deletion, audit, kernel probe,
//! image-app catalog. No network, no namespaces: the programs are run by a fake runner. One ignored test runs a real sandbox.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_proxy::{StreamHead, Streamer};

use crate::imageapps;
use crate::linuxapps::*;
use crate::linuxapps_dist as dist;
use crate::linuxapps_init as init;
use crate::rootfs;
use crate::wsexec::FakeRunner;

// ------------------------------------------------------------------------------------------------ helpers

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!("iyakki-linapp-{tag}-{}", std::process::id()));
    let _ = rootfs::remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct E {
    name: String,
    kind: u8,
    content: Vec<u8>,
    link: String,
    mode: u32,
    uid: u32,
}

fn e(name: &str, kind: u8, content: &str, link: &str) -> E {
    E { name: name.into(), kind, content: content.as_bytes().to_vec(), link: link.into(), mode: 0o644, uid: 0 }
}

fn file(name: &str, content: &str) -> E {
    e(name, b'0', content, "")
}

fn dir(name: &str) -> E {
    let mut x = e(name, b'5', "", "");
    x.mode = 0o755;
    x
}

fn sym(name: &str, target: &str) -> E {
    e(name, b'2', "", target)
}

fn tar(entries: &[E]) -> Vec<u8> {
    let mut t = Vec::new();
    for x in entries {
        let mut h = [0u8; 512];
        h[..x.name.len()].copy_from_slice(x.name.as_bytes());
        h[100..108].copy_from_slice(format!("{:07o}\0", x.mode).as_bytes());
        h[108..116].copy_from_slice(format!("{:07o}\0", x.uid).as_bytes());
        h[116..124].copy_from_slice(format!("{:07o}\0", x.uid).as_bytes());
        let size = if matches!(x.kind, b'0') { x.content.len() } else { 0 };
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[156] = x.kind;
        h[157..157 + x.link.len()].copy_from_slice(x.link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        t.extend_from_slice(&h);
        if size > 0 {
            t.extend_from_slice(&x.content);
            t.extend(std::iter::repeat_n(0u8, size.div_ceil(512) * 512 - size));
        }
    }
    t.extend(std::iter::repeat_n(0u8, 1024));
    t
}

fn unpack_bytes(bytes: &[u8], dest: &Path, pol: &rootfs::Policy) -> Result<rootfs::Report, String> {
    rootfs::untar(Cursor::new(bytes.to_vec()), dest, pol, &mut |_, _| {})
}

fn sha(b: &[u8]) -> String {
    dist::sha256_hex(b)
}

type Reply = (u16, Vec<(String, String)>, Vec<u8>);

struct Fake {
    routes: Mutex<HashMap<String, Reply>>,
    asked: Mutex<Vec<(String, Vec<(String, String)>)>>,
}

impl Streamer for Fake {
    fn open(&self, url: &str, h: &[(String, String)]) -> Result<StreamHead, String> {
        self.asked.lock().unwrap().push((url.to_string(), h.to_vec()));
        let r = self.routes.lock().unwrap().get(url).cloned().ok_or_else(|| format!("no route for {url}"))?;
        let mut headers = r.1.clone();
        headers.push(("content-length".into(), r.2.len().to_string()));
        Ok(StreamHead { status: r.0, headers, body: Box::new(Cursor::new(r.2)) })
    }
}

fn fake(routes: Vec<(String, Reply)>) -> Fake {
    Fake { routes: Mutex::new(routes.into_iter().collect()), asked: Mutex::new(vec![]) }
}

fn ok(b: &[u8]) -> Reply {
    (200, vec![], b.to_vec())
}

/// A tiny Ubuntu base: a tar.gz with a root, a shell link, /etc/passwd and an apt folder.
fn mini_base() -> Vec<u8> {
    let t = tar(&[dir("./"), dir("./bin"), dir("./etc"), dir("./etc/apt"), dir("./root"), dir("./usr"), dir("./usr/sbin"), file("./bin/dash", "#!dash"), sym("./bin/sh", "dash"), sym("./lib64", "/usr/lib64"), file("./etc/passwd", "root:x:0:0:root:/root:/bin/sh\n"), file("./etc/group", "root:x:0:\n"), sym("./etc/mtab", "/proc/self/mounts")]);
    crate::archive::tests::gzip(&t)
}

fn ubuntu_routes(base: &[u8], listed: &str) -> Vec<(String, Reply)> {
    let arch = dist::arch();
    let name = format!("ubuntu-base-24.04.3-base-{arch}.tar.gz");
    vec![
        ("https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/SHA256SUMS".into(), ok(format!("{listed}  *{name}\n{}  *ubuntu-base-24.04.3-base-s390x.tar.gz\n", "1".repeat(64)).as_bytes())),
        (format!("https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/{name}"), ok(base)),
    ]
}

fn manager(data: &Path, runner: Arc<FakeRunner>) -> LinuxApps {
    let mut la = LinuxApps::new(data, "admin", Path::new("/usr/bin/iyakki"), runner);
    la.assume(false, Some(50 << 30));
    la
}

fn any_scope(_: &Path, _: bool) -> Result<(), String> {
    Ok(())
}

fn make_sandbox(data: &Path, runner: Arc<FakeRunner>, mounts: Vec<(String, String, bool)>) -> LinuxApps {
    let la = manager(data, runner);
    let base = mini_base();
    let st = fake(ubuntu_routes(&base, &sha(&base)));
    let req = CreateReq { name: "dev".into(), release: "ubuntu-24.04".into(), network: true, mounts };
    la.create(&req, &st, &any_scope, &mut |_| {}).unwrap();
    la
}

// ------------------------------------------------------------------------------------------------ the root file system reader

#[test]
fn a_normal_root_unpacks_with_absolute_links_and_hard_links() {
    let d = tmp("root-ok");
    let t = tar(&[dir("./"), dir("./bin"), file("./bin/tool", "x"), e("./bin/tool2", b'1', "", "bin/tool"), sym("./bin/sh", "/usr/bin/dash"), sym("./lib", "usr/lib"), dir("./tmp")]);
    let r = unpack_bytes(&t, &d, &rootfs::Policy::base()).unwrap();
    assert_eq!((r.files, r.symlinks, r.hardlinks), (1, 2, 1), "{r:?}");
    assert_eq!(std::fs::read_link(d.join("bin/sh")).unwrap(), Path::new("/usr/bin/dash"), "an absolute link is kept (it is only followed inside the sandbox)");
    assert_eq!(std::fs::read_to_string(d.join("bin/tool2")).unwrap(), "x");
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn names_that_leave_the_folder_refuse_the_whole_archive() {
    for bad in ["../evil", "./a/../../evil", "/etc/evil"] {
        let d = tmp("trav");
        let out = tmp("trav-out");
        let t = tar(&[file("./ok", "1"), file(bad, "pwn")]);
        let err = unpack_bytes(&t, &d, &rootfs::Policy::base()).unwrap_err();
        assert!(err.contains("outside"), "{bad}: {err}");
        assert!(!out.join("evil").exists() && !d.parent().unwrap().join("evil").exists() && !Path::new("/etc/evil").exists());
        let _ = rootfs::remove_tree(&d);
        let _ = rootfs::remove_tree(&out);
    }
}

#[test]
fn nothing_is_written_through_a_link() {
    // the classic: a link `etc -> <outside>` then a file `etc/passwd`
    let d = tmp("thru");
    let outside = tmp("thru-outside");
    let t = tar(&[sym("./etc", &outside.display().to_string()), file("./etc/passwd", "pwned")]);
    let err = unpack_bytes(&t, &d, &rootfs::Policy::base()).unwrap_err();
    assert!(err.contains("through a link"), "{err}");
    assert!(!outside.join("passwd").exists(), "the file outside was not created");
    // also for a folder entry and a deeper path, and a relative link
    let t = tar(&[sym("./a", ".."), dir("./a/b"), file("./a/b/c", "x")]);
    assert!(unpack_bytes(&t, &tmp("thru2"), &rootfs::Policy::base()).is_err());
    // a hard link entry whose own path goes through a link
    let t = tar(&[file("./real", "x"), sym("./l", &outside.display().to_string()), e("./l/h", b'1', "", "real")]);
    assert!(unpack_bytes(&t, &tmp("thru3"), &rootfs::Policy::base()).is_err());
    assert!(!outside.join("h").exists());
}

#[test]
fn hard_links_may_only_name_plain_files_of_the_archive() {
    let p = rootfs::Policy::base();
    // to a path outside
    let err = unpack_bytes(&tar(&[e("./h", b'1', "", "../../etc/passwd")]), &tmp("hl1"), &p).unwrap_err();
    assert!(err.contains("hard link"), "{err}");
    let err = unpack_bytes(&tar(&[e("./h", b'1', "", "/etc/passwd")]), &tmp("hl2"), &p).unwrap_err();
    assert!(err.contains("hard link"), "{err}");
    // to a file that is not there
    assert!(unpack_bytes(&tar(&[e("./h", b'1', "", "nothing")]), &tmp("hl3"), &p).unwrap_err().contains("not in the archive"));
    // to a symbolic link (it could point anywhere)
    let err = unpack_bytes(&tar(&[sym("./s", "/etc/passwd"), e("./h", b'1', "", "s")]), &tmp("hl4"), &p).unwrap_err();
    assert!(err.contains("not a plain file"), "{err}");
    // to a folder
    assert!(unpack_bytes(&tar(&[dir("./d"), e("./h", b'1', "", "d")]), &tmp("hl5"), &p).is_err());
}

#[test]
fn device_nodes_and_pipes_are_never_created_and_set_id_bits_go() {
    let d = tmp("dev");
    let mut suid = file("./usr/bin/su", "x");
    suid.mode = 0o4755;
    let t = tar(&[dir("./dev"), e("./dev/sda", b'4', "", ""), e("./dev/console", b'3', "", ""), e("./run/fifo", b'6', "", ""), suid]);
    let r = unpack_bytes(&t, &d, &rootfs::Policy::base()).unwrap();
    assert!(!d.join("dev/sda").exists() && !d.join("dev/console").exists() && !d.join("run/fifo").exists());
    assert_eq!(r.skipped.len(), 3, "{r:?}");
    assert!(r.skipped[0].contains("block device") && r.skipped[1].contains("character device"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(d.join("usr/bin/su")).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode & 0o6000, 0, "set-user-id and set-group-id are removed ({mode:o})");
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn size_and_count_limits_stop_a_bomb_before_it_fills_the_disk() {
    // a header that claims far more bytes than allowed is refused before any data is written
    let d = tmp("bomb");
    let mut pol = rootfs::Policy::base();
    pol.max_bytes = 1 << 20;
    let mut big = file("./big", "");
    big.content = vec![0; 600 * 1024];
    let t = tar(&[big, {
        let mut b2 = file("./big2", "");
        b2.content = vec![0; 600 * 1024];
        b2
    }]);
    let err = unpack_bytes(&t, &d, &pol).unwrap_err();
    assert!(err.contains("larger than allowed"), "{err}");
    assert!(!d.join("big2").exists(), "stopped at the entry that went over");
    // too many files
    pol.max_bytes = 1 << 30;
    pol.max_files = 5;
    let many: Vec<E> = (0..10).map(|i| file(&format!("./f{i}"), "x")).collect();
    assert!(unpack_bytes(&tar(&many), &tmp("bomb2"), &pol).unwrap_err().contains("larger than allowed"));
    // a size field of 2^60 (base-256) is refused, not allocated
    let mut t = tar(&[file("./huge", "x")]);
    t[124] = 0x80;
    for b in &mut t[125..135] {
        *b = 0;
    }
    t[128] = 0x10;
    assert!(unpack_bytes(&t, &tmp("bomb3"), &rootfs::Policy::base()).is_err());
    // an archive cut short
    let t = tar(&[file("./a", "hello")]);
    assert!(unpack_bytes(&t[..514], &tmp("bomb4"), &rootfs::Policy::base()).is_err());
}

#[test]
fn layers_apply_in_order_with_whiteouts() {
    let d = tmp("layers");
    let mut pol = rootfs::Policy::base();
    pol.whiteouts = true;
    unpack_bytes(&tar(&[dir("./etc"), file("./etc/a", "1"), file("./etc/b", "2"), dir("./var"), file("./var/x", "x"), file("./keep", "k")]), &d, &pol).unwrap();
    unpack_bytes(&tar(&[file("./etc/.wh.a", ""), file("./var/.wh..wh..opq", ""), file("./var/y", "y"), file("./keep", "k2")]), &d, &pol).unwrap();
    assert!(!d.join("etc/a").exists() && d.join("etc/b").exists());
    assert!(!d.join("var/x").exists() && d.join("var/y").exists(), "an opaque marker empties the folder first");
    assert_eq!(std::fs::read_to_string(d.join("keep")).unwrap(), "k2");
    assert!(!d.join("etc/.wh.a").exists());
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn gzip_and_xz_files_are_detected_and_unpacked() {
    let d = tmp("comp");
    let t = tar(&[dir("./x"), file("./x/f", "hello")]);
    let gz = d.join("a.tar.gz");
    std::fs::write(&gz, crate::archive::tests::gzip(&t)).unwrap();
    let xz = d.join("a.tar.xz");
    std::fs::write(&xz, crate::xz::make_xz_stored(&t)).unwrap();
    let plain = d.join("a.tar");
    std::fs::write(&plain, &t).unwrap();
    let zst = d.join("a.tar.zst");
    std::fs::write(&zst, ruzstd::encoding::compress_to_vec(&t[..], ruzstd::encoding::CompressionLevel::Fastest)).unwrap();
    for (f, n) in [(&gz, "g"), (&xz, "x"), (&plain, "p"), (&zst, "z")] {
        let out = d.join(n);
        let r = rootfs::unpack(f, &out, &d.join("scratch"), &rootfs::Policy::base(), &mut |_, _| {}).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert_eq!(r.files, 1);
        assert_eq!(std::fs::read_to_string(out.join("x/f")).unwrap(), "hello");
    }
    assert!(!d.join("scratch/rootfs.tar.partial").exists(), "the scratch tar is removed");
    assert_eq!(rootfs::detect(&[0x28, 0xb5, 0x2f, 0xfd]), rootfs::Compression::Zstd);
    let _ = rootfs::remove_tree(&d);
}

// ------------------------------------------------------------------------------------------------ downloads

#[test]
fn the_ubuntu_sums_file_picks_the_highest_point_release_for_this_processor() {
    let arch = dist::arch();
    let sums = format!("{a}  *ubuntu-base-24.04.1-base-{arch}.tar.gz\n{b}  *ubuntu-base-24.04.10-base-{arch}.tar.gz\n{c}  *ubuntu-base-24.04.9-base-{arch}.tar.gz\n{d}  *ubuntu-base-24.04.12-base-s390x.tar.gz\nnot a line\n", a = "a".repeat(64), b = "b".repeat(64), c = "c".repeat(64), d = "d".repeat(64));
    let (name, hash) = dist::ubuntu_pick(&sums, "24.04", arch).unwrap();
    assert_eq!(name, format!("ubuntu-base-24.04.10-base-{arch}.tar.gz"));
    assert_eq!(hash, "b".repeat(64));
    assert!(dist::ubuntu_pick(&sums, "22.04", arch).is_none());
    assert!(dist::ubuntu_pick("short  *ubuntu-base-24.04.1-base-amd64.tar.gz", "24.04", "amd64").is_none(), "a hash that is not 64 hex digits is ignored");
}

#[test]
fn a_download_that_does_not_match_the_sums_file_is_refused_and_removed() {
    let d = tmp("sum-bad");
    let base = mini_base();
    let st = fake(ubuntu_routes(&base, &"0".repeat(64)));
    let rel = dist::release("ubuntu-24.04").unwrap();
    let err = dist::fetch(&st, rel, &d.join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap_err();
    assert!(err.contains("does not match") && err.contains("discarded"), "{err}");
    assert!(std::fs::read_dir(d.join("dl")).unwrap().next().is_none(), "the bad file is deleted");
    // and the good one passes, naming what was verified
    let st = fake(ubuntu_routes(&base, &sha(&base)));
    let (files, v) = dist::fetch(&st, rel, &d.join("dl2"), &|_| Ok(()), &mut |_, _| {}).unwrap();
    assert_eq!(files.len(), 1);
    assert!(v.verified.contains(&sha(&base)) && v.verified.contains("SHA256SUMS") && v.verified.contains("GPG signature of that list is not checked"), "{}", v.verified);
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn downloads_only_follow_https_redirects_to_known_hosts() {
    let base = mini_base();
    let mut routes = ubuntu_routes(&base, &sha(&base));
    routes[1].1 = (302, vec![("location".into(), "https://evil.example.org/x.tar.gz".into())], vec![]);
    let st = fake(routes);
    let err = dist::fetch(&st, dist::release("ubuntu-24.04").unwrap(), &tmp("redir").join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap_err();
    assert!(err.contains("not one of this tool's sources"), "{err}");
    let mut routes = ubuntu_routes(&base, &sha(&base));
    routes[1].1 = (302, vec![("location".into(), "http://cdimage.ubuntu.com/x".into())], vec![]);
    let err = dist::fetch(&fake(routes), dist::release("ubuntu-24.04").unwrap(), &tmp("redir2").join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap_err();
    assert!(err.contains("only https"), "{err}");
}

fn debian_routes(layer: &[u8], layer_digest: &str, index_arch: &str) -> (Vec<(String, Reply)>, String) {
    let manifest = serde_json::json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json", "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": layer_digest, "size": layer.len()}]}).to_string();
    let mdigest = format!("sha256:{}", sha(manifest.as_bytes()));
    let index = serde_json::json!({"schemaVersion": 2, "manifests": [
        {"digest": format!("sha256:{}", "e".repeat(64)), "platform": {"os": "linux", "architecture": "s390x"}},
        {"digest": mdigest, "platform": {"os": "linux", "architecture": index_arch}},
        {"digest": format!("sha256:{}", "f".repeat(64)), "platform": {"os": "unknown", "architecture": "unknown"}}]})
    .to_string();
    let routes = vec![
        ("https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/debian:pull".into(), ok(br#"{"token":"tok123"}"#)),
        ("https://registry-1.docker.io/v2/library/debian/manifests/bookworm-slim".into(), ok(index.as_bytes())),
        (format!("https://registry-1.docker.io/v2/library/debian/manifests/{mdigest}"), ok(manifest.as_bytes())),
        (format!("https://registry-1.docker.io/v2/library/debian/blobs/{layer_digest}"), (307, vec![("location".into(), "https://production.cloudflare.docker.com/blob/1".into())], vec![])),
        ("https://production.cloudflare.docker.com/blob/1".into(), ok(layer)),
    ];
    (routes, mdigest)
}

#[test]
fn debian_is_pulled_by_manifest_and_every_layer_is_checked_against_its_digest() {
    let layer = mini_base();
    let digest = format!("sha256:{}", sha(&layer));
    let (routes, mdigest) = debian_routes(&layer, &digest, dist::arch());
    let st = fake(routes);
    let rel = dist::release("debian-12").unwrap();
    let (files, v) = dist::fetch(&st, rel, &tmp("deb").join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap();
    assert_eq!(files.len(), 1);
    assert!(v.verified.contains(&mdigest) && v.source.contains("bookworm-slim"), "{v:?}");
    let asked = st.asked.lock().unwrap();
    let blob_hop = asked.iter().find(|(u, _)| u.starts_with("https://production.cloudflare")).unwrap();
    assert!(!blob_hop.1.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization")), "the registry token is not sent to the CDN host");
    assert!(asked.iter().any(|(u, h)| u.contains("/manifests/") && h.iter().any(|(k, v)| k == "Authorization" && v == "Bearer tok123")));
    drop(asked);
    // a layer that hashes differently is refused
    let (routes, _) = debian_routes(&layer, &format!("sha256:{}", "9".repeat(64)), dist::arch());
    let err = dist::fetch(&fake(routes), rel, &tmp("deb2").join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap_err();
    assert!(err.contains("manifest names") && err.contains("discarded"), "{err}");
    // an index without this processor
    let (routes, _) = debian_routes(&layer, &digest, "riscv64");
    assert!(dist::fetch(&fake(routes), rel, &tmp("deb3").join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap_err().contains("no linux/"));
}

#[test]
fn a_pinned_release_refuses_a_tag_that_moved() {
    let layer = mini_base();
    let digest = format!("sha256:{}", sha(&layer));
    let (routes, _) = debian_routes(&layer, &digest, dist::arch());
    let rel = dist::Release { pin: Some("sha256:0000000000000000000000000000000000000000000000000000000000000001"), ..*dist::release("debian-12").unwrap() };
    let err = dist::fetch(&fake(routes), &rel, &tmp("pin").join("dl"), &|_| Ok(()), &mut |_, _| {}).unwrap_err();
    assert!(err.contains("not the pinned"), "{err}");
}

#[test]
fn the_free_space_check_runs_before_the_big_transfer() {
    let base = mini_base();
    let st = fake(ubuntu_routes(&base, &sha(&base)));
    let err = dist::fetch(&st, dist::release("ubuntu-24.04").unwrap(), &tmp("space").join("dl"), &|_| Err("not enough free space".into()), &mut |_, _| {}).unwrap_err();
    assert!(err.contains("not enough"));
    // and the manager refuses at once when the volume is small
    let data = tmp("space2");
    let mut la = manager(&data, Arc::new(FakeRunner::new(&[])));
    la.assume(false, Some(100 << 20));
    let req = CreateReq { name: "dev".into(), release: "ubuntu-24.04".into(), network: false, mounts: vec![] };
    let err = la.create(&req, &fake(vec![]), &any_scope, &mut |_| {}).unwrap_err();
    assert!(err.contains("not enough free space"), "{err}");
    assert!(!data.join("linuxapps/admin/dev").exists(), "nothing was created");
}

// ------------------------------------------------------------------------------------------------ creating, running, deleting

#[test]
fn creating_a_sandbox_downloads_checks_unpacks_and_records_it() {
    let data = tmp("create");
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![]);
    let s = la.get("dev").unwrap();
    assert_eq!(s.release, "ubuntu-24.04");
    assert!(s.network && s.mounts.is_empty() && s.id_base == 0);
    assert!(s.verified.contains("SHA256SUMS"));
    let root = la.rootfs_of("dev");
    assert!(root.join("bin/dash").is_file());
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap();
    assert!(passwd.contains("root:x:0:0") && passwd.contains("user:x:1000:1000"), "{passwd}");
    assert!(std::fs::read_to_string(root.join("etc/group")).unwrap().contains("user:x:1000:"));
    assert!(root.join("home/user").is_dir() && root.join("usr/sbin/policy-rc.d").is_file());
    assert!(std::fs::read_to_string(root.join("etc/apt/apt.conf.d/90iyakki")).unwrap().contains("APT::Sandbox::User \"root\""));
    assert!(!la.dir_of("dev").join("download").exists(), "the download is removed after unpacking");
    assert_eq!(la.list().len(), 1);
    // the audit log has the create entry
    let a = la.audit_tail(10);
    assert!(a.iter().any(|x| x.2 == "sandbox.create" && x.3.contains("ubuntu-24.04") && x.3.contains("verified")), "{a:?}");
    // a second one of the same name is refused
    let base = mini_base();
    let err = la.create(&CreateReq { name: "dev".into(), release: "ubuntu-24.04".into(), network: false, mounts: vec![] }, &fake(ubuntu_routes(&base, &sha(&base))), &any_scope, &mut |_| {}).unwrap_err();
    assert!(err.contains("exists already"));
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn a_failed_create_leaves_nothing_behind_and_is_audited() {
    let data = tmp("create-bad");
    let la = manager(&data, Arc::new(FakeRunner::new(&[])));
    let base = mini_base();
    let st = fake(ubuntu_routes(&base, &"0".repeat(64)));
    let err = la.create(&CreateReq { name: "dev".into(), release: "ubuntu-24.04".into(), network: false, mounts: vec![] }, &st, &any_scope, &mut |_| {}).unwrap_err();
    assert!(err.contains("does not match"), "{err}");
    assert!(!la.dir_of("dev").exists() && la.list().is_empty());
    assert!(la.audit_tail(10).iter().any(|x| x.2 == "sandbox.create.failed"));
    // names and releases are validated before anything happens
    for bad in ["", "Dev", "a b", "-x", "x-", "1abc", "../x", &"a".repeat(31)] {
        assert!(!valid_name(bad), "{bad}");
    }
    let err = la.create(&CreateReq { name: "ok".into(), release: "arch-linux".into(), network: false, mounts: vec![] }, &fake(vec![]), &any_scope, &mut |_| {}).unwrap_err();
    assert!(err.contains("debian-12"), "{err}");
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn the_command_for_a_sandbox_is_a_golden_argument_array_without_a_shell() {
    let data = tmp("golden");
    let work = data.join("home/admin/work");
    let docs = data.join("home/admin/docs");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&docs).unwrap();
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![(work.display().to_string(), String::new(), true), (docs.display().to_string(), "/mnt/papers".into(), false)]);
    let root = std::fs::canonicalize(la.rootfs_of("dev")).unwrap().display().to_string();
    let (w, dd) = (std::fs::canonicalize(&work).unwrap().display().to_string(), std::fs::canonicalize(&docs).unwrap().display().to_string());
    let argv = la.exec_argv("dev", &["python3".into(), "-c".into(), "print('hi; rm -rf /')".into()], &ExecOpts::default()).unwrap();
    let want: Vec<String> = [
        "/usr/bin/iyakki", "linuxapp-init", "--root", &root, "--name", "dev", "--uid", "1000", "--gid", "1000", "--idmap", "single", "--net", "on",
        "--bind-rw", &w, "/mnt/work", "--bind-ro", &dd, "/mnt/papers",
        "--tmp-mb", "256", "--cwd", "/home/user", "--mem-mb", "2048", "--cpu-pct", "0", "--pids", "1024", "--seccomp", "on",
        "--", "python3", "-c", "print('hi; rm -rf /')",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(argv, want);
    // no shell anywhere in front of the command; the command stays one argument
    assert!(!argv.iter().take(argv.len() - 3).any(|a| a == "sh" || a == "-c" || a == "bash"));
    // the host paths that appear are exactly the sandbox folder and the folders that were mounted
    let host_paths: Vec<&String> = argv.iter().filter(|a| a.starts_with(&data.display().to_string())).collect();
    assert_eq!(host_paths, vec![&root, &w, &dd], "nothing else of the host is named");
    assert!(!argv.iter().any(|a| a.contains("/var/lib/iyakki") || a.contains("vault")));
    // the options parse back to the same specification
    let spec = init::Spec::parse(&argv[1..]).unwrap();
    assert_eq!(spec.binds.iter().map(|b| (b.dst.as_str(), b.rw)).collect::<Vec<_>>(), vec![("/mnt/work", true), ("/mnt/papers", false)]);
    // root inside is for apt only; the home changes with it
    let root_argv = la.exec_argv("dev", &["id".into()], &ExecOpts { root: true, ..Default::default() }).unwrap();
    assert!(root_argv.windows(2).any(|p| p == ["--uid", "0"]) && root_argv.windows(2).any(|p| p == ["--cwd", "/root"]));
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn the_network_switch_and_the_wide_id_range_show_in_the_command() {
    let data = tmp("net");
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![]);
    let net = |la: &LinuxApps| {
        let a = la.exec_argv("dev", &["true".into()], &ExecOpts::default()).unwrap();
        a[a.iter().position(|x| x == "--net").unwrap() + 1].clone()
    };
    assert_eq!(net(&la), "on");
    la.set_network("dev", false).unwrap();
    assert_eq!(net(&la), "off");
    assert!(la.audit_tail(5).iter().any(|x| x.2 == "sandbox.network" && x.3.contains("off")));
    // as root on the host the sandbox gets its own range of ids, and the files are owned in that range
    let data2 = tmp("multi");
    let mut la2 = manager(&data2, Arc::new(FakeRunner::new(&[])));
    la2.assume(true, Some(50 << 30));
    let base = mini_base();
    let st = fake(ubuntu_routes(&base, &sha(&base)));
    // lchown needs root: in a test run as a user the create reports that plainly instead of silently using the wrong owner
    let r = la2.create(&CreateReq { name: "dev".into(), release: "ubuntu-24.04".into(), network: true, mounts: vec![] }, &st, &any_scope, &mut |_| {});
    // SAFETY: a plain getter.
    if unsafe { libc::geteuid() } == 0 {
        r.unwrap();
        let a = la2.exec_argv("dev", &["true".into()], &ExecOpts::default()).unwrap();
        assert!(a.windows(2).any(|p| p == ["--idmap", "multi:2000000"]), "{a:?}");
    } else {
        assert!(r.unwrap_err().contains("could not set the owner"));
        assert!(!la2.dir_of("dev").exists());
    }
    let _ = rootfs::remove_tree(&data);
    let _ = rootfs::remove_tree(&data2);
}

#[test]
fn only_the_chosen_folders_can_be_mounted_and_never_the_data_volume() {
    let data = tmp("mounts");
    let home = data.join("home/admin");
    std::fs::create_dir_all(home.join("proj")).unwrap();
    std::fs::create_dir_all(data.join("vault")).unwrap();
    std::fs::write(home.join("file.txt"), "x").unwrap();
    // fine
    assert!(mount_source_ok(&home.join("proj").display().to_string(), &data).is_ok());
    // refused, each with its reason
    let cases: Vec<(String, &str)> = vec![
        (data.join("vault").display().to_string(), "inside IYAKKI's data folder"),
        (data.display().to_string(), "not a folder of yours"),
        (data.parent().unwrap().display().to_string(), "contains IYAKKI's data folder"),
        ("/".into(), "contains IYAKKI's data folder"),
        ("/etc".into(), "system folder"),
        ("/proc".into(), "system folder"),
        ("/dev".into(), "system folder"),
        (home.join("file.txt").display().to_string(), "not a folder"),
        ("relative/path".into(), "full path"),
        (home.join("nope").display().to_string(), "No such file"),
    ];
    for (p, why) in cases {
        let err = mount_source_ok(&p, &data).unwrap_err();
        assert!(err.contains(why), "{p}: {err}");
    }
    // a link that leads out is resolved first and then judged
    std::os::unix::fs::symlink(data.join("vault"), home.join("sneaky")).unwrap();
    assert!(mount_source_ok(&home.join("sneaky").display().to_string(), &data).unwrap_err().contains("data folder"));
    // inside-sandbox mount points
    for bad in ["/proc/x", "/dev/y", "/etc/z", "/usr/local", "/", "/mnt/../etc", "mnt/x", "/.old/x", "/tmp/x"] {
        assert!(init::bind_dst_ok(bad).is_err(), "{bad}");
    }
    for good in ["/mnt/work", "/home/user/src", "/opt/data", "/srv/x"] {
        assert!(init::bind_dst_ok(good).is_ok(), "{good}");
    }
    // the person's own file scope has the last word, and mount points must be unique
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![]);
    let deny = |_: &Path, _: bool| Err("your account may not write there".to_string());
    assert!(la.add_mount("dev", &home.join("proj").display().to_string(), "", true, &deny).unwrap_err().contains("may not write"));
    let m = la.add_mount("dev", &home.join("proj").display().to_string(), "", false, &any_scope).unwrap();
    assert_eq!(m.dst, "/mnt/proj");
    assert!(la.add_mount("dev", &home.join("proj").display().to_string(), "", false, &any_scope).unwrap_err().contains("two folders"));
    assert!(la.get("dev").unwrap().mounts.len() == 1);
    assert!(la.audit_tail(10).iter().any(|x| x.2 == "sandbox.mount" && x.3.contains("read-only")));
    la.remove_mount("dev", "/mnt/proj").unwrap();
    assert!(la.remove_mount("dev", "/mnt/proj").is_err());
    // a mounted folder replaced by a link after it was added is refused when the sandbox starts
    let victim = home.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    la.add_mount("dev", &victim.display().to_string(), "", true, &any_scope).unwrap();
    std::fs::remove_dir(&victim).unwrap();
    std::os::unix::fs::symlink(data.join("vault"), &victim).unwrap();
    assert!(la.exec_argv("dev", &["true".into()], &ExecOpts::default()).is_err());
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn package_names_are_validated_before_anything_runs() {
    for good in ["curl", "g++", "libstdc++6", "python3.12", "libc6-dev", "0ad", "x11-utils"] {
        assert!(valid_package(good), "{good}");
    }
    for bad in ["", "-y", "--option=x", "Curl", "a b", "a;b", "a$(b)", "a/b", "a=1.0", "pkg-", ".hidden", "+x", "é", &"a".repeat(101), "a\nb", "a:amd64"] {
        assert!(!valid_package(bad), "{bad:?}");
    }
    assert!(check_packages(&[]).is_err());
    assert!(check_packages(&(0..41).map(|i| format!("p{i}")).collect::<Vec<_>>()).is_err());
    assert!(check_packages(&["ok".into(), "--allow-unauthenticated".into()]).unwrap_err().contains("not a package name"));
    assert!(check_search(&["web".into(), "server".into()]).is_ok());
    for bad in ["-v", "a b", "$(x)", "", "a;b"] {
        assert!(check_search(&[bad.to_string()]).is_err(), "{bad}");
    }
}

#[test]
fn installing_packages_runs_update_then_install_as_root_inside_with_a_fixed_command() {
    let data = tmp("apt");
    let fr = Arc::new(FakeRunner::new(&[]));
    fr.answer("apt-get update", "Reading package lists...\n").answer("apt-get install", "Setting up curl\n");
    let la = make_sandbox(&data, fr.clone(), vec![]);
    let o = la.apt_install("dev", &["curl".into(), "git".into()], None).unwrap();
    assert!(o.ok());
    let lines = fr.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("-- apt-get update"), "{}", lines[0]);
    assert!(lines[1].ends_with("-- apt-get install -y --no-install-recommends curl git"), "{}", lines[1]);
    let calls = fr.calls();
    for c in &calls {
        assert!(c.argv.windows(2).any(|p| p == ["--uid", "0"]), "apt runs as root inside");
        let env = c.argv.windows(2).find(|p| p[0] == "--env" && p[1].starts_with("DEBIAN_FRONTEND")).map(|p| p[1].clone());
        assert_eq!(env.as_deref(), Some("DEBIAN_FRONTEND=noninteractive"));
    }
    let a = la.audit_tail(20);
    assert!(a.iter().any(|x| x.2 == "sandbox.apt.install" && x.3.contains("curl git")) && a.iter().any(|x| x.2 == "sandbox.apt.installed"), "{a:?}");
    // an invalid name stops everything: no program is run
    let before = fr.calls().len();
    let err = la.apt_install("dev", &["curl".into(), "-o Dir::Etc=/x".into()], None).unwrap_err();
    assert!(err.contains("not a package name"));
    assert_eq!(fr.calls().len(), before);
    // no network, no download
    la.set_network("dev", false).unwrap();
    assert!(la.apt_install("dev", &["curl".into()], None).unwrap_err().contains("network switched off"));
    assert_eq!(fr.calls().len(), before);
    // remove, upgrade, search and the installed list use fixed commands too
    la.set_network("dev", true).unwrap();
    fr.answer("apt-get remove", "Removing curl\n").answer("apt-get upgrade", "0 upgraded\n").answer("apt-cache search", "curl - command line tool\nlibcurl4 - library\n").answer("dpkg-query", "curl\t8.5.0-2\nbash\t5.2\n");
    assert!(la.apt_remove("dev", &["curl".into()], None).unwrap().ok());
    assert!(fr.lines().last().unwrap().ends_with("-- apt-get remove -y curl"));
    assert!(la.apt_upgrade("dev", None).unwrap().ok());
    assert!(fr.lines().last().unwrap().ends_with("-- apt-get upgrade -y"));
    let found = la.apt_search("dev", &["curl".into()]).unwrap();
    assert_eq!(found[0], ("curl".to_string(), "command line tool".to_string()));
    assert!(fr.lines().last().unwrap().ends_with("-- apt-cache search curl"));
    let inst = la.installed("dev").unwrap();
    assert_eq!(inst, vec![("curl".to_string(), "8.5.0-2".to_string()), ("bash".to_string(), "5.2".to_string())]);
    assert!(fr.lines().last().unwrap().ends_with("-- dpkg-query -W -f=${binary:Package}\t${Version}\n"));
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn a_terminal_line_is_split_into_words_never_given_to_a_shell() {
    let data = tmp("line");
    let fr = Arc::new(FakeRunner::new(&[]));
    fr.answer("ls", "a\n");
    let la = make_sandbox(&data, fr.clone(), vec![]);
    la.run_line("dev", "ls -la \"my dir\" ; rm -rf / && echo $HOME", None).unwrap();
    let argv = &fr.calls()[0].argv;
    let i = argv.iter().position(|a| a == "--").unwrap();
    assert_eq!(&argv[i + 1..], ["ls", "-la", "my dir", ";", "rm", "-rf", "/", "&&", "echo", "$HOME"], "operators are plain words");
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn deleting_needs_the_name_typed_and_removes_everything() {
    let data = tmp("delete");
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![]);
    // a folder inside with no permissions (packages do that) does not stop the deletion
    let hard = la.rootfs_of("dev").join("usr/locked");
    std::fs::create_dir_all(hard.join("sub")).unwrap();
    std::fs::write(hard.join("sub/f"), "x").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hard, std::fs::Permissions::from_mode(0o500)).unwrap();
    for typed in ["", "DEV", "dev ", "yes", "delete"] {
        let err = la.delete("dev", typed).unwrap_err();
        assert!(err.contains("type the name"), "{typed}: {err}");
        assert!(la.dir_of("dev").exists(), "nothing was deleted");
    }
    assert!(la.audit_tail(10).iter().any(|x| x.2 == "sandbox.delete.refused"));
    assert!(la.storage("dev") > 0);
    let freed = la.delete("dev", "dev").unwrap();
    assert!(freed > 0);
    assert!(!la.dir_of("dev").exists() && la.list().is_empty());
    assert!(la.audit_tail(10).iter().any(|x| x.2 == "sandbox.delete" && x.3.contains("dev")));
    assert!(la.get("dev").is_err());
    assert!(la.delete("../x", "../x").is_err());
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn the_audit_log_names_the_program_not_its_arguments() {
    let data = tmp("audit");
    let fr = Arc::new(FakeRunner::new(&[]));
    fr.answer("psql", "");
    let la = make_sandbox(&data, fr, vec![]);
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    la.set_audit(Arc::new(move |u, a, d| s2.lock().unwrap().push(format!("{u} {a} {d}"))));
    la.exec("dev", &["psql".into(), "postgres://me:hunter2@host/db".into()], &ExecOpts::default()).unwrap();
    let all = la.audit_tail(50);
    let line = all.iter().find(|x| x.2 == "sandbox.exec").unwrap();
    assert!(line.3.contains("psql") && line.3.contains("1 argument") && !line.3.contains("hunter2"), "{line:?}");
    assert!(seen.lock().unwrap().iter().any(|l| l.starts_with("admin sandbox.exec")));
    assert!(!std::fs::read_to_string(la.audit_path()).unwrap().contains("hunter2"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(la.audit_path()).unwrap().permissions().mode() & 0o777, 0o600);
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn a_tarball_the_person_assembled_becomes_a_sandbox() {
    let data = tmp("import");
    let la = manager(&data, Arc::new(FakeRunner::new(&[])));
    let tb = data.join("mine.tar.gz");
    std::fs::write(&tb, mini_base()).unwrap();
    let s = la.import("mine", &tb, false, &mut |_| {}).unwrap();
    assert_eq!(s.release, "imported");
    assert!(s.verified.starts_with("none") && s.verified.contains("computed here"), "{}", s.verified);
    assert!(la.rootfs_of("mine").join("bin/dash").is_file());
    assert!(la.audit_tail(5).iter().any(|x| x.2 == "sandbox.import"));
    // a hostile tarball leaves nothing behind
    let bad = data.join("bad.tar.gz");
    std::fs::write(&bad, crate::archive::tests::gzip(&tar(&[file("../escape", "x")]))).unwrap();
    assert!(la.import("bad", &bad, false, &mut |_| {}).is_err());
    assert!(!la.dir_of("bad").exists() && !data.join("linuxapps/escape").exists());
    let _ = rootfs::remove_tree(&data);
}

#[test]
fn a_job_is_the_iyakki_program_with_an_argument_array() {
    let l = crate::quickinstall::Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    let s = job_spec(&l, "admin", "Linux dev: install curl", &["install".into(), "dev".into(), "curl".into()]);
    match &s.cmd {
        crate::jobs::Cmd::Argv(a) => assert_eq!(a, &["/usr/bin/iyakki", "linux-apps", "--data", "/var/lib/iyakki", "--user", "admin", "install", "dev", "curl"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(s.workspace, crate::quickinstall::WORKSPACE, "the top bar's installer list shows it");
    assert!(is_linux_job("/usr/bin/iyakki linux-apps --data /d install dev curl"));
    assert!(!is_linux_job("/usr/bin/iyakki quick-install k3s"));
    // the verbs of the job program run against a fake runner
    let data = tmp("job");
    let fr = Arc::new(FakeRunner::new(&[]));
    fr.answer("apt-get", "ok\n");
    let la = make_sandbox(&data, fr.clone(), vec![]);
    cli_run(&la, &["install".into(), "dev".into(), "curl".into()]).unwrap();
    assert!(fr.lines().iter().any(|l| l.contains("apt-get install -y --no-install-recommends curl")));
    assert!(cli_run(&la, &["install".into(), "dev".into(), "--bad".into()]).is_err());
    assert!(cli_run(&la, &["frobnicate".into()]).is_err());
    let _ = rootfs::remove_tree(&data);
}

// ------------------------------------------------------------------------------------------------ the kernel

#[test]
fn the_probe_lists_the_kernel_options_that_are_missing() {
    let d = tmp("probe");
    let proc = d.join("proc");
    let cg = d.join("cg");
    std::fs::create_dir_all(proc.join("self/ns")).unwrap();
    std::fs::create_dir_all(proc.join("sys/user")).unwrap();
    std::fs::create_dir_all(&cg).unwrap();
    // nothing: everything is listed
    let p = Probe::read_at(&proc, &cg, true);
    assert!(!p.can_namespace());
    let m = p.missing().join(", ");
    for opt in ["CONFIG_USER_NS", "CONFIG_PID_NS", "CONFIG_NET_NS", "CONFIG_UTS_NS", "CONFIG_IPC_NS"] {
        assert!(m.contains(opt), "{m}");
    }
    assert!(choose_runtime(&p, false).unwrap_err().contains("kernel rebuild"));
    assert_eq!(choose_runtime(&p, true), Ok(Runtime::K3sPod), "the dev cluster is the fallback");
    // everything but a sysctl set to 0
    for n in ["user", "mnt", "pid", "uts", "ipc", "net"] {
        std::fs::write(proc.join("self/ns").join(n), "").unwrap();
    }
    std::fs::write(proc.join("sys/user/max_user_namespaces"), "0\n").unwrap();
    let p = Probe::read_at(&proc, &cg, true);
    assert!(p.missing().iter().any(|m| m.contains("max_user_namespaces")));
    std::fs::write(proc.join("sys/user/max_user_namespaces"), "31000\n").unwrap();
    let p = Probe::read_at(&proc, &cg, true);
    assert!(p.can_namespace());
    assert_eq!(choose_runtime(&p, false), Ok(Runtime::Namespaced));
    // optional things: cgroups and the filter
    let opt = p.optional_missing().join("\n");
    assert!(opt.contains("cgroup v2") && opt.contains("CONFIG_SECCOMP_FILTER"), "{opt}");
    std::fs::write(cg.join("cgroup.controllers"), "cpu memory\n").unwrap();
    let p = Probe::read_at(&proc, &cg, true);
    let opt = p.optional_missing().join("\n");
    assert!(opt.contains("pids cgroup") && !opt.contains("memory cgroup") && !opt.contains("cgroup v2"), "{opt}");
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn the_pod_fallback_builds_a_locked_down_pod_and_a_fixed_exec_command() {
    let m = pod_manifest("Admin", "dev", "registry.local/linuxapp-dev@sha256:abc");
    assert_eq!(m["metadata"]["namespace"], "iyakki-apps");
    assert_eq!(m["spec"]["automountServiceAccountToken"], false);
    assert_eq!(m["spec"]["hostNetwork"], false);
    assert_eq!(m["spec"]["containers"][0]["securityContext"]["privileged"], false);
    assert_eq!(m["spec"]["containers"][0]["securityContext"]["capabilities"]["drop"][0], "ALL");
    assert_eq!(pod_exec_argv("linuxapp-admin-dev", &["ls".into(), "-l".into()]), ["kubectl", "exec", "-n", "iyakki-apps", "linuxapp-admin-dev", "--", "ls", "-l"]);
}

// ------------------------------------------------------------------------------------------------ image apps

const PINNED: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn app_json(image: &str) -> String {
    format!(r#"{{"id":"demo","name":"Demo","description":"d","kind":"container","image":"{image}","command":["serve"],"ports":[{{"name":"http","port":8080}}],"env":[{{"name":"A","value":"b"}}],"volumes":[{{"name":"data","path":"/data","size_mb":8}}],"licence":"MIT"}}"#)
}

#[test]
fn an_image_without_a_digest_is_refused() {
    for bad in ["docker.io/library/busybox", "docker.io/library/busybox:latest", "docker.io/library/busybox:1.36", "busybox@sha256:abc", "busybox@sha256:ZZ23456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "busybox@md5:0123456789abcdef0123456789abcdef", "bus ybox@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "a/../b@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"] {
        assert!(imageapps::validate_image_ref(bad).is_err(), "{bad}");
        assert!(imageapps::parse(&app_json(bad)).is_err(), "{bad}");
    }
    let good = format!("docker.io/library/busybox:1.36@{PINNED}");
    imageapps::validate_image_ref(&good).unwrap();
    let a = imageapps::parse(&app_json(&good)).unwrap();
    assert!(a.pinned());
    // other fields are checked too
    let mut j: serde_json::Value = serde_json::from_str(&app_json(&good)).unwrap();
    j["licence"] = "".into();
    assert!(imageapps::parse(&j.to_string()).is_err());
    let mut j: serde_json::Value = serde_json::from_str(&app_json(&good)).unwrap();
    j["volumes"][0]["path"] = "/data/../etc".into();
    assert!(imageapps::parse(&j.to_string()).is_err());
    let mut j: serde_json::Value = serde_json::from_str(&app_json(&good)).unwrap();
    j["id"] = "Bad Id".into();
    assert!(imageapps::parse(&j.to_string()).is_err());
    assert!(imageapps::parse("{not json").is_err());
}

#[test]
fn the_three_examples_parse_are_marked_as_examples_and_wait_for_their_pins() {
    let (apps, problems) = imageapps::catalog(&[]);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(apps.len(), 3);
    assert!(apps.iter().all(|a| a.example && !a.licence.is_empty() && a.name.contains("example")));
    assert!(apps.iter().all(|a| !a.pinned()), "the shipped digests are placeholders until dist/release/pin-image-apps.sh runs");
    let err = imageapps::manifests(&apps[0]).unwrap_err();
    assert!(err.contains("not pinned yet"), "{err}");
    // a person's own files are read from their folder; a bad one is reported, a duplicate id too
    let d = tmp("catalog");
    std::fs::write(d.join("a-mine.json"), app_json(&format!("ghcr.io/me/app@{PINNED}"))).unwrap();
    std::fs::write(d.join("bad.json"), app_json("ghcr.io/me/app:latest")).unwrap();
    std::fs::write(d.join("dup.json"), app_json(&format!("ghcr.io/me/other@{PINNED}"))).unwrap();
    std::fs::write(d.join("notes.txt"), "ignored").unwrap();
    let (apps, problems) = imageapps::catalog(&[d.clone()]);
    assert_eq!(apps.len(), 4);
    assert!(apps.iter().any(|a| a.id == "demo"));
    assert!(problems.iter().any(|p| p.starts_with("bad.json") && p.contains("no digest")), "{problems:?}");
    assert!(problems.iter().any(|p| p.starts_with("dup.json") && p.contains("listed already")), "{problems:?}");
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn running_an_image_app_applies_a_locked_down_deployment_and_service_through_kubectl_only() {
    let a = imageapps::parse(&app_json(&format!("docker.io/library/busybox:1.36@{PINNED}"))).unwrap();
    let m = imageapps::manifests(&a).unwrap();
    let items = m["items"].as_array().unwrap();
    assert_eq!(items.iter().map(|i| i["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["Namespace", "Deployment", "Service"]);
    let dep = &items[1];
    assert_eq!(dep["metadata"]["namespace"], "iyakki-apps");
    let pod = &dep["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["securityContext"]["seccompProfile"]["type"], "RuntimeDefault");
    let c = &pod["containers"][0];
    assert_eq!(c["image"], format!("docker.io/library/busybox:1.36@{PINNED}"));
    assert_eq!(c["securityContext"]["allowPrivilegeEscalation"], false);
    assert_eq!(c["securityContext"]["privileged"], false);
    assert_eq!(c["securityContext"]["capabilities"]["drop"][0], "ALL");
    assert_eq!(c["securityContext"]["readOnlyRootFilesystem"], true);
    assert!(c["resources"]["limits"]["memory"].is_string());
    assert_eq!(c["command"][0], "serve");
    assert_eq!(c["env"][0]["name"], "A");
    assert_eq!(items[2]["spec"]["type"], "NodePort");
    assert!(!m.to_string().contains("hostNetwork") && !m.to_string().contains("hostPath") && !m.to_string().contains("privileged\":true"));
    // run: the manifest is written under the data folder and applied with an argument array
    let d = tmp("run-app");
    let calls = Mutex::new(Vec::<Vec<String>>::new());
    let kc = |args: &[String]| {
        calls.lock().unwrap().push(args.to_vec());
        crate::wsexec::Out { code: Some(0), stdout: "1".into(), ..Default::default() }
    };
    let msg = imageapps::run(&a, &d, &kc).unwrap();
    assert!(msg.contains("iyakki-apps") && msg.contains("pinned by digest"), "{msg}");
    let file = imageapps::manifest_path(&d, "demo");
    assert!(file.is_file());
    let want_path = file.display().to_string();
    assert_eq!(calls.lock().unwrap()[0], ["apply", "-f", want_path.as_str()]);
    // stop deletes by label in the namespace
    imageapps::stop(&a, &kc).unwrap();
    assert_eq!(calls.lock().unwrap()[1], ["delete", "deployment,service", "-n", "iyakki-apps", "-l", "iyakki.dev/image-app=demo", "--ignore-not-found"]);
    // a refusal by the cluster is reported
    let failing = |_: &[String]| crate::wsexec::Out { code: Some(1), stderr: "forbidden".into(), ..Default::default() };
    assert!(imageapps::run(&a, &d, &failing).unwrap_err().contains("forbidden"));
    assert_eq!(imageapps::status(&a, &failing), "not running");
    // a root file system entry is parsed but not run from the catalog yet
    let r = imageapps::parse(&format!(r#"{{"id":"rf","name":"R","description":"d","kind":"rootfs","rootfs_url":"https://example.org/r.tar.gz","rootfs_sha256":"{}","licence":"MIT"}}"#, "a".repeat(64))).unwrap();
    assert!(imageapps::manifests(&r).unwrap_err().contains("planned"));
    let _ = rootfs::remove_tree(&d);
}

// ------------------------------------------------------------------------------------------------ one real sandbox (needs user namespaces)

/// Runs `busybox` in a real sandbox with `unshare` system calls. Needs a static `busybox` on the host and user namespaces:
/// `cargo test -p iyakki-agent --lib linuxapps_tests::a_real_sandbox -- --ignored --test-threads=1`.
#[test]
#[ignore]
fn a_real_sandbox_runs_busybox_cannot_see_the_host_and_respects_the_switches() {
    real_sandbox(false);
}

/// The same checks through the way used on an initramfs root (move, chroot), forced on this PC.
#[test]
#[ignore]
fn a_real_sandbox_entered_the_initramfs_way_behaves_the_same() {
    init::FORCE_MOVE.store(true, std::sync::atomic::Ordering::SeqCst);
    real_sandbox(true);
    init::FORCE_MOVE.store(false, std::sync::atomic::Ordering::SeqCst);
}

fn real_sandbox(moved: bool) {
    let _ = moved;
    let bb = *["/usr/bin/busybox", "/bin/busybox"].iter().find(|p| Path::new(p).is_file()).expect("a static busybox on the host");
    let d = tmp("real");
    let root = d.join("rootfs");
    for sub in ["bin", "etc", "proc", "dev", "tmp", "home/user", "mnt/work", "mnt/ro"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::copy(bb, root.join("bin/busybox")).unwrap();
    for a in ["sh", "ls", "cat", "id", "echo", "touch", "mount", "ip", "ping", "sleep", "hostname", "ps", "wc", "head", "dd"] {
        let _ = std::os::unix::fs::symlink("busybox", root.join("bin").join(a));
    }
    std::fs::write(root.join("etc/passwd"), "user:x:1000:1000::/home/user:/bin/sh\n").unwrap();
    let rw = d.join("work");
    let ro = d.join("ro");
    std::fs::create_dir_all(&rw).unwrap();
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::write(ro.join("note"), "readable").unwrap();
    std::fs::write(d.join("host-secret"), "SECRET").unwrap();
    let run = |net: bool, cmd: &[&str]| -> (i32, String) {
        let mut s = init::Spec::new(&root.display().to_string(), "realtest", cmd.iter().map(|c| c.to_string()).collect());
        s.net = net;
        s.binds = vec![init::Bind { src: rw.display().to_string(), dst: "/mnt/work".into(), rw: true }, init::Bind { src: ro.display().to_string(), dst: "/mnt/ro".into(), rw: false }];
        s.seccomp = Path::new("/proc/sys/kernel/seccomp/actions_avail").exists();
        let args = s.to_args();
        let out = d.join("out.txt");
        // SAFETY: the test runs alone (--test-threads=1); the child only calls the sandbox code and exits.
        unsafe {
            let pid = libc::fork();
            if pid == 0 {
                let c = std::ffi::CString::new(out.display().to_string()).unwrap();
                let fd = libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o600);
                libc::dup2(fd, 1);
                libc::dup2(fd, 2);
                libc::_exit(init::main(&args));
            }
            let mut st = 0;
            libc::waitpid(pid, &mut st, 0);
            (libc::WEXITSTATUS(st), std::fs::read_to_string(&out).unwrap_or_default())
        }
    };
    let (c, o) = run(false, &["id", "-u"]);
    assert_eq!((c, o.trim()), (0, "1000"), "not root inside: {o}");
    let (_, o) = run(false, &["ls", "/"]);
    assert!(!o.contains("sys") && o.contains("bin") && o.contains("proc") && o.contains("mnt"), "{o}");
    let (c, _) = run(false, &["cat", &d.join("host-secret").display().to_string()]);
    assert_ne!(c, 0, "the host's files are not there");
    let (c, o) = run(false, &["cat", "/mnt/ro/note"]);
    assert_eq!((c, o.trim()), (0, "readable"));
    let (c, _) = run(false, &["touch", "/mnt/ro/x"]);
    assert_ne!(c, 0, "read-only means read-only");
    assert!(!ro.join("x").exists());
    let (c, _) = run(false, &["touch", "/mnt/work/made-inside"]);
    assert_eq!(c, 0);
    assert!(rw.join("made-inside").exists(), "the read-write folder is shared");
    let (c, o) = run(false, &["ls", "/proc"]);
    assert_eq!(c, 0);
    assert!(o.lines().any(|l| l == "1"), "its own pid namespace: it is pid 1 ({o})");
    let (_, o) = run(false, &["ip", "-o", "link"]);
    assert!(o.contains("lo") && !o.contains("eth") && !o.contains("wlan"), "an empty network: only the loopback ({o})");
    let (_, o) = run(true, &["ip", "-o", "link"]);
    let host_has_more = std::fs::read_dir("/sys/class/net").map(|d| d.count()).unwrap_or(1) > 1;
    assert!(!host_has_more || o.lines().count() > 1, "network on shares the host's interfaces ({o})");
    let (c, _) = run(false, &["mount", "-t", "tmpfs", "x", "/mnt/work"]);
    assert_ne!(c, 0, "no mounting from inside");
    let _ = rootfs::remove_tree(&d);
}

// ------------------------------------------------------------------------------------------------ Quick install

#[test]
fn quick_install_has_a_linux_apps_entry_that_makes_a_debian_sandbox_as_a_job() {
    use crate::quickinstall::*;
    assert_eq!(Item::parse("linux-apps"), Some(Item::LinuxApps));
    assert_eq!(Item::LinuxApps.group(), Group::Linux);
    assert_eq!(items_of(Group::Linux), [Item::LinuxApps, Item::LinuxWindows]);
    assert!(Item::LinuxApps.title().contains("Linux apps (Debian/Ubuntu)"));
    assert!(Item::LinuxApps.licence().contains("do not rebrand") || Item::LinuxApps.licence().contains("does not rebrand"));
    assert!(Item::LinuxApps.about().contains("never part of an image") && Item::LinuxApps.about().contains("checksum"));
    assert!(Item::LinuxApps.runs_as_job() && Item::LinuxApps.needs_now().is_empty(), "it needs no cluster");
    let l = Launch { exe: "/usr/bin/iyakki".into(), data: "/var/lib/iyakki".into() };
    assert_eq!(spec(Item::LinuxApps, 1, 1, &l).cmd, crate::jobs::Cmd::Argv(vec!["/usr/bin/iyakki".into(), "quick-install".into(), "linux-apps".into(), "--data".into(), "/var/lib/iyakki".into()]));
    // the step shows the licence note, makes the sandbox through the environment and marks itself done
    struct Env(Vec<String>, bool);
    impl StepEnv for Env {
        fn dep_state(&mut self, _: Item) -> DepState { DepState::Ready }
        fn say(&mut self, l: &str) { self.0.push(l.into()) }
        fn k3s_present(&mut self) -> bool { false }
        fn k3s_elsewhere(&mut self) -> Option<String> { None }
        fn install_k3s(&mut self) -> Result<String, String> { Err("no".into()) }
        fn request_k3s_start(&mut self) -> Result<(), String> { Ok(()) }
        fn cluster_ready(&mut self) -> bool { false }
        fn install_cli(&mut self, _: &str) -> Result<String, String> { Err("no".into()) }
        fn kubectl_apply(&mut self, _: &str, _: &str) -> Result<String, String> { Err("no".into()) }
        fn rollout(&mut self, _: &str, _: &str, _: std::time::Duration) -> Result<(), String> { Err("no".into()) }
        fn sleep(&mut self, _: std::time::Duration) {}
        fn mark(&mut self, item: Item, detail: &str) -> Result<(), String> {
            assert_eq!(item, Item::LinuxApps);
            assert!(detail.contains("sandbox debian"));
            self.1 = true;
            Ok(())
        }
        fn request_after(&mut self, _: &str) -> Result<(), String> { Ok(()) }
        fn lemonade_ready(&mut self) -> bool { false }
        fn lemonade_pull(&mut self, _: &str) -> Result<(), String> { Err("no".into()) }
        fn linux_apps(&mut self) -> Result<String, String> { Ok("sandbox debian (Debian 12) is ready".into()) }
    }
    let mut env = Env(vec![], false);
    run_step(Item::LinuxApps, &mut env, std::time::Duration::ZERO, std::time::Duration::ZERO).unwrap();
    assert!(env.1 && env.0.iter().any(|l| l.contains("Ubuntu") && l.contains("trademark")), "{:?}", env.0);
    // its state follows the sandboxes on disk
    assert_eq!(count_all(Path::new("/nonexistent-dir-for-linuxapps")), 0);
    let data = tmp("qi-count");
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![]);
    assert_eq!(count_all(&data), 1);
    la.delete("dev", "dev").unwrap();
    assert_eq!(count_all(&data), 0);
    let _ = rootfs::remove_tree(&data);
}

// ------------------------------------------------------------------------------------------------ the AI IDE

#[test]
fn a_workspace_can_run_its_commands_in_a_sandbox_with_only_its_own_folder_mounted() {
    let data = tmp("ide-data");
    let ws = tmp("ide-ws");
    let la = make_sandbox(&data, Arc::new(FakeRunner::new(&[])), vec![]);
    let exe = Path::new("/usr/bin/iyakki");
    let ctx = Some((data.clone(), "admin".to_string()));
    std::fs::create_dir_all(ws.join("src/deep")).unwrap();
    crate::workspace::ProjectConfig::ensure(&ws, "demo").unwrap();
    let argv = vec!["cargo".to_string(), "test".to_string(), "-p".to_string(), "x y".to_string()];
    // no sandbox chosen: the command is untouched
    assert_eq!(ide_wrap_with(ctx.clone(), exe, &ws, argv.clone()).unwrap(), argv);
    // outside any workspace: untouched
    assert_eq!(ide_wrap_with(ctx.clone(), exe, Path::new("/"), argv.clone()).unwrap(), argv);
    // choose the sandbox (the project file keeps it; unknown sandboxes are refused)
    assert!(set_workspace_sandbox(&la, &ws, Some("nope")).is_err());
    let msg = set_workspace_sandbox(&la, &ws, Some("dev")).unwrap();
    assert!(msg.contains("/mnt/workspace"), "{msg}");
    assert_eq!(crate::workspace::ProjectConfig::load(&ws).unwrap().sandbox, "dev");
    let root_c = std::fs::canonicalize(&ws).unwrap().display().to_string();
    let w = ide_wrap_with(ctx.clone(), exe, &ws.join("src/deep"), argv.clone()).unwrap();
    assert_eq!(w[0], "/usr/bin/iyakki");
    assert_eq!(w[1], "linuxapp-init");
    assert!(w.windows(3).any(|p| p == ["--bind-rw", root_c.as_str(), "/mnt/workspace"]), "{w:?}");
    assert!(w.windows(2).any(|p| p == ["--cwd", "/mnt/workspace/src/deep"]), "the same sub-folder inside: {w:?}");
    let i = w.iter().position(|a| a == "--").unwrap();
    assert_eq!(&w[i + 1..], argv.as_slice(), "the command is passed as it is, one argument each");
    let binds: Vec<&String> = w.windows(1).map(|p| &p[0]).filter(|a| a.starts_with(&data.display().to_string()) || a.starts_with(&ws.display().to_string())).collect();
    assert_eq!(binds.len(), 2, "the sandbox folder and the workspace are the only host paths: {binds:?}");
    // the sandbox's own record did not gain a mount
    assert!(la.get("dev").unwrap().mounts.is_empty());
    assert!(la.audit_tail(10).iter().any(|x| x.2 == "sandbox.runner") && la.audit_tail(10).iter().any(|x| x.2 == "sandbox.ide-run"));
    // no person known: a plain explanation, not a run on the machine
    assert!(ide_wrap_with(None, exe, &ws, argv.clone()).unwrap_err().contains("which person"));
    // a deleted sandbox stops the command instead of running it on the machine
    la.delete("dev", "dev").unwrap();
    assert!(ide_wrap_with(ctx.clone(), exe, &ws, argv.clone()).is_err());
    // back to this machine
    let la2 = manager(&data, Arc::new(FakeRunner::new(&[])));
    set_workspace_sandbox(&la2, &ws, None).unwrap();
    assert_eq!(ide_wrap_with(ctx, exe, &ws, argv.clone()).unwrap(), argv);
    assert!(!std::fs::read_to_string(crate::workspace::ProjectConfig::path(&ws)).unwrap().contains("sandbox"), "an empty choice is not written");
    let _ = rootfs::remove_tree(&data);
    let _ = rootfs::remove_tree(&ws);
}

//! Hot patches without a device: the system root is a folder, the effects (bind mounts, service restarts) are [`FakeLive`]. The pack
//! format and its tampering, the path rules, the kubectl wrapper that motivated all this, live apply with undo on any failure, next-start
//! patches, obsolete and superseded ones, the start counter, the full-boot overlay, and the approval gate of the routes.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::patch::*;
use super::DevMode;

struct Rig {
    dir: PathBuf,
    root: PathBuf,
    data: PathBuf,
    dm: DevMode,
    live: FakeLive,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn put(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const RELEASE: &str = "VERSION=0.1.0-alpha.2\nVARIANT=macweb\nBUILD_DATE=2026-10-22\n";

fn rig(tag: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-patch-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (root, data) = (dir.join("root"), dir.join("data"));
    put(&root.join("usr/bin/k3s"), "the k3s binary\n");
    std::os::unix::fs::symlink("k3s", root.join("usr/bin/kubectl")).unwrap();
    put(&root.join("usr/bin/helper"), "helper v1\n");
    put(&root.join("etc/asound.conf"), "old asound\n");
    put(&root.join("etc/old.conf"), "old conf\n");
    put(&data.join("iyakki/root/current"), "n1\nsize=1\n");
    let dm = DevMode::open(&data);
    Rig { dir, root, data, dm, live: FakeLive::default() }
}

impl Rig {
    fn env(&self, wide: bool) -> Env<'_> {
        Env { dm: &self.dm, live: &self.live, root: self.root.clone(), release: RELEASE.into(), wide, data_free: &|| 10 << 30 }
    }

    fn read(&self, p: &str) -> String {
        std::fs::read_to_string(self.root.join(p)).unwrap()
    }

    /// Stage the pack's files the way the routes do, ready for `apply`.
    fn stage(&self, p: &Pack) {
        let st = staging_dir(&self.data, &p.manifest.id);
        std::fs::create_dir_all(&st).unwrap();
        for (i, b) in &p.content {
            std::fs::write(st.join(staged_name(*i)), b).unwrap();
        }
    }

    fn apply(&self, p: &Pack, wide: bool, mode: Mode) -> Result<serde_json::Value, String> {
        self.stage(p);
        apply(&self.env(wide), &p.manifest, mode, "laptop")
    }
}

fn op(path: &str, action: Action) -> Op {
    Op { path: path.into(), action, kind: Kind::File, mode: None, uid: 0, gid: 0, size: 0, sha256: String::new(), link: None }
}

/// A pack made through the real builder from a source folder.
fn pack(r: &Rig, id: &str, ops: Vec<(Op, Option<&str>)>, tweak: impl FnOnce(&mut Manifest)) -> Pack {
    let src = r.dir.join(format!("src-{id}"));
    let _ = std::fs::remove_dir_all(&src);
    let mut m = Manifest { format: FORMAT, id: id.into(), title: format!("patch {id}"), description: String::new(), created: "2026-10-24".into(), base: "*".into(), obsolete_from: None, ops: vec![], pre: vec![], post: vec![] };
    for (o, content) in ops {
        if let Some(c) = content {
            let p = src.join("files").join(o.path.trim_start_matches('/'));
            put(&p, c);
            if o.mode.as_deref() == Some("0755") {
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        m.ops.push(o);
    }
    tweak(&mut m);
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join(MANIFEST), serde_json::to_vec(&m).unwrap()).unwrap();
    decode(&build(&src).unwrap()).unwrap()
}

fn exec(path: &str, action: Action) -> Op {
    Op { mode: Some("0755".into()), ..op(path, action) }
}

const WRAPPER: &str = "#!/bin/sh\n# kubectl wrapper: -A after the verb\nexec /usr/bin/k3s kubectl \"$@\"\n";

#[test]
fn a_pack_is_checksummed_and_any_change_is_found() {
    let r = rig("pack");
    let p = pack(&r, "k8s-fix", vec![(exec("/usr/bin/kubectl", Action::Replace), Some(WRAPPER)), (op("/etc/old.conf", Action::Delete), None)], |_| {});
    assert_eq!(p.manifest.ops[0].size, WRAPPER.len() as u64);
    assert_eq!(p.manifest.ops[0].sha256, sha256_hex(WRAPPER.as_bytes()));
    let bytes = encode(&p.manifest, &p.content);
    assert_eq!(decode(&bytes).unwrap().content, p.content);
    // a flipped byte in a file, in the manifest, a cut, a wrong magic, and trailing bytes are all refused
    for at in [bytes.len() - 40, 20, 8] {
        let mut b = bytes.clone();
        b[at] ^= 1;
        assert!(decode(&b).is_err(), "byte {at}");
    }
    assert!(decode(&bytes[..bytes.len() - 5]).unwrap_err().contains("checksum"));
    assert!(decode(b"IYPATCH0 not a pack at all, long enough to pass the length check....").unwrap_err().contains("not an IYAKKI patch"));
    // a file changed AND the trailer fixed up: the per-file SHA-256 still catches it
    let mut forged = bytes[..bytes.len() - 32].to_vec();
    let n = forged.len();
    forged[n - 3] ^= 1;
    use sha2::{Digest, Sha256};
    forged.extend(Sha256::digest(&forged).to_vec());
    assert!(decode(&forged).unwrap_err().contains("SHA-256"));
    // the builder refuses a missing file and a link as content
    let src = r.dir.join("src-bad");
    put(&src.join(MANIFEST), &serde_json::to_string(&Manifest { ops: vec![op("/usr/bin/x", Action::Add)], ..p.manifest.clone() }).unwrap());
    assert!(build(&src).unwrap_err().contains("put the file"));
    // a folder in the build layout loads like a pack
    assert_eq!(load(&r.dir.join("src-k8s-fix")).unwrap().manifest.id, "k8s-fix");
}

#[test]
fn paths_are_checked_against_never_normal_wide_and_next_start_rules() {
    for ok in ["/usr/bin/kubectl", "/usr/share/x/y.conf", "/etc/asound.conf", "/etc/iyakki/x.toml", "/opt/tool/bin/t"] {
        assert_eq!(path_reach(ok), Ok(Reach::Normal), "{ok}");
    }
    for wide in ["/etc/profile", "/bin/ls", "/lib/libfoo.so", "/var/cache/x", "/etc/inittab"] {
        assert_eq!(path_reach(wide), Ok(Reach::Wide), "{wide}");
    }
    for boot in ["/sbin/init", "/usr/bin/iyakki", "/lib/modules/6.12/x.ko", "/lib/firmware/x.bin", "/init"] {
        assert_eq!(path_reach(boot), Ok(Reach::Reboot), "{boot}");
    }
    for never in [
        "/boot/grub/grub.cfg", "/etc/shadow", "/etc/passwd", "/var/lib/iyakki/vault/system.afv", "/var/lib/iyakki", "/home/bob/x", "/root/.ssh/authorized_keys", "/etc/ssh/ssh_host_ed25519_key", "/usr/share/x/server.key", "/usr/bin/../../etc/shadow", "relative/x",
        "/usr//bin/x", "/usr/bin/", "/proc/1/mem", "/.stage1/boot/x", "/etc/ssl/private/a", "/usr/lib/id_ed25519", "/tmp/x", "/usr/bin/a b\nc",
    ] {
        assert!(path_reach(never).is_err(), "{never:?} must never be patched");
    }
}

#[test]
fn the_manifest_rules_limit_what_a_patch_may_carry_without_the_shell_scope() {
    let r = rig("rules");
    let good = pack(&r, "ok", vec![(exec("/usr/bin/x", Action::Add), Some("x"))], |_| {}).manifest;
    assert!(check_manifest(&good, false).is_ok());
    let with = |f: &dyn Fn(&mut Manifest)| {
        let mut m = good.clone();
        f(&mut m);
        m
    };
    let wide_path = with(&|m| m.ops[0].path = "/etc/profile".into());
    assert!(check_manifest(&wide_path, false).unwrap_err().contains("shell scope"));
    assert!(check_manifest(&wide_path, true).is_ok());
    assert!(check_manifest(&with(&|m| m.ops[0].path = "/etc/shadow".into()), true).is_err(), "never, even wide");
    assert!(check_manifest(&with(&|m| m.ops[0].mode = Some("4755".into())), false).unwrap_err().contains("setuid"));
    assert!(check_manifest(&with(&|m| m.ops[0].mode = Some("xyz".into())), false).is_err());
    assert!(check_manifest(&with(&|m| m.post.push(Step::Shell { cmd: "id".into() })), false).unwrap_err().contains("shell scope"));
    assert!(check_manifest(&with(&|m| m.post.push(Step::Shell { cmd: "id".into() })), true).is_ok());
    assert!(check_manifest(&with(&|m| m.post.push(Step::RestartService { name: "iyakki".into() })), true).unwrap_err().contains("cannot be restarted"));
    assert!(check_manifest(&with(&|m| m.post.push(Step::Remount { path: "/boot".into(), rw: true })), true).is_err());
    assert!(check_manifest(&with(&|m| m.ops.push(m.ops[0].clone())), false).unwrap_err().contains("twice"));
    assert!(check_manifest(&with(&|m| m.ops[0].sha256 = "zz".into()), false).is_err());
    assert!(check_manifest(&with(&|m| m.ops[0].size = MAX_FILE + 1), false).unwrap_err().contains("at most"));
    assert!(check_manifest(&with(&|m| m.id = "../x".into()), false).is_err());
    assert!(check_manifest(&with(&|m| m.format = 9), false).is_err());
    assert!(check_manifest(&with(&|m| m.ops.clear()), false).is_err());
    assert!(check_manifest(&with(&|m| m.base = "a/b".into()), false).is_err());
    assert!(check_manifest(&with(&|m| m.ops[0] = Op { link: Some("x".into()), ..m.ops[0].clone() }), false).is_err(), "a file with a link target");
}

#[test]
fn the_kubectl_wrapper_replaces_the_symlink_live_and_the_symlink_comes_back() {
    let r = rig("kube");
    let p = pack(&r, "k8s-verb-first", vec![(exec("/usr/bin/kubectl", Action::Replace), Some(WRAPPER))], |m| m.post.push(Step::Sync));
    let v = r.apply(&p, false, Mode::Live).unwrap();
    assert_eq!(v["live"], true);
    // a link is swapped for the file: a bind mount would have landed on the k3s binary itself
    let md = std::fs::symlink_metadata(r.root.join("usr/bin/kubectl")).unwrap();
    assert!(md.is_file() && md.permissions().mode() & 0o777 == 0o755);
    assert_eq!(r.read("usr/bin/kubectl"), WRAPPER);
    assert_eq!(r.read("usr/bin/k3s"), "the k3s binary\n");
    assert!(r.live.calls().iter().all(|c| !c.starts_with("bind")), "{:?}", r.live.calls());
    assert_eq!(r.live.calls(), ["sync"]);
    // stored for the next start: the layer is whole and the first stage's own planner accepts it
    let d = patches_dir(&r.data);
    assert_eq!(std::fs::read_to_string(d.join("active")).unwrap(), "k8s-verb-first\n");
    assert_eq!(std::fs::read_to_string(d.join("k8s-verb-first/layer/usr/bin/kubectl")).unwrap(), WRAPPER);
    let read = |x: &str| std::fs::read_to_string(r.data.join(x)).ok();
    let stat = |x: &str| std::fs::symlink_metadata(r.data.join(x)).ok().map(|m| m.len());
    let plan = iyakki_stage1::plan_patches("k8s-verb-first", 0, Some("n1"), &read, &stat);
    assert_eq!(plan.layers, ["iyakki/patches/k8s-verb-first/layer"], "{:?}", plan.skipped);
    // the old link is kept for the way back
    assert!(std::fs::read_to_string(d.join("k8s-verb-first/undo/undo.json")).unwrap().contains("\"k3s\""));
    // removing puts the link back
    let v = remove(&r.env(false), "k8s-verb-first", "laptop").unwrap();
    assert!(v["message"].as_str().unwrap().contains("removed"));
    assert_eq!(std::fs::read_link(r.root.join("usr/bin/kubectl")).unwrap(), Path::new("k3s"));
    assert!(!d.join("k8s-verb-first").exists());
    assert_eq!(std::fs::read_to_string(d.join("active")).unwrap(), "");
}

#[test]
fn a_regular_file_is_bound_over_and_an_added_file_is_created_and_removed_again() {
    let r = rig("bind");
    let p = pack(&r, "audio", vec![(op("/etc/asound.conf", Action::Replace), Some("new asound\n")), (exec("/usr/bin/newtool", Action::Add), Some("#!/bin/sh\necho hi\n")), (op("/etc/old.conf", Action::Delete), None)], |m| m.post.push(Step::RestartService { name: "pipewire".into() }));
    // the delete is outside the normal places? /etc/old.conf is wide: refused without the shell scope
    assert!(r.apply(&p, false, Mode::Live).unwrap_err().contains("shell scope"));
    let p = pack(&r, "audio", vec![(op("/etc/asound.conf", Action::Replace), Some("new asound\n")), (exec("/usr/bin/newtool", Action::Add), Some("#!/bin/sh\necho hi\n"))], |m| m.post.push(Step::RestartService { name: "pipewire".into() }));
    r.apply(&p, false, Mode::Live).unwrap();
    assert_eq!(r.read("etc/asound.conf"), "new asound\n");
    assert_eq!(r.read("usr/bin/newtool"), "#!/bin/sh\necho hi\n");
    assert_eq!(r.live.calls(), [format!("bind {}/etc/asound.conf", r.root.display()), format!("bind {}/usr/bin/newtool", r.root.display()), "restart pipewire".to_string()]);
    let l = list(&r.env(false));
    assert_eq!(l["patches"][0]["status"], "live");
    remove(&r.env(false), "audio", "laptop").unwrap();
    assert_eq!(r.read("etc/asound.conf"), "old asound\n");
    assert!(!r.root.join("usr/bin/newtool").exists(), "a file the patch created goes away");
    assert!(r.live.calls().iter().any(|c| c.starts_with("unbind")));
    assert_eq!(r.live.calls().last().unwrap(), "restart pipewire", "the post steps run again so the service sees the old file");
}

#[test]
fn a_deletion_unlinks_live_keeps_a_copy_and_restores_it() {
    let r = rig("del");
    let p = pack(&r, "drop-old", vec![(op("/etc/old.conf", Action::Delete), None)], |_| {});
    r.apply(&p, true, Mode::Live).unwrap();
    assert!(!r.root.join("etc/old.conf").exists());
    let d = patches_dir(&r.data).join("drop-old");
    assert!(d.join("layer/etc/old.conf").exists(), "the whiteout is in the layer");
    assert!(std::fs::read_to_string(d.join("layer.txt")).unwrap().contains("del 0 /etc/old.conf"));
    remove(&r.env(true), "drop-old", "laptop").unwrap();
    assert_eq!(r.read("etc/old.conf"), "old conf\n");
}

#[test]
fn a_failure_halfway_undoes_everything_and_leaves_no_trace() {
    let r = rig("fail");
    let p = pack(&r, "two", vec![(op("/etc/asound.conf", Action::Replace), Some("new\n")), (exec("/usr/bin/helper", Action::Replace), Some("helper v2\n"))], |_| {});
    r.live.fail.lock().unwrap().push(format!("bind {}/usr/bin/helper", r.root.display()));
    let e = r.apply(&p, false, Mode::Live).unwrap_err();
    assert!(e.contains("nothing was left changed"), "{e}");
    assert_eq!(r.read("etc/asound.conf"), "old asound\n");
    assert_eq!(r.read("usr/bin/helper"), "helper v1\n");
    assert!(!patches_dir(&r.data).join("two").exists());
    assert!(read_active_for_test(&r).is_empty());
    // a step before that fails stops it before any change
    let p = pack(&r, "pre", vec![(op("/etc/asound.conf", Action::Replace), Some("new\n"))], |m| m.pre.push(Step::DropCaches));
    r.live.fail.lock().unwrap().push("drop_caches".into());
    assert!(r.apply(&p, false, Mode::Live).unwrap_err().contains("not applied"));
    assert_eq!(r.read("etc/asound.conf"), "old asound\n");
    // a step after that fails is a warning: the patch stays
    r.live.fail.lock().unwrap().clear();
    let p = pack(&r, "post", vec![(op("/etc/asound.conf", Action::Replace), Some("new\n"))], |m| m.post.push(Step::RestartService { name: "sshd".into() }));
    r.live.fail.lock().unwrap().push("restart sshd".into());
    let v = r.apply(&p, false, Mode::Live).unwrap();
    assert!(v["warnings"][0].as_str().unwrap().contains("a step after it failed"));
    assert_eq!(r.read("etc/asound.conf"), "new\n");
}

fn read_active_for_test(r: &Rig) -> Vec<String> {
    iyakki_stage1::parse_active(&std::fs::read_to_string(patches_dir(&r.data).join("active")).unwrap_or_default())
}

#[test]
fn next_start_mode_stores_without_touching_the_running_system_and_pid1_is_never_live() {
    let r = rig("next");
    let p = pack(&r, "later", vec![(op("/etc/asound.conf", Action::Replace), Some("later\n"))], |_| {});
    let v = r.apply(&p, false, Mode::NextBoot).unwrap();
    assert_eq!((v["live"].clone(), v["needs_reboot"].clone()), (false.into(), true.into()));
    assert_eq!(r.read("etc/asound.conf"), "old asound\n");
    assert!(r.live.calls().is_empty());
    assert_eq!(list(&r.env(false))["patches"][0]["status"], "next start");
    // the PID 1 binary: only with the shell scope, stored, never bound
    put(&r.root.join("sbin/init"), "init\n");
    let init = pack(&r, "init-fix", vec![(exec("/sbin/init", Action::Replace), Some("init2\n"))], |_| {});
    assert!(r.apply(&init, false, Mode::Live).unwrap_err().contains("shell scope"));
    let v = r.apply(&init, true, Mode::Live).unwrap();
    assert_eq!((v["live_files"].clone(), v["needs_reboot"].clone()), (0.into(), true.into()));
    assert_eq!(r.read("sbin/init"), "init\n");
    assert!(v["message"].as_str().unwrap().contains("/sbin/init"));
    // the newest patch is first in the order the first stage reads
    assert_eq!(read_active_for_test(&r), ["init-fix", "later"]);
}

#[test]
fn obsolete_and_superseded_patches_are_refused_or_dropped() {
    let r = rig("obsolete");
    // fixed in the version that runs: refused
    let p = pack(&r, "old", vec![(op("/etc/asound.conf", Action::Replace), Some("x\n"))], |m| m.obsolete_from = Some("0.1.0-alpha.2".into()));
    assert!(r.apply(&p, false, Mode::Live).unwrap_err().contains("obsolete"));
    // for another root: refused
    let p = pack(&r, "other", vec![(op("/etc/asound.conf", Action::Replace), Some("x\n"))], |m| m.base = "n9".into());
    assert!(r.apply(&p, false, Mode::Live).unwrap_err().contains("n9"));
    // bound to `current`: written into the layer as the root id
    let p = pack(&r, "mine", vec![(op("/etc/asound.conf", Action::Replace), Some("x\n"))], |m| {
        m.base = "current".into();
        m.obsolete_from = Some("0.1.0-beta.1".into());
    });
    r.apply(&p, false, Mode::NextBoot).unwrap();
    assert!(std::fs::read_to_string(patches_dir(&r.data).join("mine/layer.txt")).unwrap().contains("base=n1"));
    assert!(refresh_obsolete(&r.env(false)).is_empty());
    // a new tree becomes current: the patch is obsolete and leaves `active`
    put(&r.data.join("iyakki/root/current"), "n2\n");
    assert_eq!(refresh_obsolete(&r.env(false)), ["mine"]);
    assert!(read_active_for_test(&r).is_empty());
    let l = list(&r.env(false));
    assert_eq!((l["patches"][0]["status"].as_str(), l["patches"][0]["reason"].as_str().unwrap().contains("n1")), (Some("obsolete"), true));
    // a later version than obsolete_from does the same
    let p = pack(&r, "fixedlater", vec![(op("/etc/asound.conf", Action::Replace), Some("y\n"))], |m| m.obsolete_from = Some("0.1.0-beta.1".into()));
    r.apply(&p, false, Mode::NextBoot).unwrap();
    let newer = Env { release: "VERSION=0.1.0-beta.1\n".into(), ..r.env(false) };
    assert_eq!(refresh_obsolete(&newer), ["fixedlater"]);
}

#[test]
fn the_same_id_again_replaces_the_patch_and_limits_hold() {
    let r = rig("again");
    let p = pack(&r, "fix", vec![(op("/etc/asound.conf", Action::Replace), Some("v1\n"))], |_| {});
    r.apply(&p, false, Mode::Live).unwrap();
    let p2 = pack(&r, "fix", vec![(op("/etc/asound.conf", Action::Replace), Some("v2\n"))], |_| {});
    r.apply(&p2, false, Mode::Live).unwrap();
    assert_eq!(r.read("etc/asound.conf"), "v2\n");
    assert_eq!(read_active_for_test(&r), ["fix"]);
    remove(&r.env(false), "fix", "laptop").unwrap();
    assert_eq!(r.read("etc/asound.conf"), "old asound\n", "the ORIGINAL comes back, not v1");
    // replace of a missing file, add over nothing is fine, a folder is refused
    assert!(plan(&r.env(false), &pack(&r, "m", vec![(op("/usr/bin/nothing", Action::Replace), Some("x"))], |_| {}).manifest).unwrap_err().contains("does not exist"));
    std::fs::create_dir_all(r.root.join("usr/bin/dir")).unwrap();
    assert!(plan(&r.env(false), &pack(&r, "d", vec![(op("/usr/bin/dir", Action::Add), Some("x"))], |_| {}).manifest).unwrap_err().contains("folder"));
    // room on the data volume
    let tight = Env { data_free: &|| 1 << 20, ..r.env(false) };
    assert!(plan(&tight, &p2.manifest).unwrap_err().contains("MB free"));
    // at most MAX_PATCHES active
    std::fs::write(patches_dir(&r.data).join("active"), (0..MAX_PATCHES).map(|i| format!("p{i}\n")).collect::<String>()).unwrap();
    assert!(plan(&r.env(false), &p2.manifest).unwrap_err().contains("active already"));
    // an unsent file is found before anything changes
    let st = staging_dir(&r.data, "fix");
    let _ = std::fs::remove_dir_all(&st);
    std::fs::write(patches_dir(&r.data).join("active"), "").unwrap();
    assert!(apply(&r.env(false), &p2.manifest, Mode::Live, "x").unwrap_err().contains("was not sent"));
    std::fs::create_dir_all(&st).unwrap();
    std::fs::write(st.join(staged_name(0)), "tampered\n").unwrap();
    assert!(apply(&r.env(false), &p2.manifest, Mode::Live, "x").unwrap_err().contains("does not match"));
}

#[test]
fn listing_tells_live_from_in_the_boot_layer_and_boot_ok_cleans_up() {
    let r = rig("boot");
    let d = patches_dir(&r.data);
    let p = pack(&r, "a", vec![(op("/etc/asound.conf", Action::Replace), Some("a\n"))], |_| {});
    r.apply(&p, false, Mode::Live).unwrap();
    let p = pack(&r, "b", vec![(exec("/usr/bin/helper", Action::Replace), Some("b\n"))], |_| {});
    r.apply(&p, false, Mode::Live).unwrap();
    // the system restarts: another boot id, and the first stage wrote `booted` with what it mounted
    let other = FakeLive::default();
    std::fs::write(d.join("booted"), "b\na\n").unwrap();
    std::fs::write(d.join("boot-count"), "1\n").unwrap();
    let env2 = Env { live: &other, ..r.env(false) };
    // the fake always says boot-1; simulate a later start by editing the state's boot id
    for id in ["a", "b"] {
        let s = std::fs::read_to_string(d.join(id).join("state.json")).unwrap().replace("boot-1", "boot-0");
        std::fs::write(d.join(id).join("state.json"), s).unwrap();
    }
    let l = list(&env2);
    assert_eq!(l["patches"][0]["status"], "active");
    assert_eq!(l["guard"]["boot_count"], 1);
    // removing one the boot layer holds: no live undo possible, marked, deleted when a later start no longer has it
    let v = remove(&env2, "a", "laptop").unwrap();
    assert!(v["message"].as_str().unwrap().contains("next start"));
    assert!(d.join("a").exists());
    assert_eq!(list(&env2)["patches"].as_array().unwrap().iter().find(|p| p["id"] == "a").unwrap()["status"], "removing");
    boot_ok(&env2);
    assert!(d.join("a").exists() && !d.join("boot-count").exists(), "still in the boot layer: kept; the counter is cleared");
    std::fs::write(d.join("booted"), "b\n").unwrap();
    boot_ok(&env2);
    assert!(!d.join("a").exists() && d.join("b").exists());
}

#[test]
fn a_full_boot_gets_the_active_layers_copied_over_the_unpacked_root() {
    let r = rig("overlay");
    let p = pack(&r, "k", vec![(exec("/usr/bin/kubectl", Action::Replace), Some(WRAPPER)), (op("/etc/old.conf", Action::Delete), None), (exec("/usr/bin/newtool", Action::Add), Some("t\n"))], |_| {});
    r.apply(&p, true, Mode::NextBoot).unwrap();
    // an unpacked fresh root
    let fresh = r.dir.join("fresh");
    put(&fresh.join("usr/bin/k3s"), "k3s\n");
    std::os::unix::fs::symlink("k3s", fresh.join("usr/bin/kubectl")).unwrap();
    put(&fresh.join("etc/old.conf"), "old\n");
    assert!(overlay_into(&r.data, &fresh).is_empty());
    assert_eq!(std::fs::read_to_string(fresh.join("usr/bin/kubectl")).unwrap(), WRAPPER);
    assert!(std::fs::symlink_metadata(fresh.join("usr/bin/kubectl")).unwrap().is_file());
    assert!(!fresh.join("etc/old.conf").exists());
    assert_eq!(std::fs::read_to_string(fresh.join("usr/bin/newtool")).unwrap(), "t\n");
    // a layer that lost a file is skipped, the root is left as unpacked
    std::fs::remove_file(patches_dir(&r.data).join("k/layer/usr/bin/newtool")).unwrap();
    let fresh2 = r.dir.join("fresh2");
    put(&fresh2.join("etc/old.conf"), "old\n");
    let skipped = overlay_into(&r.data, &fresh2);
    assert_eq!(skipped.len(), 1);
    assert!(fresh2.join("etc/old.conf").exists());
}

#[test]
fn the_routes_need_the_update_scope_the_approval_card_once_and_audit_what_they_do() {
    use super::grants::ShellStatus;
    use super::scopes::Scope;
    use super::tests_adb as t;
    let rg = t::rig("patch-routes");
    let adm = iyakki_sec::policy::Policy::clone(&iyakki_sec::policy::Role::Admin.template());
    let weak = t::key("weak", "root", &[Scope::Install]);
    let strong = t::key("laptop", "root", &[Scope::Install, Scope::Update]);
    std::fs::create_dir_all(rg.dir.join("devmode")).unwrap();
    std::fs::write(rg.dir.join("devmode/keys.json"), serde_json::to_string(&vec![weak.clone(), strong.clone()]).unwrap()).unwrap();
    // a plan for a NEW file in a normal place; nothing is applied by this test (the routes use the real system)
    let mut m = Manifest { format: FORMAT, id: "route".into(), title: "route".into(), description: String::new(), created: String::new(), base: "*".into(), obsolete_from: None, ops: vec![], pre: vec![], post: vec![] };
    m.ops.push(Op { size: 1, sha256: sha256_hex(b"x"), ..op("/usr/bin/iyakki-route-test-not-there", Action::Add) });
    let body = serde_json::to_vec(&m).unwrap();
    let code = |o: Option<super::api::Out>| o.unwrap().status;
    assert_eq!(code(t::call(&rg, &weak, &adm, "POST", "/v1/dev/patch/plan", &[], &body)), 403, "no update scope");
    assert_eq!(code(t::call(&rg, &weak, &adm, "GET", "/v1/dev/patch/list", &[], b"")), 403);
    // the scope alone is not enough: the card first
    let o = t::call(&rg, &strong, &adm, "POST", "/v1/dev/patch/plan", &[], &body).unwrap();
    assert_eq!(o.status, 403);
    assert!(String::from_utf8_lossy(&o.body).contains("not approved"));
    assert_eq!(rg.dm.patch_trusted("laptop"), false);
    let laptop = rg.dm.keys().into_iter().find(|k| k.name == "laptop").unwrap();
    assert_eq!(rg.dm.grant_request(&laptop, "patch", false, 10, "patch route").unwrap(), ShellStatus::Pending);
    assert_eq!(rg.dm.shell_banner().as_deref(), Some("PATCH REQUEST from laptop"));
    rg.dm.shell_approve(&adm, "root", "laptop", 10).unwrap();
    assert!(rg.dm.patch_trusted("laptop"), "remembered after the first approval");
    // now the plan goes through; a plan with a wide path is refused (not the shell scope)
    let o = t::call(&rg, &strong, &adm, "POST", "/v1/dev/patch/plan", &[], &body).unwrap();
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    m.ops[0].path = "/etc/iyakki-route-test".into();
    m.id = "wide".into();
    let o = t::call(&rg, &strong, &adm, "POST", "/v1/dev/patch/plan", &[], &serde_json::to_vec(&m).unwrap()).unwrap();
    assert_eq!(o.status, 409);
    // applying what was never sent is refused and changes nothing
    let o = t::call(&rg, &strong, &adm, "POST", "/v1/dev/patch/apply", &[("id", "nothing-staged")], b"").unwrap();
    assert_eq!(o.status, 409);
    // a user who is not an administrator cannot hold the update scope at all
    assert!(super::scopes::check(&strong, &iyakki_sec::policy::Role::User.template(), Scope::Update).is_err());
    // the activity log has the plan and the refusals
    let acts = rg.dm.activity(20);
    assert!(acts.iter().any(|a| a.action == "dev.patch.plan" && a.ok) && acts.iter().any(|a| a.action == "dev.patch.plan" && !a.ok));
    // revoking the computer forgets the approval, so a new computer with the same name is asked again
    rg.dm.revoke(&adm, "root", "laptop").unwrap();
    assert!(!rg.dm.patch_trusted("laptop"));
}

#[test]
fn the_running_system_marks_a_start_good_with_the_real_entry_point() {
    let r = rig("good");
    let d = patches_dir(&r.data);
    put(&d.join("boot-count"), "2\n");
    put(&d.join("booted"), "");
    mark_boot_good(&r.data);
    assert!(!d.join("boot-count").exists());
    // no patches folder: nothing is created
    let none = rig("good2");
    std::fs::remove_dir_all(patches_dir(&none.data)).ok();
    mark_boot_good(&none.data);
    assert!(!patches_dir(&none.data).exists());
}

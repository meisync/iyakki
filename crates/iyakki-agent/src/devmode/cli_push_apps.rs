//! `iyakki dev push-apps` (docs/development/UI-BOUNDARY.md, section 7): ship ONE program file (the one that holds the apps) to a paired device as a hot patch,
//! as a binary delta against the program that is there, without an image build and without the layers rebuild.
//!
//! Only the computer's side is here. The device decides everything: its scope check, its approval card (the person at the device answers it; this side only
//! waits) and where the file goes. Nothing here reboots or restarts anything. While the shell is a thread of the core program (`/usr/bin/iyakki`) the patch
//! is stored for the next start and the output says so; a separate `/usr/bin/iyakki-ui` is applied live (the core will restart the UI program once it exists).

use std::path::{Path, PathBuf};

use super::cli::Args;
use super::cli_patch::apply_pack;
use super::client::{DevClient, Home};
use super::patch::{self, Action, Kind, Manifest, Op};

pub const USAGE: &str = "usage: iyakki dev push-apps --bin FILE [--target /usr/bin/iyakki] [--against OLDFILE] [--add] [--shell] [--layer live-ui|live-core|reboot]";
/// The program that holds the shell and the apps today.
pub const CORE_PROGRAM: &str = "/usr/bin/iyakki";
/// The id of the patch: pushing again replaces the earlier one instead of piling up patches.
pub const PATCH_ID: &str = "push-apps";

/// What a push is going to do, before anything is sent.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub target: String,
    pub mode: &'static str,
    pub delta: bool,
    pub bytes: usize,
    pub new_size: u64,
}

/// `live` for a separate UI program, `next-boot` for the core program (a running program keeps its inode, and the shell is a thread of it).
pub fn mode_for(target: &str) -> &'static str {
    if target.rsplit('/').next() == Some("iyakki-ui") {
        "live"
    } else {
        "next-boot"
    }
}

/// What the layer the change touched means for applying it (the fast loop passes `--layer`). `reboot` (kernel, modules, boot files, image recipe) is not a
/// program push at all: it refuses and names the staged update. The live words say what is true today, not what is planned.
pub fn layer_note(layer: &str) -> Result<&'static str, String> {
    match layer {
        "live-ui" | "live-core" => Ok("This change is live-capable (no device restart): live apply of the core program is off by default until it has been tried on a real device (docs/development/LIVE-RESTART.md); until then the stored program runs at the next start."),
        "reboot" => Err("The device layer changed (kernel, modules, boot files, image recipe): a program push cannot carry it. Build an update pack (dist/buildroot/quick-push.sh or nightly-push.sh), `iyakki dev update`, then `iyakki dev reboot`.".into()),
        other => Err(format!("unknown --layer {other}: live-ui, live-core or reboot")),
    }
}

/// Build the pack for one program file. `against` is the program the device runs now (a delta is made against it); without it the whole file goes, which the
/// patch limits allow only for a small file.
pub fn make_pack(bin: &Path, target: &str, against: Option<&Path>, add: bool, work: &Path) -> Result<(Vec<u8>, Plan), String> {
    patch::path_reach(target)?;
    let meta = std::fs::metadata(bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(format!("{} is not a program file", bin.display()));
    }
    let dest = work.join("files").join(target.trim_start_matches('/'));
    std::fs::create_dir_all(dest.parent().ok_or("bad target")?).map_err(|e| e.to_string())?;
    std::fs::copy(bin, &dest).map_err(|e| format!("{}: {e}", bin.display()))?;
    let m = Manifest {
        format: patch::FORMAT,
        id: PATCH_ID.into(),
        title: "Apps program (iyakki dev push-apps)".into(),
        description: "One program file pushed as a delta by iyakki dev push-apps.".into(),
        created: String::new(),
        base: "*".into(),
        obsolete_from: None,
        ops: vec![Op { path: target.into(), action: if add { Action::Add } else { Action::Replace }, kind: Kind::File, mode: Some("0755".into()), uid: 0, gid: 0, size: 0, sha256: String::new(), link: None, delta: None }],
        pre: vec![],
        post: vec![],
    };
    std::fs::write(work.join(patch::MANIFEST), serde_json::to_vec(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let bytes = patch::build_with(work, against).map_err(|e| {
        if against.is_none() {
            format!("{e}\nThe first push needs the program that is on the device (--against FILE, the `iyakki` of the image that is installed); later pushes remember the last one.")
        } else {
            e
        }
    })?;
    let p = patch::decode(&bytes)?;
    let o = &p.manifest.ops[0];
    let plan = Plan { target: target.into(), mode: mode_for(target), delta: o.delta.is_some(), bytes: bytes.len(), new_size: o.delta.as_ref().map_or(o.size, |d| d.new_size) };
    Ok((bytes, plan))
}

fn last_path(home: &Home, target: &str) -> PathBuf {
    home.dir().join("push-apps").join(format!("{}.last", target.rsplit('/').next().unwrap_or("program")))
}

/// Returns `Some(result)` when `cmd` is `push-apps`.
pub(super) fn run(cmd: &str, args: &Args, home: &Home, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    (cmd == "push-apps").then(|| push(args, home, connect, out))
}

fn push(args: &Args, home: &Home, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let bin = PathBuf::from(args.get("--bin").ok_or_else(|| USAGE.to_string())?);
    if let Some(l) = args.get("--layer") {
        out(layer_note(l)?);
    }
    let target = args.get("--target").unwrap_or(CORE_PROGRAM).to_string();
    let last = last_path(home, &target);
    let against = args.get("--against").map(PathBuf::from).or_else(|| last.is_file().then(|| last.clone()));
    let work = std::env::temp_dir().join(format!("iyakki-push-apps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let made = make_pack(&bin, &target, against.as_deref(), args.has("--add"), &work);
    let _ = std::fs::remove_dir_all(&work);
    let (bytes, plan) = made?;
    out(&format!("{}: {} KB {} for a program of {} KB", plan.target, bytes.len().div_ceil(1024), if plan.delta { "as a delta" } else { "whole" }, plan.new_size.div_ceil(1024)));
    let pack = patch::decode(&bytes)?;
    let c = connect()?;
    out("Sending. The approval card is on the device's screen: the person there answers it.");
    apply_pack(&c, &pack, plan.mode, args.has("--shell"), out).map_err(|e| if e.contains("delta") || e.contains("sha") { format!("{e}\nThe device holds a different program than the one this delta is against: give the program that is on it with --against FILE.") } else { e })?;
    if let Some(d) = last.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::copy(&bin, &last);
    if plan.mode == "next-boot" {
        out("Stored for the next start: the shell is a thread of the core program, so the device must restart to run it (iyakki dev reboot; it is not done for you).");
    } else {
        out("Applied live. The core restarts the UI program.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-push-apps-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn program(seed: u8, patch_at: Option<usize>) -> Vec<u8> {
        let mut v: Vec<u8> = (0..400_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8 ^ seed).collect();
        if let Some(p) = patch_at {
            for b in &mut v[p..p + 64] {
                *b ^= 0xff;
            }
        }
        v
    }

    #[test]
    fn a_small_edit_ships_as_a_small_delta_against_the_old_program() {
        let d = tmp("delta");
        let (old, new) = (d.join("old"), d.join("new"));
        std::fs::write(&old, program(1, None)).unwrap();
        std::fs::write(&new, program(1, Some(200_000))).unwrap();
        let (bytes, plan) = make_pack(&new, CORE_PROGRAM, Some(&old), false, &d.join("w")).unwrap();
        assert!(plan.delta && plan.bytes < 20_000, "the pack was {} bytes", plan.bytes);
        assert_eq!(plan.new_size, 400_000);
        assert_eq!(plan.mode, "next-boot");
        let p = patch::decode(&bytes).unwrap();
        assert_eq!(p.manifest.id, PATCH_ID);
        assert_eq!(p.manifest.ops[0].mode.as_deref(), Some("0755"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn without_an_old_program_a_small_file_goes_whole_and_the_ui_program_is_applied_live() {
        let d = tmp("whole");
        let new = d.join("new");
        std::fs::write(&new, program(2, None)).unwrap();
        let (_, plan) = make_pack(&new, "/usr/bin/iyakki-ui", None, true, &d.join("w")).unwrap();
        assert!(!plan.delta);
        assert_eq!(plan.mode, "live");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_layer_decides_before_anything_is_sent() {
        assert!(layer_note("live-ui").is_ok() && layer_note("live-core").unwrap().contains("off by default"));
        assert!(layer_note("reboot").unwrap_err().contains("iyakki dev update"));
        assert!(layer_note("kernel").is_err());
    }

    #[test]
    fn a_missing_file_and_a_target_a_patch_may_never_touch_are_refused_before_anything_is_built() {
        let d = tmp("refuse");
        assert!(make_pack(&d.join("nothing"), CORE_PROGRAM, None, false, &d.join("w")).is_err());
        let f = d.join("f");
        std::fs::write(&f, b"x").unwrap();
        assert!(make_pack(&f, "/boot/iyakki", None, false, &d.join("w")).is_err());
        assert!(make_pack(&f, "usr/bin/iyakki", None, false, &d.join("w")).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    /// Timing of the pack step on a program-sized file (synthetic bytes, an insertion that shifts everything after it plus a few changed spans). Run by hand:
    /// `cargo test -p iyakki-agent --lib cli_push_apps -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_the_pack_of_a_forty_megabyte_program() {
        let d = tmp("measure");
        let n = 40usize << 20;
        let old: Vec<u8> = (0..n as u64).map(|i| ((i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 29) as u8) ^ ((i >> 12) as u8)).collect();
        let mut new = old.clone();
        new.splice(20 << 20..20 << 20, [7u8; 48]);
        for k in 0..40 {
            let at = (k * 997_003) % (n - 100);
            new[at] ^= 0x55;
        }
        std::fs::write(d.join("old"), &old).unwrap();
        std::fs::write(d.join("new"), &new).unwrap();
        let t = std::time::Instant::now();
        let (_, plan) = make_pack(&d.join("new"), CORE_PROGRAM, Some(&d.join("old")), false, &d.join("w")).unwrap();
        println!("MEASURE pack of a {} MB program: {} KB as {} in {} ms (dev test profile)", plan.new_size >> 20, plan.bytes >> 10, if plan.delta { "delta" } else { "whole" }, t.elapsed().as_millis());
        let _ = std::fs::remove_dir_all(d);
    }
}

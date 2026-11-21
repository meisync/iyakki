//! Chat tools of the Install page that make room for IYAKKI by shrinking another system's partition (F-INST-30; the command line is
//! `iyakki-usb --shrink`, F-INST-27). Two tools:
//!
//! * `install_shrink_plan` (free, administrators): what would be shrunk, to what size, and what the tool will do, in the installer's own
//!   plan text; nothing is changed. It asks the file system's own tool for the least size it needs.
//! * `install_shrink` (administrators; asks EVERY TIME, a hard ask that no standing allow and no Auto mode covers): the card shows that
//!   same plan; after the person approves it, the call must repeat the drive's name as the plan shows it (`confirm`), the same second
//!   deliberate step as the erase pages, and only then the installer's shrink runs (check, shrink the file system, check again, then the
//!   table entry). A mounted or swap partition, the running system and the boot stick are refused by the plan.
//!
//! Both go through the core's disk tools (`iyakki_core_api::installer::DiskTools`); the chat has no other way to touch a disk.
//!
//! Two more tools set up legacy BIOS boot on a shared disk (F-INST-13, and F-INST-29 for disks of 4096-byte sectors, where our own
//! boot code `builtin` is used): `install_bios_plan` (free, administrators) shows the whole plan of `--mode free-space --bios` and
//! writes nothing; `install_bios` (administrators; a hard ask with the plan on the card, then the drive's name as `confirm`) writes it
//! through the installer's writer, which saves sector 0 and the core area to a file first so `--bios-restore` can undo it.

use std::sync::Arc;

use iyakki_core_api::installer::{DiskTools, FsTool, Located, ShrinkPlan};
use iyakki_core_local::LocalDiskTools;
use serde_json::json;

use crate::settingstools::{arg_str, Entry, Env};

pub const TOOLS: [&str; 4] = ["install_shrink_plan", "install_shrink", "install_bios_plan", "install_bios"];

/// What the BIOS tools need from the installer: plan, and write. Tests use a fake; the real one is [`RealBios`].
pub trait BiosInstaller: Send + Sync {
    /// The plan text and the drive's name for the given installer arguments. Writes nothing.
    fn plan(&self, args: &[String]) -> Result<(String, String), String>;
    /// Plan again from a fresh read of the disk and write; `confirm` is the drive's name.
    fn write(&self, args: &[String], confirm: &str) -> Result<String, String>;
}

/// The real one: the core's disk tools on this computer (`iyakki_core_local::LocalDiskTools`).
pub struct RealBios;

impl BiosInstaller for RealBios {
    fn plan(&self, args: &[String]) -> Result<(String, String), String> {
        LocalDiskTools::system().bios_plan(args)
    }
    fn write(&self, args: &[String], confirm: &str) -> Result<String, String> {
        LocalDiskTools::system().bios_write(args, confirm)
    }
}

/// The installer's arguments for a BIOS install on `disk`: free-space mode, `--bios`, the save file in the data folder, and our own
/// 4096-byte-sector boot code (used only when the disk's sectors are 4096 bytes).
pub fn bios_args(dir: &std::path::Path, a: &serde_json::Value) -> Result<Vec<String>, String> {
    let disk = arg_str(a, "disk")?;
    if disk.is_empty() || !disk.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("`disk` is a drive name such as sda or nvme0n1".into());
    }
    let mut v: Vec<String> = ["--plan", "--disk", disk.as_str(), "--mode", "free-space", "--bios", "--bios-4k-boot-code", "builtin", "--no-boot-entry"].iter().map(|s| s.to_string()).collect();
    if let Some(g) = a.get("gap_start").and_then(|g| g.as_u64()) {
        v.extend(["--gap-start".to_string(), g.to_string()]);
    }
    if a.get("replace_boot_code").and_then(|b| b.as_bool()) == Some(true) {
        v.push("--bios-replace-boot-code".into());
    }
    let save = dir.join("bios-saves").join(format!("{disk}-sector0.bin"));
    v.extend(["--bios-save".to_string(), save.display().to_string()]);
    Ok(v)
}

/// Where the disk comes from (the real machine, or an image file in tests).
pub type Source = Arc<dyn Fn(&str, u32) -> Result<Located, String> + Send + Sync>;

pub fn real_source() -> Source {
    Arc::new(|disk, part| LocalDiskTools::system().locate(disk, part))
}

fn args(a: &serde_json::Value) -> Result<(String, u32, u64), String> {
    let disk = arg_str(a, "disk")?;
    let part = a.get("part").and_then(|v| v.as_u64()).filter(|p| *p >= 1 && *p < 1000).ok_or("say `part`: the partition number, such as 2")? as u32;
    let to = a.get("to_mib").and_then(|v| v.as_u64()).filter(|m| *m >= 1).ok_or("say `to_mib`: the new size in MiB")?;
    Ok((disk, part, to))
}

fn plan_of(src: &Source, tool: &dyn FsTool, a: &serde_json::Value) -> Result<(Located, ShrinkPlan), String> {
    let (disk, part, to) = args(a)?;
    let l = src(&disk, part)?;
    let p = l.plan(part, to, tool)?;
    Ok((l, p))
}

pub fn bios_entries(env: &Env, inst: Arc<dyn BiosInstaller>) -> Vec<Entry> {
    let mut v = Vec::new();
    let schema = json!({"type": "object", "properties": {"disk": {"type": "string", "description": "the disk, such as sda"}, "gap_start": {"type": "integer", "description": "the first sector of the free space to use (default: the largest)"}, "replace_boot_code": {"type": "boolean", "description": "allow replacing another system's boot code in sector 0 (it is saved first)"}}, "required": ["disk"]});
    {
        let (inst, dir) = (inst.clone(), env.dir.clone());
        v.push(
            Entry::read("Install", "install_bios_plan", "Show the plan to install IYAKKI in free space with legacy BIOS boot (also on disks of 4096-byte sectors, with our own boot code). Writes nothing.", schema.clone(), move |a| {
                let (text, name) = inst.plan(&bios_args(&dir, a)?)?;
                Ok(format!("{text}\nNothing was written. To do it, call install_bios with the same values and confirm: \"{name}\"."))
            })
            .admin(),
        );
    }
    {
        let (i1, i2, i3, d1, d2, d3, e) = (inst.clone(), inst.clone(), inst, env.dir.clone(), env.dir.clone(), env.dir.clone(), env.clone());
        let mut schema = schema;
        schema["properties"]["confirm"] = json!({"type": "string", "description": "the drive's name exactly as install_bios_plan shows it"});
        schema["required"] = json!(["disk", "confirm"]);
        v.push(
            Entry::change("Install", "install_bios", "Install IYAKKI in free space with legacy BIOS boot (4096-byte-sector disks use our own boot code). Sector 0 and the core area are saved first (undo: iyakki-usb --bios-restore). The person approves a card with the plan every time; `confirm` must be the drive's name.", schema, |a| format!("Install IYAKKI with legacy BIOS boot on {}", a["disk"].as_str().unwrap_or("a disk")), move |a| {
                let args = bios_args(&d1, a)?;
                let confirm = arg_str(a, "confirm")?;
                let (_, name) = i1.plan(&args)?;
                if confirm != name {
                    return Err(format!("confirm must be the drive's name as the plan shows it, \"{name}\"; nothing was written"));
                }
                e.audit("install.bios", &format!("{name}: started"));
                let r = i1.write(&args, &confirm);
                e.audit("install.bios", &format!("{name}: {}", if r.is_ok() { "done" } else { "failed" }));
                Ok(format!("{}\nDone. The saved sector 0 is in the data folder (bios-saves); `iyakki-usb --bios-restore` puts it back.", r?))
            })
            .with_preview(move |a| match bios_args(&d2, a).and_then(|x| i2.plan(&x)) {
                Ok((t, _)) => format!("{t}This writes to the disk, including its first sector. Back up first."),
                Err(e) => format!("This cannot be done: {e}"),
            })
            .with_refusal(move |a| bios_args(&d3, a).and_then(|x| i3.plan(&x)).err())
            .admin(),
        );
    }
    v
}

pub fn entries(env: &Env, src: Source, tool: Arc<dyn FsTool + Send + Sync>) -> Vec<Entry> {
    let mut v = bios_entries(env, Arc::new(RealBios));
    {
        let (src, tool) = (src.clone(), tool.clone());
        let schema = json!({"type": "object", "properties": {"disk": {"type": "string", "description": "the disk, such as sda"}, "part": {"type": "integer"}, "to_mib": {"type": "integer", "description": "the new size of the partition in MiB"}}, "required": ["disk", "part", "to_mib"]});
        v.push(
            Entry::read("Install", "install_shrink_plan", "Show what shrinking a partition of another system would do (the new size, every step, the warnings) to make free space for IYAKKI. Changes nothing.", schema, move |a| {
                let (_, p) = plan_of(&src, &*tool, a)?;
                Ok(format!("{}Nothing was changed. To do it, call install_shrink with the same values and confirm: \"{}\".", p.text(), p.disk_name))
            })
            .admin(),
        );
    }
    {
        let (src1, tool1, src2, tool2, src3, tool3, e) = (src.clone(), tool.clone(), src.clone(), tool.clone(), src, tool, env.clone());
        let schema = json!({"type": "object", "properties": {"disk": {"type": "string"}, "part": {"type": "integer"}, "to_mib": {"type": "integer"}, "confirm": {"type": "string", "description": "the drive's name exactly as install_shrink_plan shows it"}}, "required": ["disk", "part", "to_mib", "confirm"]});
        v.push(
            Entry::change("Install", "install_shrink", "Shrink a partition of another system to make free space for IYAKKI (ext2/3/4, NTFS, FAT32 or exFAT down to its last file). The person approves a card with the plan every time; `confirm` must be the drive's name. Back up first: a fault or a power cut can damage the file system.", schema, |a| format!("Shrink partition {} of {} to {} MiB", a["part"], a["disk"].as_str().unwrap_or("a disk"), a["to_mib"]), move |a| {
                let (l, p) = plan_of(&src1, &*tool1, a)?;
                let confirm = arg_str(a, "confirm")?;
                if confirm != p.disk_name {
                    return Err(format!("confirm must be the drive's name as the plan shows it, \"{}\"; nothing was changed", p.disk_name));
                }
                e.audit("install.shrink", &format!("{} partition {} to {} MiB: started", p.disk_name, p.part, p.new_bytes() / (1024 * 1024)));
                let r = l.apply(&p, &confirm, &*tool1);
                e.audit("install.shrink", &format!("{} partition {}: {}", p.disk_name, p.part, if r.is_ok() { "done" } else { "failed" }));
                let r = r?;
                Ok(format!("{}\nDone: {} MiB are free space now. The installer's free-space mode can use them.", r.notes.join("\n"), r.freed_bytes / (1024 * 1024)))
            })
            .with_preview(move |a| match plan_of(&src2, &*tool2, a) {
                Ok((_, p)) => format!("{}This changes another system's partition. Back up first.", p.text()),
                Err(e) => format!("This cannot be done: {e}"),
            })
            .with_refusal(move |a| plan_of(&src3, &*tool3, a).err())
            .admin(),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settingstools::Env;
    use crate::agent::Risk;
    use iyakki_core_api::installer::HostTools;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const MB: u64 = 1024 * 1024;

    fn have(t: &str) -> bool {
        Command::new("sh").args(["-c", &format!("command -v {t} >/dev/null")]).status().is_ok_and(|s| s.success())
    }
    fn run(cmd: &str, args: &[&str]) {
        let o = Command::new(cmd).args(args).output().unwrap_or_else(|e| panic!("{cmd}: {e}"));
        assert!(o.status.success(), "{cmd} {args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }

    /// A 300 MiB disk image with a GPT, a FAT partition and an ext4 partition holding a known file, made by the real tools.
    fn disk(tag: &str) -> Option<(PathBuf, PathBuf)> {
        for t in ["sgdisk", "mke2fs", "e2fsck", "resize2fs", "truncate", "dd"] {
            if !have(t) {
                eprintln!("skipped: {t} is not installed");
                return None;
            }
        }
        let d = crate::testdir::unique("iyakki-shrinktool", tag);
        let img = d.join("disk.img");
        let s = img.to_str().unwrap();
        run("truncate", &["-s", &(300 * MB).to_string(), s]);
        run("sgdisk", &["-o", s]);
        run("sgdisk", &["-n", "1:0:+200M", "-t", "1:8300", s]);
        let fs = d.join("p1.img");
        run("truncate", &["-s", &(200 * MB).to_string(), fs.to_str().unwrap()]);
        run("mke2fs", &["-q", "-t", "ext4", "-L", "OTHER", fs.to_str().unwrap()]);
        // the partition starts at sector 2048
        run("dd", &[&format!("if={}", fs.display()), &format!("of={}", img.display()), "bs=512", "seek=2048", "conv=notrunc", "status=none"]);
        Some((d, img))
    }

    fn env(dir: &Path, log: Arc<std::sync::Mutex<Vec<String>>>) -> Env {
        Env { dir: dir.to_path_buf(), user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&Arc::new(std::sync::Mutex::new(crate::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()))), audit: Arc::new(move |a, d| log.lock().unwrap().push(format!("{a}: {d}"))) }
    }

    fn call(es: &[Entry], name: &str, a: serde_json::Value) -> Result<String, String> {
        (es.iter().find(|e| e.name == name).unwrap().call)(&a)
    }

    #[test]
    fn the_plan_tool_changes_nothing_and_the_shrink_tool_needs_the_drive_name_then_shrinks_with_the_real_tools() {
        let Some((d, img)) = disk("a") else { return };
        let log = Arc::new(std::sync::Mutex::new(vec![]));
        let img2 = img.clone();
        let src: Source = Arc::new(move |_, _| Located::of_file(&img2));
        let es = entries(&env(&d, log.clone()), src, Arc::new(HostTools));
        assert!(es.iter().all(|e| e.admin_only));
        assert_eq!(es.iter().find(|e| e.name == "install_shrink").unwrap().risk, Risk::Ask);
        assert_eq!(es.iter().find(|e| e.name == "install_shrink_plan").unwrap().risk, Risk::Safe);
        let before = std::fs::read(&img).unwrap();
        let a = json!({"disk": "testfile", "part": 1, "to_mib": 120});
        let text = call(&es, "install_shrink_plan", a.clone()).unwrap();
        assert!(text.contains("Shrink") && text.contains("Nothing was changed") && text.contains("testfile"), "{text}");
        assert_eq!(std::fs::read(&img).unwrap(), before, "the plan changes nothing");
        // the card shows the same plan
        let preview = (es.iter().find(|e| e.name == "install_shrink").unwrap().preview.as_ref().unwrap())(&a);
        assert!(preview.contains("Shrink") && preview.contains("Back up first"), "{preview}");
        // a wrong or missing name changes nothing
        let mut bad = a.clone();
        bad["confirm"] = json!("sdb");
        assert!(call(&es, "install_shrink", bad).unwrap_err().contains("nothing was changed"));
        assert!(call(&es, "install_shrink", a.clone()).is_err());
        assert_eq!(std::fs::read(&img).unwrap(), before);
        // a size below what the file system needs, and a partition that is not there, are refused in words
        assert!(call(&es, "install_shrink_plan", json!({"disk": "testfile", "part": 1, "to_mib": 1})).is_err());
        assert!(call(&es, "install_shrink_plan", json!({"disk": "testfile", "part": 7, "to_mib": 100})).unwrap_err().contains("7"));
        // the real shrink
        let mut ok = a.clone();
        ok["confirm"] = json!("testfile");
        let out = call(&es, "install_shrink", ok).unwrap();
        assert!(out.contains("Done"), "{out}");
        let l = Located::of_file(&img).unwrap();
        assert_eq!(l.model.part(1).unwrap().bytes(512), 120 * MB);
        let lines = log.lock().unwrap().join("\n");
        assert!(lines.contains("started") && lines.contains("done"), "{lines}");
        let _ = std::fs::remove_dir_all(&d);
    }

    struct FakeBios {
        log: std::sync::Mutex<Vec<String>>,
    }
    impl BiosInstaller for FakeBios {
        fn plan(&self, args: &[String]) -> Result<(String, String), String> {
            self.log.lock().unwrap().push(format!("plan {}", args.join(" ")));
            if args.iter().any(|a| a == "nvme9") {
                return Err("legacy BIOS boot cannot be set up on this disk of 4096-byte sectors".into());
            }
            Ok(("PLAN\n".into(), args[2].clone()))
        }
        fn write(&self, args: &[String], confirm: &str) -> Result<String, String> {
            self.log.lock().unwrap().push(format!("write {confirm} {}", args.join(" ")));
            Ok("written".into())
        }
    }

    #[test]
    fn the_bios_tools_plan_freely_and_write_only_after_the_drive_name_with_our_own_4k_code() {
        let d = crate::testdir::unique("iyakki-biostool", "a");
        let audit = Arc::new(std::sync::Mutex::new(vec![]));
        let fake = Arc::new(FakeBios { log: Default::default() });
        let es = bios_entries(&env(&d, audit.clone()), fake.clone());
        assert!(es.iter().all(|e| e.admin_only && e.tab == "Install"));
        assert_eq!(es.iter().find(|e| e.name == "install_bios").unwrap().risk, Risk::Ask);
        assert_eq!(es.iter().find(|e| e.name == "install_bios_plan").unwrap().risk, Risk::Safe);
        let a = json!({"disk": "sdb", "replace_boot_code": true});
        let t = call(&es, "install_bios_plan", a.clone()).unwrap();
        assert!(t.contains("PLAN") && t.contains("Nothing was written") && t.contains("\"sdb\""), "{t}");
        let l = fake.log.lock().unwrap().join("\n");
        assert!(l.contains("--bios ") && l.contains("--bios-4k-boot-code builtin") && l.contains("--bios-replace-boot-code") && l.contains("bios-saves"), "{l}");
        assert!(!l.contains("write"), "{l}");
        // no name, a wrong name, a refusal of the planner and a bad disk name write nothing
        assert!(call(&es, "install_bios", a.clone()).is_err());
        let mut bad = a.clone();
        bad["confirm"] = json!("sda");
        assert!(call(&es, "install_bios", bad).unwrap_err().contains("nothing was written"));
        let mut k = json!({"disk": "nvme9", "confirm": "nvme9"});
        assert!(call(&es, "install_bios", k.clone()).unwrap_err().contains("4096"));
        k["disk"] = json!("../sda");
        assert!(call(&es, "install_bios", k).is_err());
        assert!(!fake.log.lock().unwrap().iter().any(|l| l.starts_with("write")));
        let mut ok = a;
        ok["confirm"] = json!("sdb");
        let out = call(&es, "install_bios", ok).unwrap();
        assert!(out.contains("written") && out.contains("bios-restore"), "{out}");
        assert!(fake.log.lock().unwrap().iter().any(|l| l.starts_with("write sdb ")));
        let lines = audit.lock().unwrap().join("\n");
        assert!(lines.contains("started") && lines.contains("done"), "{lines}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_tools_are_listed_for_administrators_only() {
        let log = Arc::new(std::sync::Mutex::new(vec![]));
        let d = crate::testdir::unique("iyakki-shrinktool", "b");
        let src: Source = Arc::new(|_, _| Err("no disk".into()));
        let es = entries(&env(&d, log), src, Arc::new(HostTools));
        for n in TOOLS {
            assert!(es.iter().any(|e| e.name == n && e.admin_only && e.tab == "Install"), "{n}");
        }
        assert!(call(&es, "install_shrink_plan", json!({"disk": "sda", "part": 1, "to_mib": 10})).unwrap_err().contains("no disk"));
    }
}

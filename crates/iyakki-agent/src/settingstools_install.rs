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
//! Both go through the installer's own code (`iyakki_usb::resize`); the chat has no other way to touch a disk.

use std::sync::Arc;

use iyakki_usb::resize::{FsTool, Located, ShrinkPlan};
use serde_json::json;

use crate::settingstools::{arg_str, Entry, Env};

pub const TOOLS: [&str; 2] = ["install_shrink_plan", "install_shrink"];

/// Where the disk comes from (the real machine, or an image file in tests).
pub type Source = Arc<dyn Fn(&str, u32) -> Result<Located, String> + Send + Sync>;

pub fn real_source() -> Source {
    Arc::new(|disk, part| Located::of_machine(&iyakki_usb::plan_cli::Machine::real(), disk, part))
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

pub fn entries(env: &Env, src: Source, tool: Arc<dyn FsTool + Send + Sync>) -> Vec<Entry> {
    let mut v = Vec::new();
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
            Entry::change("Install", "install_shrink", "Shrink a partition of another system to make free space for IYAKKI (ext2/3/4 or NTFS). The person approves a card with the plan every time; `confirm` must be the drive's name. Back up first: a fault or a power cut can damage the file system.", schema, |a| format!("Shrink partition {} of {} to {} MiB", a["part"], a["disk"].as_str().unwrap_or("a disk"), a["to_mib"]), move |a| {
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
    use iyakki_usb::resize::HostTools;
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
        Env { dir: dir.to_path_buf(), user: "root".into(), admin: true, settings: Arc::new(std::sync::Mutex::new(crate::settings::SettingsStore::open(&dir.join("settings.json")).unwrap())), audit: Arc::new(move |a, d| log.lock().unwrap().push(format!("{a}: {d}"))) }
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

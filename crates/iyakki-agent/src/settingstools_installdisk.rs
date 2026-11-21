//! Chat tools of the Install page that write IYAKKI to a whole disk (F-ARCH-05; the page is `iyakki-apps` `settings_install`, the work is the
//! core's [`Installer`](iyakki_core_api::installer::Installer)). Two tools:
//!
//! * `install_disk_plan` (free, administrators): without `disk` it lists the disks of this computer with what is on each and says whether
//!   anything can be installed from; with `disk` it gives the installer's own plan text for that disk (what will be erased, the partition
//!   table, every step, the warnings) and the two words the install needs. Nothing is changed.
//! * `install_to_disk` (administrators; asks EVERY TIME, a hard ask that no standing allow and no Auto mode covers): the card shows that
//!   same plan; after the person approves it, the call must repeat the drive's name (`confirm_name`) and its size as the plan shows it
//!   (`confirm_size`), the two typed confirmations of the Install page (`confirmation_ok`), and only then the installer writes the disk.
//!   The disk list is read again at that moment, so a disk that was mounted since is refused by the plan, and so are the running system and
//!   the disk IYAKKI is copied from.
//!
//! Both go through the `Installer` trait (the real one over `/sys` and `/dev`, a fake in tests): the chat has no other way to write a disk.

use std::sync::Arc;

use iyakki_core_api::installer::{confirmation_ok, make_plan_with, Choices, Installer, Phase, Plan, TableChoice};
use serde_json::json;

use crate::settingstools::{arg_str, Entry, Env};

pub const TOOLS: [&str; 2] = ["install_disk_plan", "install_to_disk"];

/// The installer of this machine.
pub fn real_installer() -> Arc<dyn Installer> {
    Arc::new(iyakki_core_local::LocalInstaller::system())
}

fn human(bytes: u64) -> String {
    iyakki_core_local::usb::ui::human_size(bytes)
}

/// The plan for `disk` over what the installer sees now.
fn plan_of(inst: &dyn Installer, disk: &str) -> Result<Plan, String> {
    let disks = inst.disks();
    let source = inst.source(&disks)?;
    make_plan_with(&disks, disk, &source, &Choices::default(), TableChoice::Auto, inst.uefi())
}

fn list_text(inst: &dyn Installer) -> String {
    let disks = inst.disks();
    let mut out = match inst.source(&disks) {
        Ok(_) => String::from("IYAKKI can be installed from here.\n"),
        Err(e) => format!("Nothing can be installed from here: {e}\n"),
    };
    if disks.is_empty() {
        out.push_str("No disk was found.");
        return out;
    }
    for d in &disks {
        let r = &d.drive;
        let what = if d.partitions.is_empty() { "empty or unreadable".to_string() } else { d.partitions.iter().map(|(n, b, w)| format!("{n} {} {w}", human(*b))).collect::<Vec<_>>().join(", ") };
        let mounted = if r.mounts.is_empty() { String::new() } else { format!("; mounted: {}", r.mounts.iter().map(|(_, m)| m.as_str()).collect::<Vec<_>>().join(", ")) };
        out.push_str(&format!("{} ({}, {}{}){}: {}\n", r.name, human(r.size_bytes), r.model, if r.removable { ", removable" } else { "" }, mounted, what));
    }
    out.push_str("To see what installing on one would do, call install_disk_plan with `disk`.");
    out
}

pub fn entries(env: &Env, inst: Arc<dyn Installer>) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let inst = inst.clone();
        let schema = json!({"type": "object", "properties": {"disk": {"type": "string", "description": "the disk, such as sda or nvme0n1; leave out to list the disks"}}});
        v.push(
            Entry::read("Install", "install_disk_plan", "List the disks of this computer and what is on each, or (with `disk`) show what installing IYAKKI on that disk would do: it erases the whole disk. Changes nothing.", schema, move |a| {
                let disk = a.get("disk").and_then(|d| d.as_str()).map(str::trim).filter(|d| !d.is_empty());
                let Some(disk) = disk else { return Ok(list_text(&*inst)) };
                let p = plan_of(&*inst, disk)?;
                let d = &p.target.drive;
                Ok(format!("{}\nNothing was changed. To do it, call install_to_disk with the same disk, confirm_name \"{}\" and confirm_size \"{}\".", p.render(), d.name, human(d.size_bytes)))
            })
            .admin(),
        );
    }
    {
        let (i1, i2, i3, e) = (inst.clone(), inst.clone(), inst, env.clone());
        let schema = json!({"type": "object", "properties": {"disk": {"type": "string"}, "confirm_name": {"type": "string", "description": "the drive's name exactly as install_disk_plan shows it"}, "confirm_size": {"type": "string", "description": "its size as install_disk_plan shows it, such as 238.5 GB"}}, "required": ["disk", "confirm_name", "confirm_size"]});
        v.push(
            Entry::change("Install", "install_to_disk", "Install IYAKKI on a whole disk, ERASING everything on it. The person approves a card with the plan every time; `confirm_name` and `confirm_size` must be the drive's name and size. Back up first.", schema, |a| format!("Install IYAKKI on {}, erasing it", a["disk"].as_str().unwrap_or("a disk")), move |a| {
                let disk = arg_str(a, "disk")?;
                let (name, size) = (arg_str(a, "confirm_name")?, arg_str(a, "confirm_size")?);
                let plan = plan_of(&*i1, &disk)?;
                if !confirmation_ok(&name, &size, &plan.target.drive) {
                    return Err(format!("confirm_name must be the drive's name \"{}\" and confirm_size its size \"{}\", as the plan shows them; nothing was changed", plan.target.drive.name, human(plan.target.drive.size_bytes)));
                }
                e.audit("install.disk", &format!("{} ({}): started", plan.target.drive.path, human(plan.target.drive.size_bytes)));
                let mut last = Phase::Checking;
                let r = i1.execute(&plan, &mut |p| last = p.phase);
                e.audit("install.disk", &format!("{}: {}", plan.target.drive.path, if r.is_ok() { "done".to_string() } else { format!("failed in {}", last.label()) }));
                let r = r?;
                Ok(format!("Done: {} written to {}. Restart from that disk to use it.", human(r.written), plan.target.drive.path))
            })
            .with_preview(move |a| match plan_of(&*i2, a.get("disk").and_then(|d| d.as_str()).unwrap_or("")) {
                Ok(p) => format!("{}This ERASES the whole disk. Back up first.", p.render()),
                Err(e) => format!("This cannot be done: {e}"),
            })
            .with_refusal(move |a| plan_of(&*i3, a.get("disk").and_then(|d| d.as_str()).unwrap_or("")).err())
            .admin(),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Risk;
    use iyakki_core_fake::FakeInstaller;
    use iyakki_core_local::usb::drives::Drive;
    use iyakki_core_local::usb::install::{DiskInfo, SourceInfo};
    use std::sync::Mutex;

    fn disk(name: &str, gb: u64, mounts: Vec<(String, String)>) -> DiskInfo {
        DiskInfo {
            drive: Drive { path: format!("/dev/{name}"), name: name.into(), model: "Test SSD".into(), size_bytes: gb * 1_000_000_000, removable: false, usb: false, read_only: false, mounts, swap: false },
            serial: "S1".into(),
            logical_sector: 512,
            partitions: vec![(format!("{name}1"), 500_000_000, "vfat".into())],
            gpt: true,
            table: "GPT".into(),
        }
    }

    fn source() -> SourceInfo {
        let mut sector0 = vec![0u8; 512];
        for (at, kind, start, sectors) in [(446usize, 0xEFu8, 2048u32, 8192u32), (462, 0x83, 10240, 2048)] {
            sector0[at + 4] = kind;
            sector0[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
            sector0[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
        }
        sector0[510] = 0x55;
        sector0[511] = 0xAA;
        SourceInfo { path: "/nonexistent/stick".into(), sector0, p1_start: 2048, p1_sectors: 8192, p2_start: 10240, copy_bytes: (2048 + 8192) * 512, bios: None }
    }

    fn env(log: Arc<Mutex<Vec<String>>>) -> Env {
        let dir = crate::testdir::unique("iyakki-installdisk", "a");
        Env { dir: dir.clone(), user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&Arc::new(Mutex::new(crate::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()))), audit: Arc::new(move |a, d| log.lock().unwrap().push(format!("{a}: {d}"))) }
    }

    fn call(es: &[Entry], name: &str, a: serde_json::Value) -> Result<String, String> {
        (es.iter().find(|e| e.name == name).unwrap().call)(&a)
    }

    #[test]
    fn the_plan_tool_lists_and_plans_and_changes_nothing_and_the_install_needs_both_typed_words_then_writes_through_the_installer() {
        let fake = Arc::new(FakeInstaller::new());
        fake.set_disks(vec![disk("sda", 500, vec![]), disk("sdb", 256, vec![("/dev/sdb1".into(), "/mnt/x".into())])]);
        fake.set_source(Ok(source()));
        let log = Arc::new(Mutex::new(vec![]));
        let es = entries(&env(log.clone()), fake.clone());
        assert!(es.iter().all(|e| e.admin_only));
        assert_eq!(es.iter().find(|e| e.name == "install_to_disk").unwrap().risk, Risk::Ask);
        assert_eq!(es.iter().find(|e| e.name == "install_disk_plan").unwrap().risk, Risk::Safe);
        assert!(TOOLS.iter().all(|t| es.iter().any(|e| e.name == *t)));

        // the list names the disks, what is on them and a mounted one
        let l = call(&es, "install_disk_plan", json!({})).unwrap();
        assert!(l.contains("can be installed from here") && l.contains("sda (500.0 GB") && l.contains("mounted: /mnt/x"), "{l}");
        // the plan says what is erased and the two words
        let p = call(&es, "install_disk_plan", json!({"disk": "sda"})).unwrap();
        assert!(p.contains("erasing EVERYTHING") && p.contains("confirm_name \"sda\"") && p.contains("confirm_size \"500.0 GB\""), "{p}");
        assert!(fake.written().is_empty(), "planning wrote nothing");
        // a mounted disk and an unknown one are refused by the plan
        assert!(call(&es, "install_disk_plan", json!({"disk": "sdb"})).unwrap_err().contains("mounted"));
        assert!(call(&es, "install_disk_plan", json!({"disk": "sdz"})).unwrap_err().contains("not a disk"));

        // the typed words: wrong name, wrong size, missing: nothing is written
        for a in [json!({"disk": "sda", "confirm_name": "sdb", "confirm_size": "500.0 GB"}), json!({"disk": "sda", "confirm_name": "sda", "confirm_size": "100 GB"}), json!({"disk": "sda", "confirm_name": "sda"})] {
            assert!(call(&es, "install_to_disk", a).is_err());
        }
        assert!(fake.written().is_empty());
        // right words (spaces and case do not matter in the size): the installer writes that disk, and the audit log has both lines
        let r = call(&es, "install_to_disk", json!({"disk": "sda", "confirm_name": "sda", "confirm_size": "500.0gb"})).unwrap();
        assert!(r.contains("Done") && r.contains("/dev/sda"), "{r}");
        assert_eq!(fake.written().len(), 1);
        assert_eq!(fake.written()[0].target.drive.path, "/dev/sda");
        let log = log.lock().unwrap().clone();
        assert!(log.iter().any(|l| l.contains("install.disk: /dev/sda (500.0 GB): started")) && log.iter().any(|l| l.ends_with("/dev/sda: done")), "{log:?}");
        // a disk that is mounted at the moment of the call is refused even with the right words
        fake.set_disks(vec![disk("sda", 500, vec![("/dev/sda1".into(), "/boot".into())])]);
        assert!(call(&es, "install_to_disk", json!({"disk": "sda", "confirm_name": "sda", "confirm_size": "500 GB"})).unwrap_err().contains("mounted"));
        assert_eq!(fake.written().len(), 1);
        // a failing write is reported and audited as failed
        fake.set_disks(vec![disk("sda", 500, vec![])]);
        fake.refuse_execute("the disk went away");
        assert!(call(&es, "install_to_disk", json!({"disk": "sda", "confirm_name": "sda", "confirm_size": "500.0 GB"})).unwrap_err().contains("went away"));
    }

    #[test]
    fn a_machine_with_nothing_to_install_from_says_so_and_refuses() {
        let fake = Arc::new(FakeInstaller::new());
        fake.set_disks(vec![disk("sda", 500, vec![])]);
        let es = entries(&env(Arc::new(Mutex::new(vec![]))), fake.clone());
        assert!(call(&es, "install_disk_plan", json!({})).unwrap().contains("Nothing can be installed from here"));
        assert!(call(&es, "install_to_disk", json!({"disk": "sda", "confirm_name": "sda", "confirm_size": "500 GB"})).is_err());
        assert!(fake.written().is_empty());
    }
}

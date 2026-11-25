//! The fake [`DiskEngine`]: a canned scan, mounts that change the canned picture, every call recorded, nothing read from or written to this computer.
//! It keeps the role rule of the real engine ([`may_use`]) so a tile under test sees the same refusals.

use std::sync::Mutex;

use iyakki_core_api::diskmodel::{check_confirmation_for, may_use, partition_refusal, Actor, AuditFn, Confirm, Disk, DiskEngine, Fs, MountFlags, MountOpts, MountPlan, Mounted, PartOp, Plan, Snapshot, Step};
use iyakki_diskcrypt::{CryptKind, Secret};

#[derive(Default)]
struct State {
    snap: Snapshot,
    users: Vec<(u32, String)>,
    calls: Vec<String>,
    signout: Option<String>,
    /// What `after_unplug` says once (a drive pulled out while it was open).
    unplug: Option<String>,
    /// The key that opens each encrypted partition (`set_key`).
    keys: Vec<(String, String)>,
    programs: Option<String>,
    /// What the assistant asked for, by person (taken once).
    format_requests: std::collections::BTreeMap<String, iyakki_core_api::diskmodel::FormatRequest>,
}

#[derive(Default)]
pub struct FakeDiskEngine {
    state: Mutex<State>,
}

impl FakeDiskEngine {
    pub fn new() -> FakeDiskEngine {
        FakeDiskEngine::default()
    }
    pub fn with(snap: Snapshot) -> FakeDiskEngine {
        let f = FakeDiskEngine::default();
        f.set_snapshot(snap);
        f
    }
    pub fn set_snapshot(&self, s: Snapshot) {
        self.state.lock().unwrap().snap = s;
    }
    /// Who uses every mounted folder (what `users_of` answers).
    pub fn set_users(&self, u: Vec<(u32, String)>) {
        self.state.lock().unwrap().users = u;
    }
    /// What `at_sign_out` says.
    pub fn set_signout(&self, s: Option<String>) {
        self.state.lock().unwrap().signout = s;
    }
    /// What the next `after_unplug` says (once).
    pub fn set_unplug(&self, s: Option<String>) {
        self.state.lock().unwrap().unplug = s;
    }
    /// The key that opens the encrypted partition `part` (any other key is a wrong key).
    pub fn set_key(&self, part: &str, key: &str) {
        self.state.lock().unwrap().keys.push((part.into(), key.into()));
    }
    /// What `use_programs` was last told.
    /// The assistant asks for a format or a resize for `user` (the tile takes it once).
    pub fn ask_format(&self, user: &str, req: iyakki_core_api::diskmodel::FormatRequest) {
        self.state.lock().unwrap().format_requests.insert(user.into(), req);
    }
    pub fn programs(&self) -> Option<String> {
        self.state.lock().unwrap().programs.clone()
    }
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn rec(&self, what: String) {
        self.state.lock().unwrap().calls.push(what);
    }
}

/// The canned plan of a change: what is lost, one sync step, nothing else.
fn plan_of(d: &Disk, op: &PartOp) -> Plan {
    let lost = match op {
        PartOp::NewTable(_) => d.parts.iter().map(|p| format!("{} ({})", p.name, p.fs.name())).collect(),
        PartOp::Delete { number } | PartOp::Format { number, .. } => d.parts.iter().filter(|p| p.number == *number).map(|p| format!("{} ({})", p.name, p.fs.name())).collect(),
        _ => Vec::new(),
    };
    Plan { disk: d.name.clone(), identity: d.identity(), op: op.clone(), lost, after: Vec::new(), steps: vec![Step::Sync], warnings: Vec::new(), expect: Vec::new(), audit: d.audit_id() }
}

impl DiskEngine for FakeDiskEngine {
    fn scan(&self) -> Snapshot {
        self.rec("scan".into());
        self.state.lock().unwrap().snap.clone()
    }
    fn fingerprint(&self) -> u64 {
        let s = self.state.lock().unwrap();
        s.snap.disks.iter().flat_map(|d| d.parts.iter()).fold(s.snap.disks.len() as u64, |h, p| h.wrapping_mul(31).wrapping_add(p.mountpoints.len() as u64 + 1))
    }
    fn forget(&self) {
        self.rec("forget".into());
    }
    fn users_of(&self, mountpoint: &str) -> Vec<(u32, String)> {
        self.rec(format!("users_of({mountpoint})"));
        self.state.lock().unwrap().users.clone()
    }
    fn mounted_in(&self, home: &str) -> Vec<(String, String)> {
        self.rec(format!("mounted_in({home})"));
        let s = self.state.lock().unwrap();
        let prefix = format!("{}/media/", home.trim_end_matches('/'));
        s.snap.disks.iter().flat_map(|d| d.parts.iter()).flat_map(|p| p.mountpoints.iter().filter(|m| m.starts_with(&prefix)).map(|m| (p.name.clone(), m.clone()))).collect()
    }
    fn plan_mount(&self, snap: &Snapshot, who: &Actor, part: &str, opts: &MountOpts) -> Result<MountPlan, String> {
        self.rec(format!("plan_mount({part})"));
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part} is not there"))?;
        may_use(who, d)?;
        if let Some(why) = p.fs.never_mountable() {
            return Err(why.to_string());
        }
        Ok(MountPlan { dev: p.path.clone(), target: format!("{}/media/{}", who.home.trim_end_matches('/'), if p.label.is_empty() { &p.name } else { &p.label }), fstype: p.fs.name(), flags: MountFlags { rdonly: opts.read_only, nosuid: true, nodev: true, noexec: !opts.allow_exec }, data: String::new(), forced_ro: None })
    }
    fn mount(&self, who: &Actor, part: &str, opts: &MountOpts, audit: AuditFn) -> Result<Mounted, String> {
        self.rec(format!("mount({part})"));
        let snap = self.state.lock().unwrap().snap.clone();
        let plan = self.plan_mount(&snap, who, part, opts).inspect_err(|e| audit("disks.mount.refused", &format!("part={part} why={e}")))?;
        let mut s = self.state.lock().unwrap();
        for d in &mut s.snap.disks {
            for p in &mut d.parts {
                if p.name == part {
                    p.mountpoints.push(plan.target.clone());
                    p.mounted_ro = plan.flags.rdonly;
                }
            }
        }
        audit("disks.mount", &format!("part={part} target={}", plan.target));
        Ok(Mounted { target: plan.target, read_only: plan.flags.rdonly, notes: Vec::new() })
    }
    fn unmount(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        self.rec(format!("unmount({part})"));
        let mut s = self.state.lock().unwrap();
        let name = {
            let (d, p) = s.snap.part(part).ok_or_else(|| format!("{part} is not there"))?;
            may_use(who, d).inspect_err(|e| audit("disks.unmount.refused", &format!("part={part} why={e}")))?;
            if p.system {
                return Err("This drive holds the running system; it is never closed.".into());
            }
            p.title()
        };
        for d in &mut s.snap.disks {
            for p in &mut d.parts {
                if p.name == part {
                    p.mountpoints.clear();
                }
            }
        }
        audit("disks.unmount", &format!("part={part}"));
        Ok(format!("{name} is closed."))
    }
    fn eject(&self, who: &Actor, disk: &str, audit: AuditFn) -> Result<String, String> {
        self.rec(format!("eject({disk})"));
        let mut s = self.state.lock().unwrap();
        let i = s.snap.disks.iter().position(|d| d.name == disk).ok_or_else(|| format!("{disk} is not there"))?;
        may_use(who, &s.snap.disks[i]).inspect_err(|e| audit("disks.eject.refused", &format!("drive={disk} why={e}")))?;
        if s.snap.disks[i].system {
            return Err("This drive holds the running system; it is never ejected.".into());
        }
        let d = s.snap.disks.remove(i);
        audit("disks.eject", &d.audit_id());
        Ok(format!("{} can be taken out.", d.title()))
    }
    fn after_unplug(&self, who: &Actor, audit: AuditFn) -> Option<String> {
        self.rec(format!("after_unplug({})", who.user));
        let s = self.state.lock().unwrap().unplug.take();
        if s.is_some() {
            audit("disks.unplugged", &format!("user={}", who.user));
        }
        s
    }
    fn at_sign_out(&self, who: &Actor, audit: AuditFn) -> Option<String> {
        self.rec(format!("at_sign_out({})", who.user));
        let s = self.state.lock().unwrap().signout.clone();
        if s.is_some() {
            audit("disks.signout.left_open", &format!("user={}", who.user));
        }
        s
    }
    fn plan_partition(&self, who: &Actor, disk: &str, op: &PartOp) -> Result<Plan, String> {
        self.rec(format!("plan_partition({disk},{})", op.name()));
        let s = self.state.lock().unwrap();
        let d = s.snap.disk(disk).ok_or_else(|| format!("{disk}: there is no such drive."))?;
        if let Some(r) = partition_refusal(who, d) {
            return Err(r);
        }
        Ok(plan_of(d, op))
    }
    fn apply_partition(&self, who: &Actor, disk: &str, op: &PartOp, confirm: &Confirm, audit: AuditFn) -> Result<String, String> {
        self.rec(format!("apply_partition({disk},{})", op.name()));
        let mut s = self.state.lock().unwrap();
        let Some(d) = s.snap.disk(disk) else { return Err(format!("{disk}: there is no such drive.")) };
        let refused = |why: &str| {
            audit("disks.partition.refused", &format!("{} op={} why={why}", d.audit_id(), op.name()));
            why.to_string()
        };
        if let Some(r) = partition_refusal(who, d) {
            return Err(refused(&r));
        }
        if op.destructive() {
            check_confirmation_for(d, op, confirm).map_err(|e| refused(&e))?;
        }
        let plan = plan_of(d, op);
        audit("disks.partition.apply", &plan.audit);
        let msg = format!("Done: {} on {}.", plan.headline(), d.name);
        let i = s.snap.disks.iter().position(|x| x.name == disk).unwrap_or(0);
        let d = &mut s.snap.disks[i];
        match op {
            PartOp::NewTable(t) => {
                d.table = *t;
                d.parts.clear();
            }
            PartOp::Delete { number } => d.parts.retain(|p| p.number != *number),
            PartOp::Rename { number, label } => d.parts.iter_mut().filter(|p| p.number == *number).for_each(|p| p.label = label.clone()),
            PartOp::Format { number, fs, label } => d.parts.iter_mut().filter(|p| p.number == *number).for_each(|p| {
                p.fs = match fs {
                    iyakki_core_api::diskmodel::FormatFs::Ext4 => Fs::Ext4,
                    iyakki_core_api::diskmodel::FormatFs::Fat32 => Fs::Fat32,
                    iyakki_core_api::diskmodel::FormatFs::Exfat => Fs::Exfat,
                    iyakki_core_api::diskmodel::FormatFs::Ntfs => Fs::Ntfs,
                };
                p.label = label.clone();
            }),
            _ => {}
        }
        audit("disks.partition.done", &plan.audit);
        Ok(msg)
    }
    fn plan_unlock(&self, snap: &Snapshot, who: &Actor, part: &str) -> Result<CryptKind, String> {
        self.rec(format!("plan_unlock({part})"));
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part}: there is no such partition (the drive may have been unplugged)."))?;
        may_use(who, d)?;
        if p.system {
            return Err(format!("{} is part of the running system and stays as it is.", p.name));
        }
        let Some(c) = &p.crypt else { return Err(format!("{} is not an encrypted LUKS or BitLocker volume, so there is nothing to unlock.", p.name)) };
        if p.unlocked {
            return Err(format!("{} is already unlocked.", p.name));
        }
        Ok(c.kind)
    }
    fn unlock(&self, who: &Actor, part: &str, secret: &Secret, audit: AuditFn) -> Result<Mounted, String> {
        self.rec(format!("unlock({part})"));
        let snap = self.state.lock().unwrap().snap.clone();
        let _ = self.plan_unlock(&snap, who, part).inspect_err(|e| audit("disks.unlock.refused", &format!("part={part} why={e}")))?;
        let mut s = self.state.lock().unwrap();
        if !s.keys.iter().any(|(p, k)| p == part && k.as_bytes() == secret.expose()) {
            audit("disks.unlock.wrong_key", &format!("part={part}"));
            return Err("That key does not open it.".into());
        }
        let target = format!("{}/media/{part}", who.home.trim_end_matches('/'));
        for p in s.snap.disks.iter_mut().flat_map(|d| d.parts.iter_mut()).filter(|p| p.name == part) {
            p.unlocked = true;
            p.mountpoints.push(target.clone());
            p.mounted_ro = true;
        }
        audit("disks.unlock", &format!("part={part}"));
        Ok(Mounted { target, read_only: true, notes: Vec::new() })
    }
    fn allow_writing(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<Mounted, String> {
        self.rec(format!("allow_writing({part})"));
        let mut s = self.state.lock().unwrap();
        let (d, p) = s.snap.part(part).ok_or_else(|| format!("{part} is not there"))?;
        may_use(who, d)?;
        if !(p.unlocked && p.mounted_ro) || p.crypt.as_ref().is_some_and(|c| c.kind == CryptKind::BitLocker) {
            return Err("Writing can be allowed only for an unlocked, read-only LUKS volume.".into());
        }
        let target = p.mountpoints.first().cloned().unwrap_or_default();
        for p in s.snap.disks.iter_mut().flat_map(|d| d.parts.iter_mut()).filter(|p| p.name == part) {
            p.mounted_ro = false;
        }
        audit("disks.unlock.writable", &format!("part={part}"));
        Ok(Mounted { target, read_only: false, notes: Vec::new() })
    }
    fn lock(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        self.rec(format!("lock({part})"));
        let mut s = self.state.lock().unwrap();
        let (d, p) = s.snap.part(part).ok_or_else(|| format!("{part} is not there"))?;
        may_use(who, d)?;
        if !p.unlocked {
            return Err(format!("{part} is not unlocked."));
        }
        for p in s.snap.disks.iter_mut().flat_map(|d| d.parts.iter_mut()).filter(|p| p.name == part) {
            p.unlocked = false;
            p.mountpoints.clear();
            p.mounted_ro = false;
        }
        audit("disks.lock", &format!("part={part}"));
        Ok(format!("{part} is locked."))
    }
    fn take_format_request(&self, _dir: &std::path::Path, user: &str) -> Option<iyakki_core_api::diskmodel::FormatRequest> {
        self.rec(format!("take_format_request({user})"));
        self.state.lock().unwrap().format_requests.remove(user)
    }
    fn use_programs(&self, bin_dir: &str) {
        self.rec(format!("use_programs({bin_dir})"));
        self.state.lock().unwrap().programs = Some(bin_dir.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::diskmodel::{Disk, Fs, Part, Table, Transport};
    use iyakki_sec::policy::Role;
    use std::cell::RefCell;

    fn part(name: &str, label: &str) -> Part {
        Part { name: name.into(), path: format!("/dev/{name}"), number: 1, start_bytes: 0, size_bytes: 1 << 30, fs: Fs::Ext4, label: label.into(), uuid: String::new(), mountpoints: vec![], space: None, mounted_ro: false, swap_in_use: false, system: false, type_name: String::new(), crypt: None, unlocked: false }
    }
    fn disk(name: &str, usb: bool, parts: Vec<Part>) -> Disk {
        Disk { name: name.into(), path: format!("/dev/{name}"), vendor: "Acme".into(), model: "Stick".into(), serial: "SN001234".into(), size_bytes: 1 << 30, sector: 512, removable: usb, transport: if usb { Transport::Usb } else { Transport::Sata }, read_only: false, table: Table::Gpt, table_notes: vec![], parts, system: false, system_why: String::new(), holders: vec![] }
    }
    fn fake() -> FakeDiskEngine {
        FakeDiskEngine::with(Snapshot { disks: vec![disk("sdb", true, vec![part("sdb1", "STICK")]), disk("sda", false, vec![part("sda1", "")])], support: vec![] })
    }

    #[test]
    fn a_mount_changes_the_picture_audits_and_the_fingerprint_moves() {
        let f = fake();
        let lines = RefCell::new(Vec::new());
        let audit = |a: &str, d: &str| lines.borrow_mut().push(format!("{a} {d}"));
        let fp = f.fingerprint();
        let who = Actor::new("ann", Role::User);
        let m = f.mount(&who, "sdb1", &MountOpts::default(), &audit).unwrap();
        assert_eq!(m.target, "/home/ann/media/STICK");
        assert_eq!(f.mounted_in("/home/ann"), vec![("sdb1".to_string(), "/home/ann/media/STICK".to_string())]);
        assert_ne!(f.fingerprint(), fp);
        assert!(lines.borrow()[0].starts_with("disks.mount "));
        f.unmount(&who, "sdb1", &audit).unwrap();
        assert!(f.mounted_in("/home/ann").is_empty());
    }

    #[test]
    fn the_role_rule_holds_a_guest_gets_no_drive_and_a_user_only_removable_ones() {
        let f = fake();
        let lines = RefCell::new(Vec::new());
        let audit = |a: &str, d: &str| lines.borrow_mut().push(format!("{a} {d}"));
        assert!(f.mount(&Actor::new("g", Role::Guest), "sdb1", &MountOpts::default(), &audit).is_err());
        assert!(f.mount(&Actor::new("u", Role::User), "sda1", &MountOpts::default(), &audit).unwrap_err().contains("removable"));
        assert!(f.mount(&Actor::new("a", Role::Admin), "sda1", &MountOpts::default(), &audit).is_ok());
        assert!(lines.borrow().iter().any(|l| l.starts_with("disks.mount.refused")));
        assert!(f.eject(&Actor::new("g", Role::Guest), "sdb", &audit).is_err());
        assert_eq!(f.scan().disks.len(), 2, "a refused eject takes nothing away");
    }

    fn crypt_part(name: &str, bitlocker: bool) -> Part {
        let mut p = part(name, "SECRET");
        p.fs = if bitlocker { Fs::BitLocker } else { Fs::Luks };
        p.crypt = Some(iyakki_diskcrypt::CryptInfo { kind: if bitlocker { CryptKind::BitLocker } else { CryptKind::Luks2 }, uuid: String::new(), label: String::new(), detail: String::new(), problem: None });
        p
    }

    #[test]
    fn a_partition_change_needs_an_administrator_the_typed_name_and_the_second_button_and_changes_the_picture() {
        let f = fake();
        let lines = RefCell::new(Vec::new());
        let audit = |a: &str, d: &str| lines.borrow_mut().push(format!("{a} {d}"));
        let admin = Actor::new("root", Role::Admin);
        let op = PartOp::Delete { number: 1 };
        assert!(f.apply_partition(&Actor::new("u", Role::User), "sdb", &op, &Confirm { typed: "sdb".into(), second: true }, &audit).unwrap_err().contains("administrator"));
        assert!(f.apply_partition(&admin, "sdb", &op, &Confirm { typed: String::new(), second: true }, &audit).is_err());
        assert!(f.apply_partition(&admin, "sdb", &op, &Confirm { typed: "sdb".into(), second: false }, &audit).unwrap_err().contains("second confirmation"));
        assert!(f.apply_partition(&admin, "sdb", &op, &Confirm { typed: "sdq".into(), second: true }, &audit).unwrap_err().contains("not"));
        assert_eq!(f.scan().disk("sdb").unwrap().parts.len(), 1, "every refusal leaves the drive as it was");
        let ok = f.apply_partition(&admin, "sdb", &op, &Confirm { typed: "1234".into(), second: true }, &audit).unwrap();
        assert!(ok.starts_with("Done: Delete partition 1"));
        assert!(f.scan().disk("sdb").unwrap().parts.is_empty());
        let l = lines.borrow();
        assert!(l.iter().filter(|x| x.starts_with("disks.partition.refused")).count() >= 4);
        assert!(l.iter().any(|x| x.starts_with("disks.partition.done")));
    }

    #[test]
    fn the_dry_run_names_what_is_lost_and_refuses_the_system_drive_and_open_drives() {
        let f = fake();
        let admin = Actor::new("root", Role::Admin);
        let plan = f.plan_partition(&admin, "sdb", &PartOp::NewTable(Table::Gpt)).unwrap();
        assert_eq!(plan.lost, vec!["sdb1 (ext4)".to_string()]);
        assert!(plan.render().contains("erasing EVERYTHING"));
        f.mount(&admin, "sdb1", &MountOpts::default(), &|_, _| {}).unwrap();
        assert!(f.plan_partition(&admin, "sdb", &PartOp::Reread).unwrap_err().contains("is open at"));
        let mut s = f.scan();
        s.disks[1].system = true;
        s.disks[1].system_why = "holds the system".into();
        f.set_snapshot(s);
        assert!(f.refusal(&admin, &f.scan().disks[1]).unwrap().contains("never changed"));
        assert!(f.plan_partition(&admin, "nope", &PartOp::Reread).is_err());
    }

    #[test]
    fn unlocking_takes_the_right_key_only_opens_read_only_and_locks_again() {
        let mut snap = fake().scan();
        snap.disks[0].parts.push(crypt_part("sdb2", false));
        snap.disks[0].parts.push(crypt_part("sdb3", true));
        let f = FakeDiskEngine::with(snap);
        f.set_key("sdb2", "open sesame");
        f.set_key("sdb3", "bl");
        let who = Actor::new("ann", Role::User);
        let lines = RefCell::new(Vec::new());
        let audit = |a: &str, d: &str| lines.borrow_mut().push(format!("{a} {d}"));
        assert_eq!(f.plan_unlock(&f.scan(), &who, "sdb2").unwrap(), CryptKind::Luks2);
        assert!(f.plan_unlock(&f.scan(), &who, "sdb1").unwrap_err().contains("not an encrypted"));
        assert!(f.unlock(&who, "sdb2", &Secret::new("hunter2"), &audit).unwrap_err().contains("does not open"));
        assert!(!f.scan().part("sdb2").unwrap().1.unlocked);
        let m = f.unlock(&who, "sdb2", &Secret::new("open sesame"), &audit).unwrap();
        assert!(m.read_only);
        assert!(f.plan_unlock(&f.scan(), &who, "sdb2").unwrap_err().contains("already unlocked"));
        assert!(!lines.borrow().iter().any(|l| l.contains("open sesame") || l.contains("hunter2")), "a key is never in an audit line");
        assert!(!f.allow_writing(&who, "sdb2", &audit).unwrap().read_only);
        f.unlock(&who, "sdb3", &Secret::new("bl"), &audit).unwrap();
        assert!(f.allow_writing(&who, "sdb3", &audit).unwrap_err().contains("LUKS"), "BitLocker stays read-only");
        assert!(f.lock(&who, "sdb2", &audit).unwrap().contains("locked"));
        assert!(f.lock(&who, "sdb2", &audit).is_err());
        assert!(f.plan_unlock(&f.scan(), &Actor::new("g", Role::Guest), "sdb3").is_err());
    }

    #[test]
    fn the_tools_seam_is_recorded() {
        let f = fake();
        assert_eq!(f.programs(), None);
        f.use_programs("/data/cli/bin");
        assert_eq!(f.programs().as_deref(), Some("/data/cli/bin"));
    }
}

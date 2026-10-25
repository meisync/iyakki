//! Writing a [`Plan`]: the order of the steps, the checks before and after, and the boot entry.
//!
//! The order is chosen so that the disk's old contents stay bootable for as long as possible:
//!
//! 1. the plan is rebuilt from a fresh read of the disk and must equal the plan the person confirmed (same digest), and nothing
//!    of the disk may be mounted;
//! 2. the new data is written into **unallocated space** (free-space modes) or into the partitions the person named (reuse mode);
//!    existing partitions that stay are not written at all;
//! 3. the partition table is changed **last**: for GPT the backup copy first, then the primary entries, then the primary header;
//!    for MBR sector 0 (only the new entries; boot code and the other entries stay byte for byte);
//! 4. everything is read back: new data by checksum, both GPT copies parsed, every kept partition (first and last MiB) and sector 0
//!    compared with what they were before;
//! 5. only then is the UEFI boot entry added.
//!
//! A power loss before step 3 leaves the old table: the other system boots as before (the new data is in free space, or in the
//! partitions the person chose to give up). Whole-disk mode is the installer's long-standing path ([`crate::install::execute`]).

use std::fs::File;
use std::io::Cursor;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use iyakki_storage::gpt::{self, Entry, Table};
use sha2::{Digest, Sha256};

use crate::drives::{self, Drive};
use crate::efi;
use crate::fatclone::{self, Fat32};
use crate::fatwrite::Fat32Rw;
use crate::gpt_install::{GptLayout, TableKind, NAME_BOOT, NAME_DATA};
use crate::install::{self, DiskInfo, SourceInfo, MIB};
use crate::planner::{build_plan, Action, DiskModel, Mode, Plan, Request, Role, SourceSpec};
use crate::writer::{Phase, Progress};

/// What the writer needs besides the plan.
#[derive(Clone, Debug, Default)]
pub struct ApplyEnv {
    /// Text of `/proc/mounts` right now.
    pub proc_mounts: String,
    /// The firmware variable store (`/sys/firmware/efi/efivars`) when a boot entry should be added.
    pub efivars: Option<PathBuf>,
    /// Testing: stop after the checks, write nothing.
    pub dry_run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyReport {
    pub written: u64,
    pub verified: bool,
    /// The UEFI entry added, or why none was.
    pub boot_entry: Option<String>,
    pub notes: Vec<String>,
}

const KEEP_DIR: [&str; 2] = ["EFI", "IYAKKI"];
pub const KEEP_KERNEL: &str = "KERNEL.EFI";
pub const KEEP_INITRD: &str = "INITRD";
const DEFAULT_ARGS: &str = "console=tty0 panic=20 quiet loglevel=3 vt.global_cursor_default=0";

/// The image's needs, read from the image: the boot volume size, the kernel and RAM disk bytes, whether it is a Secure Boot image.
pub fn source_spec(src: &SourceInfo) -> Result<SourceSpec, String> {
    let mut f = File::open(&src.path).map_err(|e| format!("{}: {e}", src.path.display()))?;
    let mut payload = 0u64;
    if let Ok(mut fat) = Fat32::open(&mut f, src.p1_start * 512) {
        for it in fat.walk()? {
            if !it.dir && matches!(it.path.to_ascii_uppercase().as_str(), "/BOOT/BZIMAGE" | "/BOOT/INITRD") {
                payload += it.size as u64;
            }
        }
    }
    Ok(SourceSpec { path: src.path.clone(), boot_bytes: src.p1_sectors * 512, payload_bytes: payload, secure_boot: install::is_secure_boot_image(src) })
}

/// The first `linux` line's arguments of the image's boot menu (the plain "IYAKKI" entry), for an EFI-stub boot.
pub fn kernel_args(grub_cfg: &str) -> String {
    for l in grub_cfg.lines().map(str::trim) {
        if let Some(rest) = l.strip_prefix("linux ") {
            let args = rest.split_once(' ').map_or("", |(_, a)| a).trim();
            if !args.is_empty() {
                return args.to_string();
            }
        }
    }
    DEFAULT_ARGS.to_string()
}

fn put(f: &File, what: &str, off: u64, data: &[u8]) -> Result<(), String> {
    f.write_all_at(data, off).map_err(|e| format!("writing {what}: {e}"))
}

fn sha_range(f: &File, off: u64, len: u64) -> Result<String, String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    while done < len {
        let n = ((len - done) as usize).min(buf.len());
        f.read_exact_at(&mut buf[..n], off + done).map_err(|e| format!("reading back: {e}"))?;
        h.update(&buf[..n]);
        done += n as u64;
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Hash of a partition's first and last MiB: the cheap proof that a kept partition was not written.
fn sentinel(f: &File, off: u64, len: u64) -> Result<String, String> {
    let head = len.min(MIB);
    let mut h = sha_range(f, off, head)?;
    if len > 2 * MIB {
        h.push_str(&sha_range(f, off + len - MIB, MIB)?);
    }
    Ok(h)
}

/// The marker block the installer writes at the start of a data partition (first boot formats it).
fn data_marker(req: &Request) -> Result<Vec<u8>, String> {
    let mut marker = vec![0u8; MIB as usize];
    marker[..iyakki_storage::PENDING_MARKER.len()].copy_from_slice(iyakki_storage::PENDING_MARKER);
    let conf = iyakki_storage::pending_conf_bytes(&req.choices().conf_text())?;
    let off = iyakki_storage::PENDING_CONF_OFFSET as usize;
    marker[off..off + conf.len()].copy_from_slice(&conf);
    Ok(marker)
}

fn random_guid() -> gpt::Guid {
    gpt::random_guid()
}

fn mbr_entry(s0: &mut [u8], slot: u32, status: u8, kind: u8, start: u64, count: u64) {
    let e = 446 + 16 * (slot as usize - 1);
    s0[e] = status;
    s0[e + 1..e + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    s0[e + 4] = kind;
    s0[e + 5..e + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    s0[e + 8..e + 12].copy_from_slice(&(start as u32).to_le_bytes());
    s0[e + 12..e + 16].copy_from_slice(&(count as u32).to_le_bytes());
}

/// The table after the plan: a GPT (built from the fresh `table`) or the new sector 0 of an MBR disk.
enum NewTable {
    Gpt(Table),
    Mbr(Vec<u8>),
}

fn new_table(model: &DiskModel, plan: &Plan, s0: &[u8], gpt_now: Option<Table>) -> Result<NewTable, String> {
    let ss = model.sector as u64;
    match plan.table_after {
        TableKind::Gpt => {
            let mut t = gpt_now.unwrap_or_else(|| Table { sector_size: model.sector, total_sectors: model.total_sectors(), disk_guid: random_guid(), entries: vec![] });
            // a table made for a smaller disk (or a cloned image) is rewritten for the true size
            t.total_sectors = model.total_sectors();
            for st in &plan.steps {
                let (ty, name) = match st.role {
                    Some(Role::Boot) => (gpt::TYPE_ESP, NAME_BOOT),
                    Some(Role::Data) => (gpt::TYPE_LINUX_DATA, NAME_DATA),
                    _ => continue,
                };
                match st.action {
                    Action::Create => t.entries.push(Entry { index: st.index, type_guid: ty, guid: random_guid(), first_lba: st.first_lba, last_lba: st.first_lba + st.sectors - 1, attrs: 0, name: name.into() }),
                    Action::Format => {
                        let e = t.entries.iter_mut().find(|e| e.index == st.index).ok_or("internal error: a formatted partition is not in the GPT")?;
                        e.type_guid = ty;
                        e.name = name.into();
                        e.attrs = 0;
                        // a fresh GUID: another system may remember the old one
                        e.guid = random_guid();
                    }
                    _ => {}
                }
            }
            t.entries.sort_by_key(|e| e.index);
            t.check()?;
            Ok(NewTable::Gpt(t))
        }
        TableKind::Mbr => {
            let mut s = if plan.new_table { vec![0u8; 512] } else { s0[..512].to_vec() };
            if plan.new_table {
                let sig = {
                    let g = random_guid();
                    [g[0], g[1], g[2], g[3] | 1]
                };
                s[440..444].copy_from_slice(&sig);
            }
            for st in &plan.steps {
                let ty = match st.role {
                    Some(Role::Boot) => 0xEF,
                    Some(Role::Data) => 0x83,
                    _ => continue,
                };
                let k = 512 / ss.min(512);
                match st.action {
                    Action::Create => mbr_entry(&mut s, st.index, 0, ty, st.first_lba * k, st.sectors * k),
                    Action::Format => s[446 + 16 * (st.index as usize - 1) + 4] = ty,
                    _ => {}
                }
            }
            s[510] = 0x55;
            s[511] = 0xAA;
            Ok(NewTable::Mbr(s))
        }
    }
}

fn legacy_plan(model: &DiskModel, plan: &Plan, src: &SourceInfo) -> Result<install::Plan, String> {
    let d = &model.path;
    let drive = Drive { path: d.clone(), name: model.name.clone(), model: model.model.clone(), size_bytes: model.size_bytes, removable: model.removable, usb: model.usb, read_only: false, mounts: vec![], swap: false };
    let disk = DiskInfo { drive, serial: model.serial.clone(), logical_sector: model.sector, partitions: vec![], gpt: model.table == Some(TableKind::Gpt), table: String::new() };
    let (layout, g) = match plan.table_after {
        TableKind::Gpt => {
            let g = GptLayout::new(model.size_bytes, model.sector, src.p1_sectors * 512)?;
            (g.data_layout(), Some(g))
        }
        TableKind::Mbr => {
            if src.bios.is_some() {
                return Err("a whole-disk MBR install needs the install template; this stick has a GPT. Choose GPT (--table gpt) or use the template file.".into());
            }
            let mut s0 = src.sector0.clone();
            (crate::install::patch_mbr(&mut s0, model.size_bytes, [0; 4])?, None)
        }
    };
    Ok(install::Plan { target: disk, source: src.clone(), layout, choices: plan.request.choices(), steps: vec![], warnings: vec![], table: plan.table_after, table_why: plan.table_why.clone(), gpt: g })
}

fn mounted_now(model: &DiskModel, proc_mounts: &str, names: &[String]) -> Option<String> {
    let all = drives::parse_proc_mounts(proc_mounts);
    all.iter().find(|(dev, _)| drives::belongs_to(dev, &model.name) && names.iter().any(|n| dev.rsplit('/').next() == Some(n.as_str()))).map(|(d, m)| format!("{d} is mounted at {m}"))
}

/// Write the plan for `req`. `model` is a **fresh** read of the disk; `expected_digest` is the digest of the plan the person
/// confirmed (`Plan::digest`); both must agree with the plan rebuilt here. `path` is the file or device to write.
pub fn apply(path: &Path, model: &DiskModel, req: &Request, src: &SourceInfo, expected_digest: Option<&str>, uefi: bool, env: &ApplyEnv, progress: &mut dyn FnMut(Progress)) -> Result<ApplyReport, String> {
    // tests write regular files standing in for disks, never a device
    #[cfg(test)]
    if !std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
        return Err(format!("{}: tests never write anything but regular files", path.display()));
    }
    let spec = source_spec(src)?;
    let plan = build_plan(model, req, &spec, uefi)?;
    if let Some(d) = expected_digest {
        if d != plan.digest() {
            return Err("the disk or the plan changed since you confirmed it (the digests differ); nothing was written. Make the plan again.".into());
        }
    }
    let destroyed: Vec<String> = plan.steps.iter().filter(|s| s.action.destructive() || s.action == Action::AddFiles).map(|s| s.part.clone()).collect();
    if let Some(m) = mounted_now(model, &env.proc_mounts, &if req.mode == Mode::WholeDisk { model.parts.iter().map(|p| p.name.clone()).collect() } else { destroyed }) {
        return Err(format!("{m}; unmount it first. Nothing was written."));
    }
    if env.dry_run {
        return Ok(ApplyReport { written: 0, verified: false, boot_entry: None, notes: vec!["dry run: nothing was written".into()] });
    }
    if req.mode == Mode::WholeDisk {
        let lp = legacy_plan(model, &plan, src)?;
        let r = install::execute(&lp, false, &env.proc_mounts, progress)?;
        return Ok(ApplyReport { written: r.written, verified: true, boot_entry: None, notes: vec![] });
    }
    apply_partial(path, model, &plan, src, env, progress)
}

fn apply_partial(path: &Path, model: &DiskModel, plan: &Plan, src: &SourceInfo, env: &ApplyEnv, progress: &mut dyn FnMut(Progress)) -> Result<ApplyReport, String> {
    let t0 = Instant::now();
    let ss = model.sector as u64;
    let disp = path.display().to_string();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{disp}: {e}"))?;
    // before: sector 0 and every kept partition
    let mut s0 = vec![0u8; 512.max(ss as usize)];
    f.read_exact_at(&mut s0, 0).map_err(|e| format!("reading {disp}: {e}"))?;
    let gpt_now = if model.table == Some(TableKind::Gpt) {
        let mut h = File::open(path).map_err(|e| e.to_string())?;
        Some(gpt::read_gpt(&mut h, model.sector, model.total_sectors()).map_err(|e| format!("the GPT cannot be read: {e}"))?.table)
    } else {
        None
    };
    let mut kept: Vec<(String, u64, u64, String)> = Vec::new();
    for st in plan.steps.iter().filter(|s| s.action == Action::Keep) {
        kept.push((st.part.clone(), st.first_lba * ss, st.bytes, sentinel(&f, st.first_lba * ss, st.bytes)?));
    }
    let table = new_table(model, plan, &s0, gpt_now.clone())?;
    let total_work: u64 = plan.steps.iter().filter(|s| matches!(s.action, Action::Create | Action::Format)).map(|s| if s.role == Some(Role::Boot) { s.bytes } else { 2 * MIB }).sum::<u64>() + 2 * MIB;
    let mut done = 0u64;
    let mut say = |phase: Phase, add: u64, done: &mut u64| {
        *done += add;
        progress(Progress { phase, done: (*done).min(total_work), total: total_work, elapsed: t0.elapsed() });
    };
    let mut regions: Vec<(u64, u64, String)> = Vec::new();

    // 1. the data of the new and reused partitions (new ones sit in free space; the old table still describes the old disk)
    let marker = data_marker(&plan.request)?;
    let volume_id = {
        let g = random_guid();
        u32::from_le_bytes([g[0], g[1], g[2], g[3]]) ^ 0x4959_414B
    };
    let mut lean_note: Option<String> = None;
    for st in plan.steps.iter().filter(|s| matches!(s.action, Action::Create | Action::Format)) {
        let off = st.first_lba * ss;
        match st.role {
            Some(Role::Data) => {
                put(&f, &st.part, off, &marker)?;
                regions.push((off, 8192, sha_of(&marker[..8192])));
                if st.action == Action::Format && st.bytes > 2 * MIB {
                    put(&f, &st.part, off + st.bytes - MIB, &vec![0u8; MIB as usize])?;
                }
                say(Phase::Writing, 2 * MIB, &mut done);
            }
            Some(Role::Boot) => {
                let mut input = File::open(&src.path).map_err(|e| format!("{}: {e}", src.path.display()))?;
                let mut vol = Fat32::open(&mut input, src.p1_start * 512)?;
                let mut h = Sha256::new();
                let (mut pos, mut since) = (0u64, 0u64);
                let mut buf: Vec<u8> = Vec::with_capacity(4 * MIB as usize);
                let mut flush_buf = |buf: &mut Vec<u8>, pos: &mut u64, since: &mut u64, done: &mut u64| -> Result<(), String> {
                    if buf.is_empty() {
                        return Ok(());
                    }
                    h.update(&buf[..]);
                    put(&f, &st.part, off + *pos, buf)?;
                    *pos += buf.len() as u64;
                    *since += buf.len() as u64;
                    if *since >= 8 * MIB {
                        f.sync_data().map_err(|e| format!("flushing {disp}: {e}"))?;
                        *since = 0;
                    }
                    say(Phase::Writing, buf.len() as u64, done);
                    buf.clear();
                    Ok(())
                };
                // a boot volume that carries the fast-start (lean) entries and files is copied without them: the new disk has no
                // lean tree and no data volume yet (crate::leanstrip)
                let items = vol.walk_long()?;
                let paths: Vec<String> = items.iter().filter(|i| !i.dir).map(|i| i.path.clone()).collect();
                let cfg = items.iter().find(|i| !i.dir && i.path.eq_ignore_ascii_case("/boot/grub/grub.cfg")).map(|i| vol.read_file(i)).transpose()?.map(|b| String::from_utf8_lossy(&b).into_owned());
                let mut lean = crate::leanstrip::LeanFree::new();
                let mut plain = fatclone::Unchanged;
                let editor: &mut dyn fatclone::Editor = if crate::leanstrip::needs_cleaning(&paths, cfg.as_deref()) { &mut lean } else { &mut plain };
                fatclone::clone_to_with(&mut vol, model.sector, st.bytes, NAME_BOOT, volume_id, editor, &mut |b| {
                    buf.extend_from_slice(b);
                    if buf.len() >= 4 * MIB as usize {
                        flush_buf(&mut buf, &mut pos, &mut since, &mut done)?;
                    }
                    Ok(())
                })?;
                if lean.menu_changed || !lean.dropped.is_empty() {
                    lean_note = Some("the fast-start (lean boot) menu entries and files of the install stick were not copied: this disk has no lean tree yet; the default entry is IYAKKI".to_string());
                }
                flush_buf(&mut buf, &mut pos, &mut since, &mut done)?;
                regions.push((off, st.bytes, h.finalize().iter().map(|b| format!("{b:02x}")).collect()));
            }
            _ => {}
        }
    }
    // keep the existing ESP: add EFI/IYAKKI/{KERNEL.EFI, INITRD}
    let mut keep_args = String::new();
    let mut esp_before: Vec<fatclone::Item> = Vec::new();
    if plan.keep_esp {
        let st = plan.steps.iter().find(|s| s.action == Action::AddFiles).ok_or("internal error: no ESP step")?;
        let esp_off = st.first_lba * ss;
        {
            let mut h = File::open(path).map_err(|e| e.to_string())?;
            esp_before = Fat32::open(&mut h, esp_off)?.walk()?;
        }
        let mut input = File::open(&src.path).map_err(|e| e.to_string())?;
        let mut vol = Fat32::open(&mut input, src.p1_start * 512)?;
        let items = vol.walk()?;
        let find = |p: &str| items.iter().find(|i| !i.dir && i.path.eq_ignore_ascii_case(p)).cloned().ok_or_else(|| format!("the image has no {p}"));
        let (k, r) = (find("/boot/bzImage")?, find("/boot/initrd")?);
        keep_args = items.iter().find(|i| i.path.eq_ignore_ascii_case("/boot/grub/grub.cfg")).map(|i| vol.read_file(i)).transpose()?.map_or_else(|| DEFAULT_ARGS.to_string(), |b| kernel_args(&crate::leanstrip::strip_lean_block(&String::from_utf8_lossy(&b))));
        let kernel = vol.read_file(&k)?;
        let initrd = vol.read_file(&r)?;
        let mut rw = Fat32Rw::open(&f, esp_off, st.bytes)?;
        let need = (kernel.len() + initrd.len()) as u64 + 4 * MIB;
        if rw.free_bytes() < need {
            return Err(format!("the EFI System Partition has {} free, {} are needed; nothing was written", crate::ui::human_size(rw.free_bytes()), crate::ui::human_size(need)));
        }
        let (dir, existed) = rw.mkdir_p(&KEEP_DIR)?;
        if existed {
            return Err("the folder EFI/IYAKKI already exists on the EFI System Partition (an earlier install?). It is not overwritten: remove that folder with the other system or the IYAKKI shell, then try again. Nothing else was changed.".into());
        }
        rw.create_file(dir, KEEP_KERNEL, kernel.len() as u64, &mut Cursor::new(&kernel), &mut |n| say(Phase::Writing, n, &mut done))?;
        rw.create_file(dir, KEEP_INITRD, initrd.len() as u64, &mut Cursor::new(&initrd), &mut |n| say(Phase::Writing, n, &mut done))?;
        rw.flush()?;
        rw.write_fsinfo()?;
    }
    f.sync_all().map_err(|e| format!("flushing {disp}: {e}"))?;

    // 2. the table, last
    let parsed_expect: Option<Table>;
    let mbr_expect: Option<Vec<u8>>;
    match &table {
        NewTable::Gpt(t) => {
            let (entries, hb, hp) = (t.entries_bytes(), t.header_bytes(true), t.header_bytes(false));
            // the backup copy first, the primary entries, then the primary header
            put(&f, &disp, t.backup_entries_lba() * ss, &entries)?;
            put(&f, &disp, t.last_lba() * ss, &hb)?;
            f.sync_data().map_err(|e| e.to_string())?;
            if plan.new_table {
                // a blank disk: the protective MBR (no boot code) goes in first so that the table is complete
                let sig = {
                    let g = random_guid();
                    [g[0], g[1], g[2], g[3]]
                };
                put(&f, &disp, 0, &t.protective_mbr(&[], sig))?;
            }
            put(&f, &disp, 2 * ss, &entries)?;
            f.sync_data().map_err(|e| e.to_string())?;
            put(&f, &disp, ss, &hp)?;
            parsed_expect = Some(t.clone());
            mbr_expect = None;
        }
        NewTable::Mbr(s) => {
            put(&f, &disp, 0, s)?;
            parsed_expect = None;
            mbr_expect = Some(s.clone());
        }
    }
    say(Phase::Syncing, 0, &mut done);
    f.sync_all().map_err(|e| format!("flushing {disp}: {e}"))?;

    // 3. read everything back
    let total_verify: u64 = regions.iter().map(|r| r.1).sum::<u64>().max(1);
    let mut verified = 0u64;
    for (off, len, sha) in &regions {
        let got = sha_range(&f, *off, *len)?;
        if &got != sha {
            return Err(format!("verification FAILED at byte {off}: what was written does not read back identically. The new partition is not usable; the other partitions were not changed. Run the installer again."));
        }
        verified += len;
        progress(Progress { phase: Phase::Verifying, done: verified, total: total_verify, elapsed: t0.elapsed() });
    }
    if let Some(t) = &parsed_expect {
        let mut h = File::open(path).map_err(|e| e.to_string())?;
        let p = gpt::read_gpt(&mut h, model.sector, model.total_sectors()).map_err(|e| format!("verification FAILED: the partition table on the disk is not valid ({e})"))?;
        if !p.primary_ok || !p.backup_ok || !p.problems.is_empty() || p.table != *t {
            return Err(format!("verification FAILED: the two copies of the partition table are not as written ({}).", p.problems.join("; ")));
        }
    }
    if let Some(s) = &mbr_expect {
        let mut back = vec![0u8; 512];
        f.read_exact_at(&mut back, 0).map_err(|e| e.to_string())?;
        if &back != s {
            return Err("verification FAILED: sector 0 is not as written".into());
        }
    }
    // nothing outside the plan changed
    if parsed_expect.is_some() && !plan.new_table {
        let mut now = vec![0u8; 512.max(ss as usize)];
        f.read_exact_at(&mut now, 0).map_err(|e| e.to_string())?;
        if now != s0 {
            return Err("verification FAILED: sector 0 (the protective MBR) changed, and it must not".into());
        }
    }
    for (name, off, len, sha) in &kept {
        if &sentinel(&f, *off, *len)? != sha {
            return Err(format!("verification FAILED: the kept partition {name} changed while installing"));
        }
    }
    if plan.keep_esp {
        let st = plan.steps.iter().find(|s| s.action == Action::AddFiles).unwrap();
        let mut h = File::open(path).map_err(|e| e.to_string())?;
        let after = Fat32::open(&mut h, st.first_lba * ss)?.walk()?;
        for it in &esp_before {
            if !after.iter().any(|a| a == it) {
                return Err(format!("verification FAILED: {} on the EFI System Partition changed", it.path));
            }
        }
        for want in ["/EFI/IYAKKI/KERNEL.EFI", "/EFI/IYAKKI/INITRD"] {
            if !after.iter().any(|a| a.path == want) {
                return Err(format!("verification FAILED: {want} is missing after writing"));
            }
        }
    }
    drop(f);
    drives::reread_partitions(&disp);

    // 4. the UEFI boot entry
    let mut notes = Vec::new();
    notes.extend(lean_note);
    let mut boot_entry = None;
    if let Some(dir) = &env.efivars {
        let st = plan.steps.iter().find(|s| s.role == Some(Role::Boot) && s.action != Action::Keep).ok_or("internal error: no boot step")?;
        let signature = match &table {
            NewTable::Gpt(t) => efi::Signature::Gpt(t.entries.iter().find(|e| e.index == st.index).map(|e| e.guid).ok_or("internal error: boot entry missing")?),
            NewTable::Mbr(s) => efi::Signature::Mbr(s[440..444].try_into().unwrap()),
        };
        let (file, args) = if plan.keep_esp { (efi::backslashed("/EFI/IYAKKI/KERNEL.EFI"), format!("initrd={} {keep_args}", efi::backslashed("/EFI/IYAKKI/INITRD"))) } else { (efi::backslashed("/EFI/BOOT/BOOTX64.EFI"), String::new()) };
        match efi::add_boot_entry(dir, &efi::BootEntry { number: st.index, start_lba: st.first_lba, sectors: st.sectors, signature, file, args }) {
            Ok(n) => {
                notes.push(format!("UEFI boot entry {n} \"IYAKKI\" added at the end of the boot order; pick it in the firmware's boot menu"));
                boot_entry = Some(n);
            }
            Err(e) => notes.push(format!("could not add the UEFI boot entry ({e}); pick the disk in the firmware's boot menu instead")),
        }
    } else {
        notes.push("no UEFI boot entry was added; pick the disk in the firmware's boot menu".into());
    }
    Ok(ApplyReport { written: done, verified: true, boot_entry, notes })
}

fn sha_of(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Reading a regular file (or device) as a disk model, for the callers (CLI, GUI) that have only a path.
pub fn model_of_file(path: &Path, meta: &crate::planner::DiskMeta, size_bytes: u64, sector: u32, proc_mounts: &str, boot_media: Option<&str>) -> Result<DiskModel, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(crate::planner::read_model(&mut f, meta, size_bytes, sector, &drives::parse_proc_mounts(proc_mounts), boot_media))
}

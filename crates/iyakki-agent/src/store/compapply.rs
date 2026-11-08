//! Applying system components to a running OS, safely (docs/development/COMPONENT-UPDATES-PLAN.md, "Built so far"; docs/UPDATES.md).
//!
//! The pieces before this file stop at "verified and staged". This file takes the staged, hash-checked files and hands them to the
//! machinery that already updates a system, so nothing here invents a new way to write boot files:
//!
//! * **kernel, boot, base** become an update payload (`update.json` + `boot/...`) and go through the Update centre's engine
//!   ([`Centre::install`]: the same planning, staging with checksums, previous version copied to the data volume and kept in the boot menu as
//!   "IYAKKI (previous version)", UPDATING marker, atomic replace, boot counter, trial record). After it, every written file is read back
//!   against the SHA-256 of the signed list; a mismatch puts the previous version back with [`Centre::rollback`] (F-UPD-07) and nothing is
//!   recorded as installed.
//! * **core** and **kernel-layer** (files of the running system) become a patch pack (a delta against the running program when the store
//!   has one) and go through the patch machinery in *next start* mode: stored now, active at the next start, the patch start counter turns
//!   a pack off by itself if the system does not come up. The running program is not touched.
//!
//! The person's approval is a value ([`Approval`]) that only [`Approval::given`] makes, from the [`Review`] the card showed; it names that
//! review, so a plan that changed after the card was shown is not applied. [`apply_staged`] refuses without it, when free space is short,
//! when the boot partition is not this install's, when a `requires` range would break, and when an update is already pending. The installed
//! record ([`super::compinstalled`]) is written only after every part was verified.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::compmanifest::{self as cm, Restart};
use super::compinstalled::{self as ci, Facts};
use super::components::{self as comps, human, Installed, Plan, Release, Staged, Via};
use crate::devmode::centre::{self, Centre, Trust};
use crate::devmode::{files, patch, update as up, DevMode};

// ------------------------------------------------------------------------------------------------ what the apply works on

/// The machine the apply works on: the Update centre's engine, the patch machinery's system effects and root, the facts for the installed
/// record. A device uses [`Svc::real`]; tests build one around folders.
pub struct Svc {
    pub centre: Centre,
    pub live: Box<dyn patch::Live>,
    /// The running system's root (`/`); a folder in tests.
    pub patch_root: PathBuf,
    pub facts: Facts,
    /// Fixes the free space of the work folder and the data volume the patch layer goes to (tests).
    pub work_free: Option<u64>,
    /// Test knob, like `up::Env::fault_after`: runs between the end of the install and its read-back. None on a device.
    pub after_write: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Svc {
    pub fn real(data: &Path) -> Svc {
        Svc { centre: Centre::real(data), live: Box::new(patch::SysLive), patch_root: PathBuf::from("/"), facts: Facts::real(), work_free: None, after_write: None }
    }

    pub fn data(&self) -> &Path {
        self.centre.data()
    }

    /// The folder the staged blobs and the payloads are made in (on the data volume).
    pub fn work(&self) -> PathBuf {
        self.data().join("component-work")
    }

    /// What is installed, corrected from the facts (not saved).
    pub fn installed(&self) -> Installed {
        ci::current(self.data(), &self.facts)
    }

    fn free(&self, p: &Path) -> Option<u64> {
        self.work_free.or_else(|| crate::sharedsite::statvfs_free(p))
    }
}

// ------------------------------------------------------------------------------------------------ the review and the approval

/// What the person is shown before anything is downloaded or written, and what an approval is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub text: String,
    /// Identifies exactly this plan (store, channel, release, every part with its version and hashes).
    pub digest: String,
    /// Any part needs the machine to restart (kernel, boot, base).
    pub restart_machine: bool,
    /// Any part is stored for the next start of IYAKKI's own programs (core, kernel layer).
    pub restart_core: bool,
    pub parts: usize,
    pub download: u64,
}

/// The person's yes to one [`Review`]. Only [`Approval::given`] makes one, and the only callers are the assistant's approval card and
/// Settings' confirm key, after they showed the review's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    digest: String,
    by: String,
}

impl Approval {
    pub fn given(review: &Review, by: &str) -> Approval {
        Approval { digest: review.digest.clone(), by: by.to_string() }
    }

    pub fn by(&self) -> &str {
        &self.by
    }
}

pub fn digest_of(p: &Plan, release: Option<&Release>) -> String {
    let mut h = Sha256::new();
    h.update(format!("{}|{}|", p.store, p.channel));
    if let Some(r) = release {
        h.update(format!("{}|{}|", r.version, r.variant));
    }
    for s in &p.steps {
        h.update(format!("{}|{}|{}|{}|{}|{}|{:?};", s.component, s.from.as_deref().unwrap_or("-"), s.to, s.sha256, s.blob_sha256, s.install_path, s.via));
    }
    crate::pkg::hex(&h.finalize())
}

/// The release a plan's parts become: the store's signed label, unless the caller overrides it.
pub fn release_of(p: &Plan, over: Option<&Release>) -> Result<Release, String> {
    match (over, &p.release) {
        (Some(o), _) => Ok(o.clone()),
        (None, Some(l)) => Ok(Release::from(l)),
        (None, None) => Err("the store's signed list carries no release label (the IYAKKI version and variant the parts belong to): the store's publisher has to add one, or give the label yourself".into()),
    }
}

/// The card: what changes, how big, whether a restart is needed, and what is kept to go back to. Nothing has been fetched.
pub fn review(p: &Plan, release: Option<&Release>) -> Review {
    let mut lines = vec![];
    if let Some(r) = release {
        lines.push(format!("Install system components from {} (channel {}) as IYAKKI {} ({}).", p.store, p.channel, r.version, r.variant));
    } else {
        lines.push(format!("Install system components from {} (channel {}).", p.store, p.channel));
    }
    let (mut machine, mut core) = (false, false);
    for s in &p.steps {
        let how = match s.via {
            Via::Delta => format!("only the difference, {} (the whole file is {})", human(s.download), human(s.full_download)),
            Via::Full => format!("the whole file, {}", human(s.download)),
        };
        let after = match s.restart {
            Restart::Machine => {
                machine = true;
                "needs a restart of the machine to run"
            }
            Restart::Services => {
                core = true;
                "stored now, takes over at the next start of IYAKKI (needs a restart)"
            }
        };
        lines.push(format!("  {} {} -> {}: {how}; {after}.", s.component, s.from.as_deref().unwrap_or("(not on record)"), s.to));
    }
    lines.push(format!("Download: {} in all, checked against the store's signed list before anything is written.", human(p.download_total())));
    lines.push(if machine || core { "A restart is needed to use it; the computer does not restart by itself.".to_string() } else { "No restart is needed.".to_string() });
    let mut kept = vec![];
    if p.steps.iter().any(|s| !s.install_path.starts_with('/')) {
        kept.push("the version you run now stays on the boot menu as \"IYAKKI (previous version)\" (the boot menu starts it by itself if the new start does not reach the first picture), and Settings -> System update can roll back to it");
    }
    if p.steps.iter().any(|s| s.install_path.starts_with('/')) {
        kept.push("the running IYAKKI program is not replaced: the new one is stored as a patch layer for the next start, and the patch start counter turns the layer off by itself if starts keep failing");
    }
    for k in kept {
        lines.push(format!("Kept for going back: {k}."));
    }
    lines.push("Your files, accounts and settings are not touched.".to_string());
    for b in &p.blocked {
        lines.push(format!("Not included: {}", b.why));
    }
    lines.push("Approving this downloads, checks and installs these parts; if any check fails nothing is recorded as installed.".to_string());
    Review { text: lines.join("\n"), digest: digest_of(p, release), restart_machine: machine, restart_core: core, parts: p.steps.len(), download: p.download_total() }
}

// ------------------------------------------------------------------------------------------------ staging

/// Everything the apply needs: the plan, the verified files, and the release label they become.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub plan: Plan,
    pub staged: Staged,
    pub release: Release,
}

/// Download and verify what `plan` says (the existing staging step) into the work folder. Applies nothing.
#[allow(clippy::too_many_arguments)]
pub fn prepare(svc: &Svc, plan: &Plan, base: &str, st: &dyn iyakki_proxy::Streamer, release_override: Option<&Release>, progress: &mut dyn FnMut(&str, u64, u64), cancel: &dyn Fn() -> bool) -> Result<Prepared, String> {
    let release = release_of(plan, release_override)?;
    if plan.steps.is_empty() {
        return Err("there is nothing to install".into());
    }
    let inst = svc.installed();
    let staged = comps::stage(plan, base, st, &inst, &svc.work().join("staging"), 2, progress, cancel)?;
    Ok(Prepared { plan: plan.clone(), staged, release })
}

// ------------------------------------------------------------------------------------------------ the apply

#[derive(Clone, Debug)]
pub struct Applied {
    pub lines: Vec<String>,
    pub restart_machine: bool,
    pub restart_core: bool,
    /// The record after the apply.
    pub installed: Installed,
    /// The Update centre's sentence about the boot partition, when parts went there.
    pub boot_message: Option<String>,
}

impl Applied {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

fn is_boot_part(path: &str) -> bool {
    !path.starts_with('/')
}

fn is_root_image(path: &str) -> bool {
    path.eq_ignore_ascii_case(up::ROOT_IMAGE)
}

/// Something written earlier that has not finished (an unfinished update, a restart still to come).
fn pending(svc: &Svc) -> Option<String> {
    if let Some(t) = up::interrupted_text(svc.data()) {
        return Some(format!("an earlier update did not finish: {t}"));
    }
    if let Some(t) = centre::read_trial(svc.data()).filter(|t| t.state == "restart") {
        return Some(format!("an update to IYAKKI {} is already written and waits for a restart (restart first, then check again)", t.to));
    }
    let inst = svc.installed();
    if !inst.awaiting.is_empty() {
        return Some(format!("{} written earlier waits for a restart (restart first, then check again)", inst.awaiting.keys().cloned().collect::<Vec<_>>().join(", ")));
    }
    None
}

/// The `requires` of every part, against what the device has now with the planned versions put over it.
fn recheck(p: &Plan, inst: &Installed) -> Result<(), String> {
    for s in &p.steps {
        if inst.versions.get(&s.component) != s.from.as_ref() {
            return Err(format!("{} changed on this device since the plan was made (now {}): look at the plan again", s.component, inst.versions.get(&s.component).map_or("not on record", String::as_str)));
        }
    }
    for s in &p.steps {
        for (k, want) in &s.requires {
            if cm::COMPONENTS.contains(&k.as_str()) {
                let range = cm::Range::parse(want).map_err(|e| format!("{} {}: {e}", s.component, s.to))?;
                let after = p.steps.iter().find(|o| &o.component == k).map(|o| o.to.clone()).or_else(|| inst.versions.get(k).cloned());
                if !after.as_deref().is_some_and(|v| range.matches(v)) {
                    return Err(format!("{} {} needs {k} {} but this device would have {}: update {k} first", s.component, s.to, range.text(), after.map_or("no version on record".to_string(), |v| format!("{k} {v}"))));
                }
            } else if inst.extras.get(k) != Some(want) {
                return Err(format!("{} {} needs {k} = {want} but this device reports {}", s.component, s.to, inst.extras.get(k).map_or("nothing for it".to_string(), |v| format!("{v:?}"))));
            }
        }
    }
    Ok(())
}

/// Every staged file is still the file the signed list promises (read again from disk).
fn recheck_files(prep: &Prepared) -> Result<(), String> {
    for i in &prep.staged.items {
        let ok = std::fs::metadata(&i.file).is_ok_and(|m| m.is_file() && m.len() == i.step.size) && files::sha256_file(&i.file).is_ok_and(|h| h == i.step.sha256);
        if !ok {
            return Err(format!("the staged {} {} is not the file the signed list promises any more: fetch it again", i.step.component, i.step.to));
        }
    }
    Ok(())
}

/// Read back what the boot-partition parts wrote, against the SHA-256 of the signed list.
fn verify_boot(svc: &Svc, items: &[&comps::StagedItem], m: &up::Manifest) -> Result<(), String> {
    let vol = svc.centre.boot_read()?;
    for i in items {
        let path = &i.step.install_path;
        let got = if is_root_image(path) {
            // a lean install's system image goes to the data volume
            let id = up::root_id(m).ok_or("the system image has no usable id")?;
            files::sha256_file(&svc.data().join(iyakki_stage1::ROOTS_DIR).join(id).join("root.sfs"))?
        } else if path.eq_ignore_ascii_case("boot/grub/grub.cfg") {
            // the boot menu is rewritten with the previous-version entry; the engine reads it back as written
            continue;
        } else {
            let p = up::find_ci(&vol.root, path).ok_or_else(|| format!("{path} is not on the boot partition after the update"))?;
            files::sha256_file(&p)?
        };
        if got != i.step.sha256 {
            return Err(format!("{} ({path}) reads back different from the signed list after it was written", i.step.component));
        }
    }
    Ok(())
}

fn space_needed(prep: &Prepared, boot_root: Option<&Path>) -> u64 {
    let mut n: u64 = prep.staged.items.iter().map(|i| i.step.size).sum();
    if let Some(root) = boot_root {
        n += up::list_boot(root).values().sum::<u64>();
    }
    n.saturating_add(comps::DISK_MARGIN)
}

/// Apply the staged, verified parts. See the module text for what it refuses and how a failed check is undone.
pub fn apply_staged(svc: &Svc, prep: &Prepared, approval: Option<&Approval>) -> Result<Applied, String> {
    let rev = review(&prep.plan, Some(&prep.release));
    // 1. the person's yes, for exactly this plan
    let approval = approval.ok_or_else(|| format!("not applied: this needs the person's approval of the plan first.\n{}", rev.text))?;
    if approval.digest != rev.digest {
        return Err("not applied: the approval was given for a different plan than this one; show the plan again".into());
    }
    if prep.staged.items.is_empty() || prep.staged.items.len() != prep.plan.steps.len() || prep.staged.items.iter().zip(&prep.plan.steps).any(|(i, s)| i.step != *s) {
        return Err("not applied: the staged files are not the ones in the plan".into());
    }
    let by = approval.by().to_string();
    // 2. refusals that need nothing to be written
    if let Some(why) = pending(svc) {
        return Err(format!("not applied: {why}"));
    }
    let inst = svc.installed();
    recheck(&prep.plan, &inst).map_err(|e| format!("not applied: {e}"))?;
    recheck_files(prep).map_err(|e| format!("not applied: {e}"))?;
    let (boot, run): (Vec<&comps::StagedItem>, Vec<&comps::StagedItem>) = prep.staged.items.iter().partition(|i| is_boot_part(&i.step.install_path));
    let work = svc.work();
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let out = work.join("out");
    let mut lines = vec![];
    let mut boot_message = None;
    let mut rollback_boot = false;
    // 3. the boot partition's parts, through the Update centre's engine
    if !boot.is_empty() {
        // the root image carries /etc/iyakki-release: a label that is not the running one needs the base in this update, or the first
        // start after it would not match the label the boot menu waits for (and never be marked good)
        let running = svc.centre.versions().running;
        if up::compare_versions(&prep.release.version, &running) != std::cmp::Ordering::Equal && !prep.plan.steps.iter().any(|s| s.component == "base") {
            return Err(format!("not applied: the store labels these parts IYAKKI {} but this install runs {running} and the base system (which carries the label) is not part of this update. A partial update has to keep the label of the install it goes onto.", prep.release.version));
        }
        let vol = svc.centre.boot_read().map_err(|e| format!("not applied: {e}"))?;
        let need = space_needed(prep, Some(&vol.root));
        if let Some(free) = svc.free(&work).filter(|f| *f < need) {
            return Err(format!("not applied: not enough room to prepare the update: {} needed in the work folder, {} free", human(need), human(free)));
        }
        let sub = Staged { dir: prep.staged.dir.clone(), items: boot.iter().map(|i| (*i).clone()).collect() };
        let ho = comps::handover(&sub, &out, &prep.release, Some(&vol.root), &inst).map_err(|e| format!("not applied: {e}"))?;
        drop(vol);
        let payload = ho.payload.ok_or("not applied: no update payload was made")?;
        let result = (|| {
            let found = centre::read_pack(&payload, svc.centre.key())?;
            // the parts are signed by the store the person pinned and checked file by file; the payload itself carries no release signature
            let pv = svc.centre.preview(&found)?;
            if let Trust::Bad(e) = &found.trust {
                return Err(format!("the payload's signature is wrong: {e}"));
            }
            let other: Vec<&String> = pv.confirm.iter().filter(|c| !c.contains("no signature")).collect();
            if !other.is_empty() {
                return Err(format!("the update centre asks for a confirmation this apply does not give ({}); use Settings -> System update with a pack instead", other.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ")));
            }
            let msg = svc.centre.install(&found, true, &by)?;
            Ok::<_, String>((found, msg))
        })();
        let _ = std::fs::remove_dir_all(&payload);
        let (found, msg) = result.map_err(|e| format!("not applied: {e}"))?;
        rollback_boot = true;
        if let Some(f) = &svc.after_write {
            f();
        }
        if let Err(why) = verify_boot(svc, &boot, &found.manifest) {
            let back = svc.centre.rollback(&by);
            return Err(match back {
                Ok(m) => format!("a check after writing failed ({why}); the previous version was put back: {m} Nothing is recorded as installed."),
                Err(e) => format!("a check after writing failed ({why}) and the previous version could not be put back automatically ({e}); choose \"{}\" in the boot menu. Nothing is recorded as installed.", up::PREV_TITLE),
            });
        }
        lines.push(format!("Boot parts written and read back: {}.", boot.iter().map(|i| format!("{} {}", i.step.component, i.step.to)).collect::<Vec<_>>().join(", ")));
        boot_message = Some(msg);
    }
    // 4. the parts that are files of the running system, as a patch pack for the next start
    if !run.is_empty() {
        let r = apply_patch(svc, prep, &run, &inst, &out, &by);
        match r {
            Ok(l) => lines.extend(l),
            Err(e) => {
                let undone = if rollback_boot {
                    match svc.centre.rollback(&by) {
                        Ok(_) => " The boot parts written a moment ago were put back.".to_string(),
                        Err(e2) => format!(" The boot parts written a moment ago could not be put back automatically ({e2}); choose \"{}\" in the boot menu.", up::PREV_TITLE),
                    }
                } else {
                    String::new()
                };
                return Err(format!("the program patch failed ({e}); nothing is recorded as installed.{undone}"));
            }
        }
    }
    // 5. everything verified: now, and only now, the record
    let boot_id = svc.facts.boot_id.clone().unwrap_or_default();
    let items: Vec<ci::Applied1> = prep.staged.items.iter().map(|i| ci::Applied1 { component: i.step.component.clone(), version: i.step.to.clone(), sha256: i.step.sha256.clone(), file: i.step.install_path.starts_with('/').then(|| PathBuf::from(&i.step.install_path)), needs_restart: true }).collect();
    let installed = ci::record_applied(svc.data(), &items, &boot_id).map_err(|e| format!("the parts are in place but the record could not be saved ({e}); the next start corrects it from the system"))?;
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&prep.staged.dir);
    if let Some(m) = &boot_message {
        lines.push(m.clone());
    }
    lines.push(format!("Installed: {}. A restart is needed to run them; the computer does not restart by itself.", prep.staged.items.iter().map(|i| format!("{} {}", i.step.component, i.step.to)).collect::<Vec<_>>().join(", ")));
    Ok(Applied { lines, restart_machine: !boot.is_empty(), restart_core: !run.is_empty(), installed, boot_message })
}

fn apply_patch(svc: &Svc, prep: &Prepared, run: &[&comps::StagedItem], inst: &Installed, out: &Path, by: &str) -> Result<Vec<String>, String> {
    let sub = Staged { dir: prep.staged.dir.clone(), items: run.iter().map(|i| (*i).clone()).collect() };
    let ho = comps::handover(&sub, out, &prep.release, None, inst)?;
    let pack_file = ho.patch_pack.ok_or("no patch pack was made")?;
    let pack = patch::load(&pack_file)?;
    let m = &pack.manifest;
    let st = patch::staging_dir(svc.data(), &m.id);
    let _ = std::fs::remove_dir_all(&st);
    std::fs::create_dir_all(&st).map_err(|e| format!("{}: {e}", st.display()))?;
    for (i, b) in &pack.content {
        std::fs::write(st.join(patch::staged_name(*i)), b).map_err(|e| format!("{}: {e}", st.display()))?;
    }
    let dm = DevMode::open(svc.data());
    let data = svc.data().to_path_buf();
    let free = svc.work_free;
    let data_free = move || free.or_else(|| crate::sharedsite::statvfs_free(&data)).unwrap_or(u64::MAX);
    // the person approved this very plan; the patch machinery's wide paths (the program itself is one) are open for it
    let env = patch::Env { dm: &dm, live: svc.live.as_ref(), root: svc.patch_root.clone(), release: svc.centre.release_text().to_string(), wide: true, data_free: &data_free };
    let r = patch::apply(&env, m, patch::Mode::NextBoot, by);
    let _ = std::fs::remove_dir_all(&st);
    let v = r?;
    // read back: the patch is listed and waits for the next start
    let listed = patch::list(&env);
    let ok = listed["patches"].as_array().into_iter().flatten().any(|p| p["id"] == m.id.as_str() && p["status"] == "next start");
    if !ok {
        let _ = patch::remove(&env, &m.id, by);
        return Err("the patch layer was written but is not listed as waiting for the next start".into());
    }
    let _ = std::fs::remove_file(&pack_file);
    Ok(vec![v["message"].as_str().unwrap_or("The patch layer is stored.").to_string(), format!("Program parts stored as patch {}: {}.", m.id, run.iter().map(|i| format!("{} {}", i.step.component, i.step.to)).collect::<Vec<_>>().join(", "))])
}

// ------------------------------------------------------------------------------------------------ one call for the tool and Settings

/// One line per store for Settings: how many parts are newer than what is installed (from the last list fetched).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoreLine {
    pub store: String,
    pub newer: usize,
    pub checked_at: i64,
    pub error: String,
}

pub const LAST_CHECK_FILE: &str = "components-last-check.json";

pub fn save_check(data: &Path, lines: &[StoreLine]) {
    let p = data.join(LAST_CHECK_FILE);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_vec(lines).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub fn load_check(data: &Path) -> Vec<StoreLine> {
    std::fs::read(data.join(LAST_CHECK_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The sentence Settings shows for one store line.
pub fn check_sentence(l: &StoreLine) -> String {
    if !l.error.is_empty() {
        format!("Components from {}: could not check ({}).", l.store, l.error)
    } else if l.newer == 0 {
        format!("Components from {}: up to date.", l.store)
    } else {
        format!("Components from {}: {} newer.", l.store, l.newer)
    }
}

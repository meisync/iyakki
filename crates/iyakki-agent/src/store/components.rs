//! System components on the device (docs/development/COMPONENT-UPDATES-PLAN.md, "What a running OS does"): check the signed
//! `components.json` of a pinned private store, plan what to take (a delta from the installed version when the store has one for exactly
//! that file, else the whole file), refuse what the installed parts cannot support (naming the part that must go first), download into a
//! staging folder (resuming, hash-checked, never more than a small buffer in memory except where a delta is applied) and turn the verified
//! files into the inputs the existing machinery takes: an update payload (`update.json` + `boot/...`, what `dev update` and the Update
//! centre read) for the boot partition's parts, and a patch pack (`IYPATCH1`) for the parts that are files of the running system.
//!
//! **Nothing here applies anything to a running system.** The last step is "verified and staged, here is the plan". Applying is
//! `compapply.rs`; the record of what is installed (`components-installed.json`) is kept by `compinstalled.rs`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::compmanifest::{self as cm, Doc, Entry, Range, Restart};
use crate::devmode::{patch, update};

/// One installed store that may offer components: its base address and the key the person confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedStore {
    pub name: String,
    pub base: String,
    pub pin: String,
}

/// What this device has: versions, the SHA-256 and (when known) the file of each component's current artifact, and other facts a
/// `requires` may name (`abi`). Kept in `components-installed.json` in the data folder: written after a verified apply
/// (`compinstalled::record_applied`) and corrected from facts at boot (`compinstalled::correct`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Installed {
    pub versions: BTreeMap<String, String>,
    pub shas: BTreeMap<String, String>,
    pub files: BTreeMap<String, PathBuf>,
    pub extras: BTreeMap<String, String>,
    /// Where each version came from: `applied` (this device wrote it after a verified apply) or `observed` (read from the running system).
    pub how: BTreeMap<String, String>,
    /// Components written but not yet running (a restart is needed), with the boot id of the start in which they were written.
    pub awaiting: BTreeMap<String, String>,
    /// The release label (`VERSION=` of the release file) the last correction saw. The boot and base parts cannot be read back from the
    /// running system (only the label can), so an applied version of them stands while the label stays the same.
    pub release: String,
}

pub const INSTALLED_FILE: &str = "components-installed.json";

impl Installed {
    pub fn load(data: &Path) -> Installed {
        std::fs::read(data.join(INSTALLED_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, data: &Path) -> Result<(), String> {
        let p = data.join(INSTALLED_FILE);
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// Ask a store for its signed list and check it against the pinned key.
pub fn fetch_doc(up: &dyn iyakki_proxy::Upstream, store: &PinnedStore) -> Result<Doc, String> {
    let url = cm::url_for_catalog(&store.base);
    let r = up.request("GET", &url, &[], &[]).map_err(|e| format!("could not reach {}: {e}", store.name))?;
    match r.status {
        200 => cm::verify(&r.body, &store.pin),
        404 => Err(format!("{} does not offer system components (it has no /v1/components.json)", store.name)),
        s => Err(format!("{} answered {s} for its component list", store.name)),
    }
}

// ------------------------------------------------------------------------------------------------ the plan

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Full,
    Delta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub component: String,
    pub from: Option<String>,
    pub to: String,
    pub via: Via,
    /// What is downloaded (the delta or the whole file) and its SHA-256.
    pub blob_sha256: String,
    pub download: u64,
    /// The whole file the step ends with.
    pub sha256: String,
    pub size: u64,
    /// The full alternative when a delta was chosen (so a failed delta can fall back).
    pub full_download: u64,
    pub install_path: String,
    pub restart: Restart,
    pub requires: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    pub component: String,
    pub version: String,
    pub why: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub store: String,
    pub channel: String,
    pub serial: u64,
    pub steps: Vec<Step>,
    pub blocked: Vec<Blocked>,
    pub current: Vec<String>,
    pub notes: Vec<String>,
    /// The release label the store signed with the list (the payload's version and variant), when it gave one.
    pub release: Option<cm::ReleaseLabel>,
}

impl Plan {
    pub fn download_total(&self) -> u64 {
        self.steps.iter().map(|s| s.download).sum()
    }

    pub fn restart(&self) -> Option<Restart> {
        self.steps.iter().map(|s| s.restart).max_by_key(|r| matches!(r, Restart::Machine))
    }
}

pub fn human(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}

/// Why `e` cannot go onto a device whose parts are `overlay` (the installed versions with the planned ones put over them).
fn unmet(e: &Entry, overlay: &BTreeMap<String, String>, inst: &Installed, doc: &Doc, channel: &str) -> Option<String> {
    for (k, want) in &e.requires {
        if cm::COMPONENTS.contains(&k.as_str()) {
            let Ok(range) = Range::parse(want) else { return Some(format!("it asks for {k} {want}, which this device cannot read")) };
            match overlay.get(k) {
                Some(have) if range.matches(have) => {}
                got => {
                    let offered = doc.latest(k, channel).filter(|x| range.matches(&x.version));
                    let hint = match offered {
                        Some(x) => format!("{k} {} is in this store: update {k} first", x.version),
                        None => format!("no {k} in this store's {channel} channel is in that range yet: update {k} first when one is published"),
                    };
                    let has = match got {
                        None => format!("no {k} on record"),
                        Some(v) if inst.versions.get(k) == Some(v) => format!("{k} {v}"),
                        Some(v) => format!("{k} {v} once the planned update is in"),
                    };
                    return Some(format!("{} {} needs {k} {} but this device has {has}; {hint}", e.component, e.version, range.text()));
                }
            }
        } else if inst.extras.get(k) != Some(want) {
            return Some(format!("{} {} needs {k} = {want} but this device reports {}", e.component, e.version, inst.extras.get(k).map_or("nothing for it".to_string(), |v| format!("{v:?}"))));
        }
    }
    None
}

/// Decide what to take from `doc` for `channel`. `last_serial` is the newest list this device saw from the store: an older one is refused.
pub fn plan(doc: &Doc, store: &str, channel: &str, inst: &Installed, last_serial: Option<u64>) -> Result<Plan, String> {
    if last_serial.is_some_and(|s| doc.serial < s) {
        return Err(format!("{store} sent an older component list (serial {}) than the one this device saw before ({}): refused", doc.serial, last_serial.unwrap_or(0)));
    }
    let mut p = Plan { store: store.to_string(), channel: channel.to_string(), serial: doc.serial, release: doc.release.clone(), ..Default::default() };
    let mut cand: Vec<&Entry> = vec![];
    for c in cm::COMPONENTS {
        let Some(e) = doc.latest(c, channel) else { continue };
        if let Some(sha) = inst.shas.get(c) {
            if doc.yanked.iter().any(|y| y.component == c && &y.sha256 == sha) {
                p.notes.push(format!("The {c} on this device ({}) was withdrawn by the store: {}.", inst.versions.get(c).map_or("?", String::as_str), doc.yanked.iter().find(|y| y.component == c && &y.sha256 == sha).map_or("", |y| y.reason.as_str())));
            }
        }
        match inst.versions.get(c) {
            Some(v) if compare(&e.version, v) != std::cmp::Ordering::Greater => p.current.push(format!("{c} {v}")),
            _ => cand.push(e),
        }
    }
    // refuse what the parts that will be there cannot support, until nothing more drops out (a refused part may be what another needs)
    loop {
        let mut overlay = inst.versions.clone();
        for e in &cand {
            overlay.insert(e.component.clone(), e.version.clone());
        }
        let bad: Vec<(String, String)> = cand.iter().filter_map(|e| unmet(e, &overlay, inst, doc, channel).map(|w| (e.component.clone(), w))).collect();
        if bad.is_empty() {
            break;
        }
        for (c, why) in bad {
            if let Some(i) = cand.iter().position(|e| e.component == c) {
                let e = cand.remove(i);
                p.blocked.push(Blocked { component: c, version: e.version.clone(), why });
            }
        }
    }
    for e in &cand {
        let installed_sha = inst.shas.get(&e.component);
        let delta = e.delta.iter().find(|d| Some(&d.from_sha256) == installed_sha && d.size < e.size && inst.files.get(&e.component).is_some_and(|f| f.is_file()));
        let (via, blob, download) = match delta {
            Some(d) => (Via::Delta, d.sha256.clone(), d.size),
            None => (Via::Full, e.sha256.clone(), e.size),
        };
        let install_path = if e.install_path.is_empty() { cm::default_install_path(&e.component).unwrap_or("").to_string() } else { e.install_path.clone() };
        p.steps.push(Step { component: e.component.clone(), from: inst.versions.get(&e.component).cloned(), to: e.version.clone(), via, blob_sha256: blob, download, sha256: e.sha256.clone(), size: e.size, full_download: e.size, install_path, restart: cm::restart_of(&e.component), requires: e.requires.clone() });
    }
    order(&mut p.steps);
    Ok(p)
}

fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    update::compare_versions(a, b)
}

/// A part another planned part needs comes first (the requirement is not met by the installed version); otherwise the table's order.
fn order(steps: &mut Vec<Step>) {
    let mut rest: Vec<Step> = std::mem::take(steps);
    while !rest.is_empty() {
        let i = rest.iter().position(|s| !s.requires.keys().any(|k| rest.iter().any(|o| &o.component == k && o.component != s.component))).unwrap_or(0);
        steps.push(rest.remove(i));
    }
}

/// The plan in plain words, for the person: what changes, how big, whether a restart is needed. Nothing has been fetched yet.
pub fn plan_text(p: &Plan) -> String {
    let mut out = vec![format!("System components from {} (channel {}):", p.store, p.channel)];
    for s in &p.steps {
        let how = match s.via {
            Via::Delta => format!("only the difference, {} (the whole file is {})", human(s.download), human(s.full_download)),
            Via::Full => format!("the whole file, {}", human(s.download)),
        };
        let after = match s.restart {
            Restart::Machine => "the machine restarts to use it",
            Restart::Services => "IYAKKI's services restart, not the machine",
        };
        out.push(format!("  {} {} -> {}: {how}; {after}.", s.component, s.from.as_deref().unwrap_or("(not on record)"), s.to));
    }
    if p.steps.is_empty() {
        out.push("  Nothing to update.".into());
    } else {
        out.push(format!("Download: {} in all. {}", human(p.download_total()), match p.restart() {
            Some(Restart::Machine) => "A restart of the machine is needed; the version you run now is kept to go back to.",
            Some(Restart::Services) => "No machine restart is needed, only IYAKKI's services.",
            None => "",
        }));
    }
    for b in &p.blocked {
        out.push(format!("Not offered: {}", b.why));
    }
    if !p.current.is_empty() {
        out.push(format!("Up to date: {}.", p.current.join(", ")));
    }
    out.extend(p.notes.iter().cloned());
    out.push("Nothing has been downloaded or installed.".into());
    out.join("\n")
}

// ------------------------------------------------------------------------------------------------ download

const BUF: usize = 64 << 10;
/// Room kept free beyond a download.
pub const DISK_MARGIN: u64 = 64 << 20;

fn sha_of_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; BUF];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(crate::pkg::hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

/// Download `<base>/v1/blobs/<sha>` into `dir/<sha>` (resuming `dir/<sha>.part`), checked against `sha` and `size`. A verified file that is
/// already there is reused. `retries` more tries after a network problem, each continuing where the file ends.
#[allow(clippy::too_many_arguments)]
pub fn download_blob(st: &dyn iyakki_proxy::Streamer, base: &str, sha: &str, size: u64, dir: &Path, retries: u32, progress: &mut dyn FnMut(u64, u64), cancel: &dyn Fn() -> bool) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (done, part) = (dir.join(sha), dir.join(format!("{sha}.part")));
    if std::fs::metadata(&done).is_ok_and(|m| m.len() == size) && sha_of_file(&done).is_ok_and(|h| h == sha) {
        progress(size, size);
        return Ok(done);
    }
    let _ = std::fs::remove_file(&done);
    let url = format!("{}/v1/blobs/{sha}", cm::base_of(base));
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    if let Some(free) = crate::sharedsite::statvfs_free(dir) {
        let need = (size - have).saturating_add(DISK_MARGIN);
        if free < need {
            return Err(format!("not enough room to download {}: {} needed, {} free", human(size), human(need), human(free)));
        }
    }
    let mut last_err = String::new();
    for attempt in 0..=retries {
        have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have == size {
            break;
        }
        let mut headers = vec![("Accept-Encoding".to_string(), "identity".to_string())];
        if have > 0 {
            headers.push(("Range".to_string(), format!("bytes={have}-")));
        }
        let head = match st.open(&url, &headers) {
            Ok(h) => h,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        match head.status {
            200 => have = 0,
            206 => {
                let ok = head.header("content-range").and_then(|c| c.strip_prefix("bytes ")).and_then(|c| c.split('-').next()).and_then(|a| a.trim().parse::<u64>().ok()) == Some(have);
                if !ok {
                    let _ = std::fs::remove_file(&part);
                    last_err = "the store answered a different part than asked for; starting again".into();
                    continue;
                }
            }
            404 => return Err("the store no longer has this file (it may have been withdrawn or cleaned up); check for a newer list".into()),
            s => return Err(format!("the store answered {s} for the file")),
        }
        let mut f = if have == 0 { std::fs::File::create(&part) } else { std::fs::OpenOptions::new().append(true).open(&part) }.map_err(|e| format!("{}: {e}", part.display()))?;
        let mut body = head.body;
        let mut buf = vec![0u8; BUF];
        let mut broke = false;
        loop {
            if cancel() {
                return Err("cancelled; what was downloaded is kept and the next try continues from it".into());
            }
            match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if have + n as u64 > size {
                        drop(f);
                        let _ = std::fs::remove_file(&part);
                        return Err("the store sent more bytes than the list says: the file was discarded".into());
                    }
                    f.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
                    have += n as u64;
                    progress(have, size);
                }
                Err(e) => {
                    last_err = e.to_string();
                    broke = true;
                    break;
                }
            }
        }
        let _ = f.flush();
        drop(f);
        if !broke && have == size {
            break;
        }
        if !broke {
            last_err = format!("the connection ended after {} of {}", human(have), human(size));
        }
        let _ = attempt;
    }
    let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if got != size {
        return Err(format!("{last_err}. {} of {} are kept; run it again to continue", human(got), human(size)));
    }
    if sha_of_file(&part)? != sha {
        let _ = std::fs::remove_file(&part);
        return Err("the downloaded file does not match its SHA-256 in the signed list: it was removed (the store or the connection delivered something else)".into());
    }
    std::fs::rename(&part, &done).map_err(|e| format!("{}: {e}", done.display()))?;
    Ok(done)
}

#[derive(Clone, Debug)]
pub struct StagedItem {
    pub step: Step,
    /// The whole artifact, its SHA-256 and size checked.
    pub file: PathBuf,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct Staged {
    pub dir: PathBuf,
    pub items: Vec<StagedItem>,
}

/// Fetch every step of `plan` into `dir`. A delta is applied to the installed file with `bindelta::apply_delta_files` (streamed; its checks: the old file
/// must be the one it was made against, the result must match the hash); when that fails the whole file is fetched instead.
pub fn stage(plan: &Plan, base: &str, st: &dyn iyakki_proxy::Streamer, inst: &Installed, dir: &Path, retries: u32, progress: &mut dyn FnMut(&str, u64, u64), cancel: &dyn Fn() -> bool) -> Result<Staged, String> {
    let mut staged = Staged { dir: dir.to_path_buf(), items: vec![] };
    for s in &plan.steps {
        let mut note = String::new();
        let mut file = None;
        if s.via == Via::Delta {
            let d = download_blob(st, base, &s.blob_sha256, s.download, dir, retries, &mut |a, b| progress(&s.component, a, b), cancel)?;
            match apply_delta_step(s, &d, inst, dir) {
                Ok(f) => {
                    let _ = std::fs::remove_file(&d);
                    note = format!("made from the installed {} and a {} difference", s.component, human(s.download));
                    file = Some(f);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&d);
                    note = format!("the difference could not be used ({e}); the whole file was fetched instead");
                }
            }
        }
        let file = match file {
            Some(f) => f,
            None => download_blob(st, base, &s.sha256, s.size, dir, retries, &mut |a, b| progress(&s.component, a, b), cancel)?,
        };
        staged.items.push(StagedItem { step: s.clone(), file, note });
    }
    Ok(staged)
}

fn apply_delta_step(s: &Step, delta_file: &Path, inst: &Installed, dir: &Path) -> Result<PathBuf, String> {
    let old_path = inst.files.get(&s.component).ok_or("the installed file is not known")?;
    let old_len = std::fs::metadata(old_path).map_err(|e| format!("{}: {e}", old_path.display()))?.len();
    if old_len > crate::bindelta::MAX_FILE || s.size > crate::bindelta::MAX_FILE {
        return Err(format!("larger than the {} MB a difference may make", crate::bindelta::MAX_FILE >> 20));
    }
    if let Some(free) = crate::sharedsite::statvfs_free(dir) {
        if free < s.size.saturating_add(DISK_MARGIN) {
            return Err(format!("not enough room to make the new file ({} needed, {} free)", human(s.size.saturating_add(DISK_MARGIN)), human(free)));
        }
    }
    // streamed: the old file is read where the difference points, the result goes to a temporary file with its hash computed on the way
    let out = dir.join(&s.sha256);
    let info = crate::bindelta::apply_delta_files(old_path, delta_file, &out)?;
    if info.new_len != s.size || info.new_sha256 != s.sha256 {
        let _ = std::fs::remove_file(&out);
        return Err("the result is not the file the signed list promises".into());
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ hand-over

/// What the staged parts are labelled with when they become an update payload (the IYAKKI release they belong to).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub variant: String,
}

impl From<&cm::ReleaseLabel> for Release {
    fn from(r: &cm::ReleaseLabel) -> Release {
        Release { version: r.version.clone(), variant: r.variant.clone() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Handover {
    /// A folder `dev update` / the Update centre read (`update.json` + `boot/...`): the boot partition's parts.
    pub payload: Option<PathBuf>,
    /// A patch pack (`iyakki dev patch`): the parts that are files of the running system.
    pub patch_pack: Option<PathBuf>,
    pub lines: Vec<String>,
}

fn link_or_copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let _ = std::fs::remove_file(to);
    std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ())).map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))
}

fn safe_id(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' }).collect()
}

/// Turn verified staged parts into update inputs under `out`. `rest_from` is a folder laid out like the installed boot partition (`boot/...`);
/// the parts that are not being replaced are taken from it so the payload is a whole boot set. `installed_files` (component -> installed file)
/// lets a patch pack carry a delta instead of a large file.
pub fn handover(staged: &Staged, out: &Path, release: &Release, rest_from: Option<&Path>, inst: &Installed) -> Result<Handover, String> {
    handover_with_root(staged, out, release, rest_from, None, inst)
}

/// `handover`, with `active_root`: the system image that is really running (on a lean install it lives on the data volume, and the boot
/// partition may still hold an OLD copy of it from the first install). When this update has no base part, the payload's root image is
/// that file, never the stale one from the boot partition: a payload carrying the stale image would switch the next start back to it.
pub fn handover_with_root(staged: &Staged, out: &Path, release: &Release, rest_from: Option<&Path>, active_root: Option<&Path>, inst: &Installed) -> Result<Handover, String> {
    let mut h = Handover::default();
    let (boot, run): (Vec<&StagedItem>, Vec<&StagedItem>) = staged.items.iter().partition(|i| !i.step.install_path.starts_with('/'));
    for i in &staged.items {
        if i.step.install_path.is_empty() {
            return Err(format!("{} has no install path in the store's list: it cannot be handed over", i.step.component));
        }
    }
    if !boot.is_empty() {
        let dir = out.join(format!("update-{}", safe_id(&release.version)));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for i in &boot {
            link_or_copy(&i.file, &dir.join(&i.step.install_path))?;
        }
        if let Some(rest) = rest_from {
            for (rel, _) in update::list_boot(rest).into_iter().filter(|(r, _)| update::valid_path(r)) {
                if !boot.iter().any(|i| i.step.install_path.eq_ignore_ascii_case(&rel)) {
                    link_or_copy(&rest.join(&rel), &dir.join(&rel))?;
                }
            }
        }
        if let Some(root) = active_root {
            let has_base = boot.iter().any(|i| i.step.install_path.eq_ignore_ascii_case("boot/root.sfs"));
            if !has_base {
                let dest = update::find_ci(&dir, "boot/root.sfs").unwrap_or_else(|| dir.join("boot/root.sfs"));
                if dest.exists() {
                    std::fs::remove_file(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                }
                link_or_copy(root, &dest)?;
            }
        }
        // the release file of the boot partition says which IYAKKI these files are: it follows the label the payload is made as (a new file:
        // the one taken from the installed partition may be a hard link to it)
        if !boot.iter().any(|i| i.step.install_path.eq_ignore_ascii_case("boot/iyakki-release")) {
            if let Some(p) = update::find_ci(&dir, "boot/iyakki-release") {
                let old = std::fs::read_to_string(&p).unwrap_or_default();
                let mut text = format!("VERSION={}\nVARIANT={}\n", release.version, release.variant);
                for l in old.lines().filter(|l| !l.starts_with("VERSION=") && !l.starts_with("VARIANT=")) {
                    text.push_str(l);
                    text.push('\n');
                }
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
        let mut files = vec![];
        for (rel, size) in update::list_boot(&dir).into_iter().filter(|(r, _)| update::valid_path(r)) {
            files.push((rel.clone(), size, crate::devmode::files::sha256_file(&dir.join(&rel))?));
        }
        let m = update::build_manifest(&dir, &files, Some(&release.version), Some(&release.variant), "components").map_err(|e| format!("{e} (give --rest-from: the installed boot files the payload completes)"))?;
        update::write_manifest(&dir, &m)?;
        update::verify_payload(&dir, &m)?;
        h.lines.push(format!("Update payload for IYAKKI {} ({}): {} files in {}; install it with `iyakki dev update` or put it on a stick for Settings -> System update. Parts replaced: {}.", m.version, m.variant, m.files.len(), dir.display(), boot.iter().map(|i| format!("{} {}", i.step.component, i.step.to)).collect::<Vec<_>>().join(", ")));
        h.payload = Some(dir);
    }
    if !run.is_empty() {
        let id = format!("component-{}", safe_id(&run.iter().map(|i| format!("{}-{}", i.step.component, i.step.to)).collect::<Vec<_>>().join("-")));
        let build = out.join(format!("{id}-build"));
        let old = out.join(format!("{id}-old"));
        for d in [&build, &old] {
            if d.exists() {
                std::fs::remove_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
        }
        let mut ops = vec![];
        for i in &run {
            let rel = i.step.install_path.trim_start_matches('/');
            link_or_copy(&i.file, &build.join("files").join(rel))?;
            if let Some(f) = inst.files.get(&i.step.component).filter(|f| f.is_file()) {
                link_or_copy(f, &old.join(rel))?;
            }
            ops.push(json!({"path": i.step.install_path, "action": "replace", "mode": "0755"}));
        }
        let title: String = format!("System components: {}", run.iter().map(|i| format!("{} {}", i.step.component, i.step.to)).collect::<Vec<_>>().join(", ")).chars().take(120).collect();
        let manifest: Value = json!({"format": patch::FORMAT, "id": id.chars().take(64).collect::<String>(), "title": title, "description": "Made from signed component files of a private store.", "base": "current", "ops": ops});
        std::fs::write(build.join(patch::MANIFEST), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(|e| e.to_string())?;
        let bytes = patch::build_with(&build, old.is_dir().then_some(old.as_path())).map_err(|e| format!("the patch pack for {} could not be made: {e}", run.iter().map(|i| i.step.component.as_str()).collect::<Vec<_>>().join(", ")))?;
        let pack = patch::decode(&bytes)?;
        let file = out.join(format!("{}.iypatch", id.chars().take(64).collect::<String>()));
        std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        let _ = std::fs::remove_dir_all(&build);
        let _ = std::fs::remove_dir_all(&old);
        h.lines.push(format!("Patch pack {} ({}): apply with `iyakki dev patch`. {}", file.display(), human(bytes.len() as u64), if pack.manifest.needs_reboot() { "It replaces a program the system only reloads at the next start, so it needs the shell scope and the next restart (the patch machinery treats this path as restart-only; a service-only swap is a later step)." } else { "It can be applied without a restart." }));
        h.patch_pack = Some(file);
    }
    Ok(h)
}

/// The text after staging: verified, where, and what the person does next. Nothing was applied.
pub fn staged_text(staged: &Staged, h: &Handover) -> String {
    let mut out = vec![format!("Verified and staged in {}:", staged.dir.display())];
    for i in &staged.items {
        out.push(format!("  {} {}: {}, SHA-256 {} checked{}", i.step.component, i.step.to, human(i.step.size), &i.step.sha256[..12], if i.note.is_empty() { String::new() } else { format!(" ({})", i.note) }));
    }
    out.extend(h.lines.iter().cloned());
    out.push("Nothing was applied to this system.".into());
    out.join("\n")
}

//! "Hardware firmware" (Setup item `fw-hardware`): the vendor firmware files THIS computer asks for and does not have, fetched from the
//! upstream linux-firmware release at the person's click. The images carry none of them (`dist/buildroot/configs/iyakki_no_firmware.fragment`,
//! `docs/FIRMWARE-LICENCES.md`); nothing is downloaded, copied or installed before the person has read the licence list and clicked.
//!
//! 1. **Detect** ([`parse_log`], [`read_devs`], [`build_plan`]): the kernel log says which files the drivers asked for ("Direct firmware load
//!    for X failed", "firmware: failed to load X", ...); PCI and USB ids say which card that is. Pure functions with fixtures.
//! 2. **Source**: a pinned release of the linux-firmware repository ([`manifest`]: tag, commit, size and SHA-256 of every file, the SHA-256 of
//!    the whole snapshot tarball; made by `dist/scripts/firmware-notes/pin.sh`). Only the needed files and their licence texts are fetched,
//!    one by one from `git.kernel.org` at the pinned commit (https only, redirects to other hosts refused, resumable, every file checked).
//!    Other ways to get the same bytes: a folder or USB stick (a `firmware-extra` folder, an unpacked release, or the release tarball),
//!    the staging folder the developer-mode link fills, or the internet of another computer over a cable (`fwextra_net.rs`).
//! 3. **Install** ([`run`]): the files, their licence texts and the WHENCE entries become the driver pack `fw-hardware` on the data volume
//!    (`iyakki_core_local::driver_packs::apply`: linked into `/lib/firmware` at every boot, the mechanism the other firmware packs use). Drivers that are
//!    safe to reload are reloaded (never the graphics driver); otherwise the result says "restart needed" and the tile offers a Restart
//!    button. Nothing restarts by itself. [`remove`] takes the pack away.
//!
//! The world is reached only through [`Env`]; the tests give it a fake.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use iyakki_core_local::driver_packs::pack::{self};
use iyakki_core_local::driver_packs::apply;

use crate::fwpack::{ModState, Outcome};
pub use iyakki_shared::fwextra::*;
pub use iyakki_shared::fwextra::{TAG};


// ------------------------------------------------------------------------------------------------ the manifest










// ------------------------------------------------------------------------------------------------ detection: the kernel log










// ------------------------------------------------------------------------------------------------ detection: PCI and USB ids











// ------------------------------------------------------------------------------------------------ vendors and licences







// ------------------------------------------------------------------------------------------------ the plan











// ------------------------------------------------------------------------------------------------ acceptance









// ------------------------------------------------------------------------------------------------ folder and tarball sources














/// Take the wanted files out of the release tarball `tar_gz` while it is decompressed. `want` gets the path inside the release.
pub fn pick_from_tarball(tar_gz: &Path, want: &dyn Fn(&str) -> bool) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut p = TarPick { prefix: format!("linux-firmware-{TAG}/"), want, max: 64 << 20, head: Vec::new(), left: 0, pad: 0, kind: 0, name: String::new(), capture: None, long: None, pax: None, got: BTreeMap::new(), failed: None };
    crate::archive::gunzip_to_writer(tar_gz, &mut p, 8 << 30)?;
    Ok(p.got)
}

// ------------------------------------------------------------------------------------------------ the job








/// Run the job: detect, check the acceptance, get the bytes from `source`, verify, install, decide about the restart.
pub fn run(env: &mut dyn Env, data: &Path, source: &Source, opts: &Opts) -> Result<Outcome, String> {
    run_with(env, manifest(), data, source, opts)
}

pub fn run_with(env: &mut dyn Env, m: &Manifest, data: &Path, source: &Source, opts: &Opts) -> Result<Outcome, String> {
    let mut lines = Vec::new();
    match run_inner(env, m, data, source, opts, &mut lines) {
        Ok(o) => Ok(o),
        Err(e) => {
            let o = Outcome { at: env.now(), headline: format!("Not done: {e}"), lines, source: source.name().into(), ..Default::default() };
            write_outcome(data, &o);
            env.audit(&format!("firmware.fail {e}"));
            Err(e)
        }
    }
}

fn run_inner(env: &mut dyn Env, m: &Manifest, data: &Path, source: &Source, opts: &Opts, lines: &mut Vec<String>) -> Result<Outcome, String> {
    note(env, lines, format!("[0/6] Hardware firmware, linux-firmware {} (commit {}). {NOTICE}", m.tag, &m.commit[..10]));
    let plan = plan_in(m, env, data);
    for l in plan.lines() {
        note(env, lines, l);
    }
    let sel = plan.select(&opts.with);
    if sel.files.is_empty() {
        return Err("no missing firmware file was found in the kernel log (or only a separate group, which you did not include); nothing to do".into());
    }
    note(env, lines, format!("[1/6] {} file(s) in the release, {} KB with the licence texts", sel.unique().len(), sel.bytes(m) >> 10));
    if sel.bytes(m) > MAX_PLAN_BYTES {
        return Err(format!("the list is {} MB, more than the {} MB this job allows; nothing was fetched", sel.bytes(m) >> 20, MAX_PLAN_BYTES >> 20));
    }
    let Some(acc) = accepted_in(m, data, &sel) else {
        return Err("the licence list was not accepted (or the list changed since you read it). Open the card, read the licences and accept; nothing was downloaded or installed".into());
    };
    note(env, lines, format!("[2/6] licences accepted by {} (list {})", acc.by, &acc.digest[..16]));
    env.audit(&format!("firmware.start source={} files={} list={}", source.name(), sel.unique().len(), &acc.digest[..16]));
    if env.data_in_memory() {
        return Err("the data volume is in memory (a live session): the firmware would be gone after a restart. Start from the installed system".into());
    }
    let dl = download_dir(data);
    std::fs::create_dir_all(&dl).map_err(|e| format!("{}: {e}", dl.display()))?;
    let need = sel.bytes(m).saturating_mul(3) + NEEDED_FREE;
    match env.free_bytes(data) {
        Some(f) if f < need => return Err(format!("the data volume has {} MB free; this needs about {} MB", f >> 20, need >> 20)),
        Some(f) => note(env, lines, format!("free space on the data volume: {} MB", f >> 20)),
        None => note(env, lines, "note: the free space of the data volume could not be read".into()),
    }

    // 3. the bytes, every file checked against the pinned size and sha256
    let uniq = sel.unique();
    let docs = sel.docs(m);
    let mut got: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    match source {
        Source::Download => {
            note(env, lines, format!("[3/6] downloading {} file(s) and {} licence text(s) from the linux-firmware repository (git.kernel.org), commit {}. A stopped download resumes", uniq.len(), docs.len(), &m.commit[..10]));
            let all: Vec<(&String, &(u64, String))> = docs.iter().chain(uniq.iter()).collect();
            for (p, (size, sha)) in all {
                let dest = dl.join(p);
                if let Some(d) = dest.parent() {
                    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                }
                env.fetch(&m.url(p), *size, &dest)?;
                let bytes = std::fs::read(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                if let Err(e) = check_bytes(p, *size, sha, &bytes) {
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!("{e}. The file was deleted; the upstream file may have changed or the download was damaged. Nothing was installed"));
                }
                got.insert(p.clone(), bytes);
            }
            note(env, lines, format!("  all {} files match the pinned checksums", got.len()));
        }
        Source::Folder(dir) => {
            note(env, lines, format!("[3/6] reading the files from {}", dir.display()));
            let scan = scan_folder(dir);
            for (p, why) in scan.rejected.iter().take(10) {
                note(env, lines, format!("  ignored {p}: {why}"));
            }
            let mut missing: Vec<String> = Vec::new();
            for (p, (size, sha)) in docs.iter().chain(uniq.iter()) {
                match find_in_folder(dir, p) {
                    Some(f) => {
                        let bytes = std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                        check_bytes(p, *size, sha, &bytes).map_err(|e| format!("{}: {e}. Nothing was installed (only the exact files of linux-firmware {} are accepted)", f.display(), m.tag))?;
                        got.insert(p.clone(), bytes);
                    }
                    None => missing.push(p.clone()),
                }
            }
            if !missing.is_empty() {
                if let Some(t) = &scan.tarball {
                    note(env, lines, format!("  reading {} (checking the whole file first: this takes a while)", t.display()));
                    let sum = crate::fwpack::file_sha256(t)?;
                    if sum != m.snapshot_sha256 {
                        return Err(format!("{} has sha256 {sum}, not the pinned {}; it is not the release this version expects. Nothing was installed", t.display(), m.snapshot_sha256));
                    }
                    let want: BTreeSet<&String> = missing.iter().collect();
                    let picked = pick_from_tarball(t, &|p| want.contains(&p.to_string()))?;
                    for (p, bytes) in picked {
                        let (size, sha) = docs.get(&p).or_else(|| uniq.get(&p)).cloned().ok_or("an unexpected file in the tarball")?;
                        check_bytes(&p, size, &sha, &bytes)?;
                        got.insert(p, bytes);
                    }
                    missing.retain(|p| !got.contains_key(p));
                }
            }
            if !missing.is_empty() {
                let only_docs = missing.iter().all(|p| docs.contains_key(p));
                if only_docs {
                    note(env, lines, format!("  note: {} licence text(s) are not in the folder ({}); they stay on the vendor's repository", missing.len(), missing.join(", ")));
                } else {
                    return Err(format!("{} file(s) the list needs are not in {} (for example {}); nothing was installed. Make the folder with dist/scripts/firmware-notes/fetch.sh on a computer that has network", missing.len(), dir.display(), missing[0]));
                }
            }
            note(env, lines, format!("  {} files found and checked", got.len()));
        }
    }

    // 4. the pack: the files under the names the drivers ask for (and the release path), licence texts, the WHENCE entries
    note(env, lines, "[4/6] installing as a driver pack on the data volume (linked into /lib/firmware at every start)".into());
    let paths: Vec<String> = sel.files.iter().flat_map(|f| [f.requested.clone(), f.path.clone()]).collect();
    let whence = got.get("WHENCE").map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    let p = build_pack(m, &sel, &got, &whence_excerpt(&whence, &paths), &reload_list(env, &sel));
    let src = match source {
        Source::Download => format!("linux-firmware {} (commit {}, git.kernel.org), downloaded on request", m.tag, m.commit),
        Source::Folder(_) => format!("a folder, files checked against linux-firmware {} (commit {})", m.tag, m.commit),
    };
    let rep = env.install(&format!("linux-firmware {}", m.tag), &src, &p)?;
    for l in &rep.lines {
        note(env, lines, format!("  {l}"));
    }
    let _ = std::fs::remove_dir_all(&dl);

    // 5. restart?
    note(env, lines, "[5/6] checking whether the drivers picked the files up".into());
    let mut why: Vec<String> = Vec::new();
    for f in &sel.files {
        if GPU_DRIVERS.contains(&f.driver.as_str()) && !why.iter().any(|w| w.contains(&f.driver)) {
            why.push(format!("{} may drive the screen, so it is not reloaded while you use it", f.driver));
        } else if f.driver.is_empty() && matches!(family_of(&f.path).0.as_str(), "intel-gpu" | "amd-gpu" | "nvidia") && !why.iter().any(|w| w.contains("graphics")) {
            why.push("graphics firmware is read when the graphics driver starts".into());
        }
    }
    for d in sel.files.iter().map(|f| f.driver.as_str()).filter(|d| !d.is_empty()).collect::<BTreeSet<_>>() {
        if env.module_state(d) == ModState::Builtin {
            why.push(format!("{d} is built into the kernel and reads its firmware only at start"));
        }
    }
    if rep.lines.iter().any(|l| l.contains("did not load")) {
        why.push("a driver did not load again".into());
    }
    let reboot = !why.is_empty();
    let headline = if reboot { "Hardware firmware installed. Restart to finish.".to_string() } else { "Hardware firmware installed and the drivers were loaded again.".to_string() };
    let mut all = lines.clone();
    for w in &why {
        all.push(format!("restart wanted: {w}"));
    }
    let o = Outcome { at: env.now(), installed: true, files: uniq.len(), source: source.name().into(), reboot_needed: reboot, headline, lines: all };
    write_outcome(data, &o);
    env.audit(&format!("firmware.installed files={} restart={}", uniq.len(), reboot));
    env.say(&format!("[6/6] done: {}", o.headline));
    Ok(o)
}

/// The modules that are safe to unload and load again: the drivers that asked, loaded as modules, and not a graphics driver.
/// Core modules a kernel log line names as the "driver" of a firmware request without being the transport that loads it: reloading
/// them tears down every controller of that kind. `bluetooth` is the "Bluetooth: hci0: ..." prefix of a Bluetooth firmware line (the
/// MediaTek patch on a Lenovo); reloading the core on a Mac whose Broadcom chip runs its own firmware re-probed and wedged that chip
/// (2026-11-08). The transport driver (btusb, hci_uart) is what a file needs, and it is named by its own lines when it is one.
pub use pack::CORE_MODULES;




// ------------------------------------------------------------------------------------------------ remove





// ------------------------------------------------------------------------------------------------ the real machine

pub struct RealEnv {
    pub data: PathBuf,
    pub streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>,
    pub sys: PathBuf,
}

impl RealEnv {
    pub fn new(data: &Path, streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>) -> RealEnv {
        RealEnv { data: data.to_path_buf(), streamer, sys: iyakki_core_local::kernelfacts::SYS.into() }
    }
}

impl Env for RealEnv {
    fn say(&mut self, line: &str) {
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
    fn audit(&mut self, line: &str) {
        // the job runs as a process of its own: the audit line goes to the job log (the tile copies it to the audit log)
        crate::qprintln!("audit: {line}");
    }
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        crate::k3sdev::space(dir).map(|s| s.0)
    }
    fn data_in_memory(&self) -> bool {
        crate::macfw::in_memory(&iyakki_core_local::kernelfacts::mounts_text(), &self.data)
    }
    fn kernel_log(&self) -> String {
        iyakki_core_local::starters::dmesg()
    }
    fn devs(&self) -> Vec<Dev> {
        read_devs(&self.sys)
    }
    fn fetch(&mut self, url: &str, want: u64, dest: &Path) -> Result<(), String> {
        let st = self.streamer.clone();
        crate::fwpack::fetch_resume(&*st, url, want, dest, 5, std::time::Duration::from_secs(3), &mut |l| self.say(l), &|| false)
    }
    fn install(&mut self, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        let (fs, real) = apply::Fs::for_data(&self.data);
        let run = iyakki_core_local::kmodpack::runner(real);
        crate::fwpack::install_checked(&fs, run.as_ref(), PACK_ID, version, "LicenseRef-linux-firmware-various-per-file", source, p)
    }
    fn module_state(&self, name: &str) -> ModState {
        crate::fwpack::module_state_in(&self.sys, name)
    }
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
}

#[cfg(test)]
#[path = "fwextra_tests.rs"]
mod tests;

#[cfg(test)]
use iyakki_core_local::kmodpack::sha256_hex;

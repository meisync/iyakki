//! The boot loader's menu file (`EFI/iyakki-boot/boot.cfg`) must name RAM disks that exist on the boot volume.
//!
//! A lean stick has `boot/initrd-lean` and no `boot/initrd`; a full one has `boot/initrd`. GRUB picks whichever is there (the lean block of
//! `grub.cfg`), but the loader reads exactly the file a row names, so a `boot.cfg` written for the other kind of volume (an update pack made
//! from a full image put on a lean stick: found 2026-11-12, "initrd /boot/initrd cannot be read", KI-200) starts nothing. This module is the one
//! place that decides which RAM disk a row should name ([`fix_text`]) and repairs a volume ([`fix_volume`]); every writer of `boot.cfg` and the
//! lean strip step call it. Pure text and plain file reads: nothing here touches the base OS.

use std::path::{Path, PathBuf};

/// Where the loader's menu file lives on the boot volume (any letter case on the FAT volume).
pub const BOOT_CFG: &str = "EFI/iyakki-boot/boot.cfg";
/// The full RAM disk and its lean sibling as a menu row names them.
pub const FULL_INITRD: &str = "/boot/initrd";
pub const LEAN_INITRD: &str = "/boot/initrd-lean";
/// The lean system image: its presence marks a volume that carries its own root, so GRUB (and the loader) should use the small RAM disk.
pub const LEAN_ROOT: &str = "/boot/root.sfs";

/// One change made to the text: the entry (or `-` outside any entry), the old and the new path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub entry: String,
    pub from: String,
    pub to: String,
}

fn lean_of(p: &str) -> String {
    format!("{p}-lean")
}

fn full_of(p: &str) -> Option<String> {
    p.strip_suffix("-lean").map(str::to_string)
}

/// Menu rows are `initrd PATH`; a row a `requires PATH` line of the same entry guards (the "Full start" row) is hidden by the loader when the
/// file is missing and is left alone. `exists` answers for a volume path such as `/boot/initrd`. `prefer_lean`: the volume carries its own root
/// and the small RAM disk, so a row that names `/boot/initrd` is moved to `/boot/initrd-lean` even when the full one is there too (what GRUB does).
/// Returns the new text (unchanged text when nothing needs a change) and the list of changes.
pub fn fix_text(cfg: &str, exists: &dyn Fn(&str) -> bool, prefer_lean: bool) -> (String, Vec<Change>) {
    // first pass: the `requires` paths of every entry
    let lines: Vec<&str> = cfg.split('\n').collect();
    let mut entry_of: Vec<usize> = Vec::with_capacity(lines.len());
    let mut names: Vec<String> = vec!["-".to_string()];
    let mut guards: Vec<Vec<String>> = vec![vec![]];
    for l in &lines {
        let t = l.trim();
        if let Some(id) = t.strip_prefix("entry ") {
            names.push(id.trim().to_string());
            guards.push(vec![]);
        } else if let Some(g) = t.strip_prefix("requires ") {
            if let Some(last) = guards.last_mut() {
                last.push(g.trim().to_string());
            }
        }
        entry_of.push(names.len() - 1);
    }
    let mut changes = Vec::new();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        let Some(rest) = t.strip_prefix("initrd ") else {
            out.push((*l).to_string());
            continue;
        };
        let path = rest.trim();
        let e = entry_of[i];
        let new = if guards[e].iter().any(|g| g == path) {
            None
        } else if path == FULL_INITRD && prefer_lean && exists(LEAN_INITRD) {
            Some(LEAN_INITRD.to_string())
        } else if !exists(path) {
            // the file named is not there: the sibling (lean for full, full for lean) when that one is
            let alt = full_of(path).unwrap_or_else(|| lean_of(path));
            if exists(&alt) { Some(alt) } else { None }
        } else {
            None
        };
        match new {
            Some(n) => {
                let indent = &l[..l.len() - l.trim_start().len()];
                out.push(format!("{indent}initrd {n}"));
                changes.push(Change { entry: names[e].clone(), from: path.to_string(), to: n });
            }
            None => out.push((*l).to_string()),
        }
    }
    (out.join("\n"), changes)
}

/// The rows that name a RAM disk that is not on the volume and are not guarded by a `requires` line (what the loader would refuse to start).
pub fn missing_initrds(cfg: &str, exists: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
    let mut entry = "-".to_string();
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut guarded: Vec<(String, String)> = Vec::new();
    for l in cfg.lines() {
        let t = l.trim();
        if let Some(id) = t.strip_prefix("entry ") {
            entry = id.trim().to_string();
        } else if let Some(p) = t.strip_prefix("initrd ") {
            rows.push((entry.clone(), p.trim().to_string()));
        } else if let Some(g) = t.strip_prefix("requires ") {
            guarded.push((entry.clone(), g.trim().to_string()));
        }
    }
    rows.into_iter().filter(|(e, p)| !guarded.iter().any(|(ge, gp)| ge == e && gp == p) && !exists(p)).collect()
}

/// Case-insensitive lookup of `rel` under `root` (a FAT volume keeps the case it was written with).
pub fn find_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        let direct = cur.join(part);
        if direct.exists() {
            cur = direct;
            continue;
        }
        let hit = std::fs::read_dir(&cur).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        cur = hit.path();
    }
    Some(cur)
}

/// Does the volume carry its own root, so the small RAM disk is the one in use?
pub fn lean_volume(root: &Path) -> bool {
    find_ci(root, LEAN_INITRD).is_some() && (find_ci(root, LEAN_ROOT).is_some() || find_ci(root, FULL_INITRD).is_none())
}

/// [`fix_text`] with the volume's own answers. For a `boot.cfg` text that is about to be written to `root`.
pub fn fix_for_volume(cfg: &str, root: &Path) -> (String, Vec<Change>) {
    let exists = |p: &str| find_ci(root, p).is_some_and(|f| f.is_file());
    fix_text(cfg, &exists, lean_volume(root))
}

fn describe(c: &Change) -> String {
    format!("entry {}: initrd {} -> {}", c.entry, c.from, c.to)
}

/// Repair the loader's `boot.cfg` of a mounted boot volume: rows that name a missing RAM disk (or the full one on a lean volume) are
/// rewritten to the one that exists, through a temporary file and a rename. Returns one log line per change (empty when the file is right or
/// there is none). A file that cannot be read or written is an error and is left as it was.
pub fn fix_volume(root: &Path) -> Result<Vec<String>, String> {
    let Some(path) = find_ci(root, BOOT_CFG).filter(|p| p.is_file()) else { return Ok(vec![]) };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (fixed, changes) = fix_for_volume(&text, root);
    if changes.is_empty() {
        return Ok(vec![]);
    }
    let tmp = path.with_file_name("boot.cfg.fix");
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(fixed.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    if let Some(d) = path.parent().and_then(|d| std::fs::File::open(d).ok()) {
        let _ = d.sync_all();
    }
    Ok(changes.iter().map(describe).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(initrd: &str) -> String {
        format!("timeout 0\ntries 9\n\nentry iyakki\n  title IYAKKI\n  kind linux\n  kernel /boot/bzImage\n  initrd {initrd}\n  cmdline quiet\n  default\n\nentry install\n  title Install\n  kind linux\n  kernel /boot/bzImage\n  initrd {initrd}\n  cmdline quiet\n\nentry full\n  title Full start\n  kind linux\n  kernel /boot/bzImage\n  initrd /boot/initrd\n  cmdline quiet\n  more\n  requires /boot/initrd\n\nentry prev-plain\n  kind linux\n  kernel /EFI/iyakki-boot/prev/bzImage\n  initrd /EFI/iyakki-boot/prev/initrd\n  requires /EFI/iyakki-boot/prev/set-plain\n\nentry recovery\n  kind chain\n  kernel /EFI/BOOT/grub-recovery.efi\n")
    }

    fn vol(tag: &str, files: &[&str], cfg_text: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-bootcfg-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for f in files {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let p = d.join(BOOT_CFG);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, cfg_text).unwrap();
        d
    }

    fn read(d: &Path) -> String {
        std::fs::read_to_string(d.join(BOOT_CFG)).unwrap()
    }

    #[test]
    fn a_lean_stick_without_the_full_initrd_gets_the_lean_one_in_every_row_that_is_not_guarded() {
        let d = vol("lean", &["boot/bzImage", "boot/initrd-lean", "boot/root.sfs"], &cfg("/boot/initrd"));
        let log = fix_volume(&d).unwrap();
        assert_eq!(log.len(), 2, "{log:?}");
        let t = read(&d);
        assert_eq!(t.matches("initrd /boot/initrd-lean").count(), 2);
        // the guarded Full-start row and the previous-version rows stay as they were
        assert!(t.contains("initrd /boot/initrd\n  cmdline quiet\n  more\n  requires /boot/initrd"));
        assert!(t.contains("initrd /EFI/iyakki-boot/prev/initrd\n"));
        assert!(missing_initrds(&t, &|p| find_ci(&d, p).is_some()).iter().all(|(e, _)| e == "prev-plain"), "only the guarded-by-another-file row is open");
        assert!(fix_volume(&d).unwrap().is_empty(), "a second run changes nothing");
    }

    #[test]
    fn a_full_stick_keeps_the_full_initrd() {
        let d = vol("full", &["boot/bzImage", "boot/initrd"], &cfg("/boot/initrd"));
        let before = read(&d);
        assert!(fix_volume(&d).unwrap().is_empty());
        assert_eq!(read(&d), before);
        // a row that names the lean disk on a volume that has only the full one is moved back
        let d2 = vol("full2", &["boot/bzImage", "boot/initrd"], &cfg("/boot/initrd-lean"));
        assert_eq!(fix_volume(&d2).unwrap().len(), 2);
        assert!(read(&d2).contains("initrd /boot/initrd\n  cmdline quiet\n  default"));
    }

    #[test]
    fn a_volume_with_both_disks_and_its_own_root_uses_the_lean_one_and_without_a_root_keeps_the_full() {
        let d = vol("both-root", &["boot/initrd", "boot/initrd-lean", "boot/root.sfs"], &cfg("/boot/initrd"));
        assert_eq!(fix_volume(&d).unwrap().len(), 2);
        assert!(read(&d).contains("initrd /boot/initrd-lean"));
        let d2 = vol("both-noroot", &["boot/initrd", "boot/initrd-lean"], &cfg("/boot/initrd"));
        assert!(fix_volume(&d2).unwrap().is_empty(), "no lean root: the full disk is a working start and stays");
    }

    #[test]
    fn stripping_the_full_initrd_then_fixing_leaves_a_menu_that_starts() {
        let d = vol("strip", &["boot/bzImage", "boot/initrd", "boot/initrd-lean", "boot/root.sfs"], &cfg("/boot/initrd"));
        std::fs::remove_file(d.join("boot/initrd")).unwrap();
        let t = read(&d);
        assert!(!missing_initrds(&t, &|p| find_ci(&d, p).is_some()).is_empty(), "the strip alone breaks the menu: this is KI-200");
        fix_volume(&d).unwrap();
        let t = read(&d);
        assert!(missing_initrds(&t, &|p| find_ci(&d, p).is_some_and(|f| f.is_file())).iter().all(|(e, _)| e == "prev-plain"));
        assert!(t.contains("entry iyakki") && t.contains("default"), "the rest of the file is untouched");
    }

    #[test]
    fn an_update_text_written_to_a_lean_stick_is_fixed_before_it_is_written() {
        let d = vol("update", &["boot/bzImage", "boot/initrd-lean", "boot/root.sfs"], "old\n");
        let (t, ch) = fix_for_volume(&cfg("/boot/initrd"), &d);
        assert_eq!(ch.len(), 2);
        assert!(ch.iter().all(|c| c.from == "/boot/initrd" && c.to == "/boot/initrd-lean"));
        assert!(t.contains("initrd /boot/initrd-lean"));
    }

    #[test]
    fn a_missing_disk_with_no_sibling_is_reported_not_invented_and_upper_case_names_work() {
        let d = vol("none", &["BOOT/BZIMAGE"], &cfg("/boot/initrd"));
        assert!(fix_volume(&d).unwrap().is_empty());
        assert_eq!(missing_initrds(&read(&d), &|p| find_ci(&d, p).is_some()).len(), 3);
        let d2 = vol("caps", &["BOOT/INITRD-LEAN", "BOOT/ROOT.SFS"], &cfg("/boot/initrd"));
        assert_eq!(fix_volume(&d2).unwrap().len(), 2);
        assert!(fix_volume(Path::new("/nonexistent-iyakki-volume")).unwrap().is_empty());
    }
}

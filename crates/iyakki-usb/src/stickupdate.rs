//! `iyakki-usb --update DRIVE --from FOLDER`: replace the boot files of an IYAKKI stick or disk WITHOUT mounting it and without touching
//! anything else (F-UPG-05). FOLDER is what `iyakki dev update-pack IMAGE -o FOLDER` writes (`boot/...` and `EFI/...`).
//!
//! * The boot volume is found the way `update-pack` finds it (an MBR or GPT image, or a bare FAT32 volume) and opened with the in-house
//!   FAT32 writer (`Fat32Rw`): a file is replaced by writing the new clusters first and rewriting its one directory entry last (a power
//!   cut leaves the old file or the new one, `Fat32Rw::replace_file`).
//! * Only files that already exist on the stick are replaced, and only under `boot/` and `EFI/`. A new file is added only when its name is
//!   a name the writer can create (8.3, or a long name with a generated alias) and its folder exists; any other new file is refused with the plan (nothing is half done). Nothing is deleted.
//! * Order: everything under `boot/` first (biggest first), the `EFI/` loaders last, so a stick cut in the middle still has its old loader
//!   pointing at files that exist. After every file the new contents are read back and compared with the folder's.
//! * The plan is shown first; nothing is written without `--yes`. The install-media marker is never touched.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::bootpack::{boot_volume_offset, MAX_FILE, MAX_FILES, MAX_TOTAL};
use crate::fatclone::Fat32;
use crate::fatwrite::Fat32Rw;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Replace,
    Add,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// `boot/bzImage`, `EFI/BOOT/BOOTX64.EFI`...
    pub path: String,
    pub size: u64,
    pub kind: Kind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Files of the folder that are not part of an update (not under boot/ or EFI/, or the install marker).
    pub skipped: Vec<String>,
}

fn clean(c: &str) -> bool {
    !c.is_empty() && c.len() <= 100 && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || " ._-+~@()".contains(ch))
}

/// The files of the update folder: (relative path with `/`, size), `boot/` and `EFI/` only, within the limits an update may carry.
pub fn read_folder(from: &Path) -> Result<(Vec<(String, u64)>, Vec<String>), String> {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![(from.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let ft = e.file_type().map_err(|e| e.to_string())?;
            if ft.is_symlink() {
                return Err(format!("{path} is a link; an update folder holds plain files only"));
            }
            if !clean(&name) {
                return Err(format!("{path} has a name that is not safe to write to a stick"));
            }
            if ft.is_dir() {
                stack.push((e.path(), path));
            } else {
                let top = path.split('/').next().unwrap_or("").to_ascii_lowercase();
                if (top != "boot" && top != "efi") || !path.contains('/') || crate::fatclone::is_install_media_marker(&format!("/{path}")) {
                    skipped.push(path);
                    continue;
                }
                let size = e.metadata().map_err(|e| e.to_string())?.len();
                if size > MAX_FILE {
                    return Err(format!("{path} is {} MB: more than an update carries", size / 1_000_000));
                }
                total += size;
                if total > MAX_TOTAL || out.len() >= MAX_FILES {
                    return Err("the folder holds more than an update may carry".into());
                }
                out.push((path, size));
            }
        }
    }
    if out.is_empty() {
        return Err("the folder has no boot/ or EFI/ files: it is not an update (make one with `iyakki dev update-pack IMAGE -o FOLDER`)".into());
    }
    skipped.sort();
    Ok((out, skipped))
}

fn is_loader(path: &str) -> bool {
    path.to_ascii_lowercase().starts_with("efi/")
}

/// The public key that signs update packs (the one that signs the downloads manifest; the same file the agent embeds).
const RELEASE_PUBKEY: &str = include_str!("../../../legal/downloads-manifest.pub");

/// Whether the pack folder's `update.json` carries a good signature (F-UPG-05: the same trust rule as the Update centre).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// `update.json.sig` is a valid ed25519 signature over the exact bytes of `update.json`.
    Signed,
    /// No manifest, no signature file, or no release key in this build: nothing can be said.
    Unsigned(String),
    /// A signature that does not match: never written.
    Bad(String),
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut o = [0u8; N];
    for (i, b) in o.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(o)
}

/// The key named in the text of a key file (`None` while it says `unset`).
pub fn key_in(text: &str) -> Option<ed25519_dalek::VerifyingKey> {
    let line = text.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#'))?;
    ed25519_dalek::VerifyingKey::from_bytes(&unhex::<32>(line)?).ok()
}

/// Check `from/update.json` against `from/update.json.sig` with `key` (the built-in release key when `None`).
pub fn check_signature(from: &Path, key: Option<&ed25519_dalek::VerifyingKey>) -> Trust {
    let builtin = key_in(RELEASE_PUBKEY);
    let Some(key) = key.or(builtin.as_ref()) else { return Trust::Unsigned("this build has no release key to check a signature with".into()) };
    let Ok(manifest) = std::fs::read(from.join("update.json")) else { return Trust::Unsigned("the folder has no update.json".into()) };
    let Ok(sig) = std::fs::read_to_string(from.join("update.json.sig")) else { return Trust::Unsigned("the folder has no update.json.sig".into()) };
    let Some(sig) = unhex::<64>(&sig).map(|b| ed25519_dalek::Signature::from_bytes(&b)) else { return Trust::Bad("the signature is not 128 hexadecimal characters".into()) };
    match key.verify_strict(&manifest, &sig) {
        Ok(()) => Trust::Signed,
        Err(_) => Trust::Bad("the signature does not match update.json and the release key".into()),
    }
}

/// What an update of `target` from `from` would do, without writing anything.
pub fn plan(target: &Path, from: &Path) -> Result<Plan, String> {
    let (files, skipped) = read_folder(from)?;
    let mut f = File::open(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let base = boot_volume_offset(&mut f)?;
    let mut vol = Fat32::open(&mut f, base)?;
    let have: std::collections::BTreeMap<String, u32> = vol.walk_long()?.into_iter().filter(|i| !i.dir).map(|i| (i.path.trim_start_matches('/').to_ascii_lowercase(), i.size)).collect();
    let mut steps = Vec::new();
    let mut refused = Vec::new();
    for (path, size) in files {
        if have.contains_key(&path.to_ascii_lowercase()) {
            steps.push(Step { path, size, kind: Kind::Replace });
        } else {
            let name = path.rsplit('/').next().unwrap_or("");
            if crate::fatwrite::can_create(name) {
                steps.push(Step { path, size, kind: Kind::Add });
            } else {
                refused.push(path);
            }
        }
    }
    if !refused.is_empty() {
        return Err(format!("these files are new on the stick and their names are not names the writer can create: {}. Nothing was written.", refused.join(", ")));
    }
    // boot files first (biggest first), loaders last
    steps.sort_by(|a, b| is_loader(&a.path).cmp(&is_loader(&b.path)).then(b.size.cmp(&a.size)).then(a.path.cmp(&b.path)));
    Ok(Plan { steps, skipped })
}

fn sha(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Write the plan. `progress(step index, step count, path)` is called before each file. Every file is read back and compared.
pub fn apply(target: &Path, from: &Path, plan: &Plan, progress: &mut dyn FnMut(usize, usize, &str)) -> Result<(), String> {
    let f = OpenOptions::new().read(true).write(true).open(target).map_err(|e| format!("{}: {e}", target.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let base = {
        let mut r = &f;
        boot_volume_offset(&mut r)?
    };
    let mut rw = Fat32Rw::open(&f, base, len.saturating_sub(base))?;
    let n = plan.steps.len();
    for (i, s) in plan.steps.iter().enumerate() {
        progress(i, n, &s.path);
        let parts: Vec<&str> = s.path.split('/').collect();
        let (name, dirs) = parts.split_last().ok_or("an empty path")?;
        // walk to the folder without creating anything: a new file goes into a folder that exists
        let mut dir = rw.root_cluster();
        for d in dirs {
            match rw.find(dir, d)? {
                Some((c, true)) => dir = c,
                _ => return Err(format!("{} has no folder {d} on the stick; nothing more was written", s.path)),
            }
        }
        let src = from.join(&s.path);
        let mut file = File::open(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        match s.kind {
            Kind::Replace => rw.replace_file(dir, name, s.size, &mut file, &mut |_| {})?,
            Kind::Add => rw.create_file(dir, name, s.size, &mut file, &mut |_| {})?,
        }
        rw.flush()?;
    }
    rw.write_fsinfo()?;
    drop(rw);
    // read every written file back
    let mut h = File::open(target).map_err(|e| e.to_string())?;
    let mut vol = Fat32::open(&mut h, base)?;
    let items = vol.walk_long()?;
    for s in &plan.steps {
        let it = items.iter().find(|i| !i.dir && i.path.trim_start_matches('/').eq_ignore_ascii_case(&s.path)).ok_or_else(|| format!("{} is not on the stick after writing", s.path))?.clone();
        let data = vol.read_file(&it)?;
        let want = sha(&from.join(&s.path))?;
        let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        if got != want {
            return Err(format!("{} does not read back as written (checksum differs)", s.path));
        }
    }
    Ok(())
}

pub const HELP: &str = "iyakki-usb --update DRIVE --from FOLDER [--yes] [--allow-unsigned]\n  Replaces the boot files of an IYAKKI stick or disk with the ones in FOLDER (what `iyakki dev update-pack IMAGE -o FOLDER` writes),\n  without mounting it. Only files under boot/ and EFI/ that already exist are replaced; nothing is deleted. Without --yes it only shows the plan. The pack's update.json.sig is checked against the release key: a wrong signature is always refused, a missing one needs --allow-unsigned.\n";

/// The command line: `--update TARGET --from FOLDER [--yes]`. Returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let val = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let (Some(target), Some(from)) = (val("--update"), val("--from")) else {
        eprint!("{HELP}");
        return 2;
    };
    let (target, from): (PathBuf, PathBuf) = (target.into(), from.into());
    let yes = args.iter().any(|a| a == "--yes");
    let allow_unsigned = args.iter().any(|a| a == "--allow-unsigned");
    let trust = check_signature(&from, None);
    match &trust {
        Trust::Signed => println!("update.json is signed by the release key."),
        Trust::Unsigned(why) => println!("Not signed: {why}."),
        Trust::Bad(why) => {
            eprintln!("Refused: {why}. Nothing was written.");
            return 1;
        }
    }
    let p = match plan(&target, &from) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("{} file(s) on {}:", p.steps.len(), target.display());
    for s in &p.steps {
        println!("  {} {} ({} bytes)", if s.kind == Kind::Replace { "replace" } else { "add    " }, s.path, s.size);
    }
    for k in &p.skipped {
        println!("  skipped {k} (not part of an update)");
    }
    if !yes {
        println!("Nothing was written. Run again with --yes to write.");
        return 0;
    }
    if let Trust::Unsigned(why) = &trust {
        if !allow_unsigned {
            eprintln!("Refused: the update is not signed ({why}). Nothing was written. Add --allow-unsigned only for a pack you made yourself.");
            return 1;
        }
    }
    match apply(&target, &from, &p, &mut |i, n, path| println!("[{}/{n}] {path}", i + 1)) {
        Ok(()) => {
            println!("Done: every file was read back and matches.");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod signature_tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-usb-upsig-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn a_good_signature_passes_a_wrong_one_is_bad_and_missing_parts_are_only_unsigned() {
        let k = SigningKey::from_bytes(&[7u8; 32]);
        let d = dir("a");
        assert!(matches!(check_signature(&d, Some(&k.verifying_key())), Trust::Unsigned(w) if w.contains("no update.json")));
        std::fs::write(d.join("update.json"), b"{\"files\":[]}").unwrap();
        assert!(matches!(check_signature(&d, Some(&k.verifying_key())), Trust::Unsigned(w) if w.contains("update.json.sig")));
        std::fs::write(d.join("update.json.sig"), hex(&k.sign(b"{\"files\":[]}").to_bytes())).unwrap();
        assert_eq!(check_signature(&d, Some(&k.verifying_key())), Trust::Signed);
        // the manifest changed after it was signed
        std::fs::write(d.join("update.json"), b"{\"files\":[1]}").unwrap();
        assert!(matches!(check_signature(&d, Some(&k.verifying_key())), Trust::Bad(_)));
        // another key
        let other = SigningKey::from_bytes(&[8u8; 32]);
        std::fs::write(d.join("update.json"), b"{\"files\":[]}").unwrap();
        assert!(matches!(check_signature(&d, Some(&other.verifying_key())), Trust::Bad(_)));
        std::fs::write(d.join("update.json.sig"), "nothex").unwrap();
        assert!(matches!(check_signature(&d, Some(&k.verifying_key())), Trust::Bad(w) if w.contains("128")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_key_file_text_is_read_like_the_agents_and_unset_means_no_key() {
        assert!(key_in(RELEASE_PUBKEY).is_some(), "the built-in release key is a valid ed25519 key");
        assert!(key_in("# comment\nunset\n").is_none());
        assert!(key_in("# only comments\n").is_none());
    }
}

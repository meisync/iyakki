//! WASM migrators (F-CORE-32): an app's package may carry `migrations/NAME.wasm` (or `NAME.wat`, text) instead of the declarative
//! `migrations/NAME.json`. The module runs in the tile sandbox (`iyakki-wasm`): no network, no clock, no files but a copy of the app's own
//! data folder in memory under `/data`, fuel and memory limits, a wall-clock limit. It exports `migrate() -> i32` (0 = done) and moves the
//! data with `iyakki.fs_read` and `iyakki.fs_write` (token 0 is `FsRead("/data")`, token 1 is `FsWrite("/data")`; nothing else exists). What it changed is written back to the real folder only when it returned 0; the caller
//! (`datamigrate::run_for_app`) has already taken the snapshot and puts the data back on any failure.

use std::path::{Component, Path, PathBuf};

use iyakki_sec::{Capability, CapabilityStore};
use iyakki_vfs::{DiffKind, MemFs};
use iyakki_wasm::{Sandbox, SandboxConfig, WasmError};

/// The most files and bytes of an app's data a migrator is given (a bigger folder is refused, the data is untouched).
pub const MAX_FILES: usize = 2_000;
pub const MAX_BYTES: u64 = 16 << 20;
/// How long a migrator may run.
pub const WALL: std::time::Duration = std::time::Duration::from_secs(10);

fn load(dir: &Path, rel: &Path, fs: &mut MemFs, files: &mut usize, bytes: &mut u64) -> Result<(), String> {
    let here = dir.join(rel);
    let rd = std::fs::read_dir(&here).map_err(|e| format!("cannot read {}: {e}", here.display()))?;
    for e in rd {
        let e = e.map_err(|e| e.to_string())?;
        let md = std::fs::symlink_metadata(e.path()).map_err(|e| e.to_string())?;
        let r = rel.join(e.file_name());
        if md.file_type().is_symlink() {
            continue; // links are never followed
        }
        if md.is_dir() {
            load(dir, &r, fs, files, bytes)?;
        } else if md.is_file() {
            *files += 1;
            *bytes += md.len();
            if *files > MAX_FILES || *bytes > MAX_BYTES {
                return Err(format!("the app's data is larger than a migrator may be given ({MAX_FILES} files, {} MB)", MAX_BYTES >> 20));
            }
            let data = std::fs::read(e.path()).map_err(|e| e.to_string())?;
            let name = r.to_str().ok_or("a file name is not text")?;
            fs.write(&format!("/data/{}", name.replace('\\', "/")), &data).map_err(|e| format!("{e:?}"))?;
        }
    }
    Ok(())
}

fn safe_rel(vpath: &str) -> Option<PathBuf> {
    let rel = vpath.strip_prefix("/data/")?;
    let p = PathBuf::from(rel);
    p.components().all(|c| matches!(c, Component::Normal(_))).then_some(p)
}

/// Run the migrator `module` (wasm binary or WAT text) over `data_dir`. On `Ok` the changed files are written back (a new file, a changed file,
/// a removed file); on `Err` nothing was written.
pub fn apply(data_dir: &Path, module: &[u8]) -> Result<String, String> {
    let mut base = MemFs::with_quota((MAX_BYTES * 2) as usize);
    let (mut files, mut bytes) = (0usize, 0u64);
    load(data_dir, Path::new(""), &mut base, &mut files, &mut bytes)?;
    let vfs = std::sync::Arc::new(std::sync::Mutex::new(base.fork()));
    let caps = CapabilityStore::new();
    let tokens = vec![caps.issue(Capability::FsRead("/data".into())).map_err(|e| e.to_string())?, caps.issue(Capability::FsWrite("/data".into())).map_err(|e| e.to_string())?];
    let sb = Sandbox::new(SandboxConfig { fuel: 200_000_000, max_memory_bytes: 32 << 20 }).map_err(|e| e.to_string())?.with_wall_time(WALL);
    let out = sb.run(module, "migrate", vfs.clone(), caps, tokens).map_err(|e| match e {
        WasmError::OutOfFuel => "the migrator used up its time budget".to_string(),
        other => format!("the migrator failed: {other}"),
    })?;
    match out.result {
        Some(0) | None => {}
        Some(code) => return Err(format!("the migrator stopped with the code {code}")),
    }
    let after = vfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fork();
    let changes = after.diff(&base);
    let mut written = 0;
    // check every path first: a migrator cannot reach outside the folder
    let mut plan = Vec::new();
    for (p, kind) in &changes {
        plan.push((safe_rel(p).ok_or_else(|| format!("the migrator wrote outside the data folder ({p})"))?, *kind, p.clone()));
    }
    for (rel, kind, vpath) in plan {
        let target = data_dir.join(&rel);
        match kind {
            DiffKind::Removed => {
                std::fs::remove_file(&target).map_err(|e| format!("cannot remove {}: {e}", rel.display()))?;
            }
            DiffKind::Added | DiffKind::Modified => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let data = after.read(&vpath).map_err(|e| format!("{e:?}"))?;
                let tmp = target.with_extension("migrating");
                std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
                std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
            }
        }
        written += 1;
    }
    Ok(format!("a migrator changed {written} file{}", if written == 1 { "" } else { "s" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-wasmmig-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Reads /data/v.txt (up to 16 bytes), writes /data/v2.txt with the same bytes, removes nothing.
    const COPY: &str = r#"(module
      (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
      (import "iyakki" "fs_read" (func $fr (param i32 i32 i32 i32 i32) (result i32)))
      (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 256) "/data/v.txt")
      (data (i32.const 320) "/data/v2.txt")
      (func (export "migrate") (result i32)
        (local $n i32)
        (drop (call $tg (i32.const 0) (i32.const 0)))
        (drop (call $tg (i32.const 1) (i32.const 64)))
        (local.set $n (call $fr (i32.const 0) (i32.const 256) (i32.const 11) (i32.const 512) (i32.const 64)))
        (if (i32.lt_s (local.get $n) (i32.const 0)) (then (return (local.get $n))))
        (call $fw (i32.const 64) (i32.const 320) (i32.const 12) (i32.const 512) (local.get $n))))"#;

    #[test]
    fn a_migrator_moves_data_inside_the_sandbox_and_only_what_it_changed_is_written() {
        let d = dir("copy");
        std::fs::write(d.join("v.txt"), "version-1").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/keep.txt"), "kept").unwrap();
        let msg = apply(&d, COPY.as_bytes()).unwrap();
        assert!(msg.contains("1 file"), "{msg}");
        assert_eq!(std::fs::read_to_string(d.join("v2.txt")).unwrap(), "version-1");
        assert_eq!(std::fs::read_to_string(d.join("v.txt")).unwrap(), "version-1");
        assert_eq!(std::fs::read_to_string(d.join("sub/keep.txt")).unwrap(), "kept");
    }

    #[test]
    fn a_migrator_that_fails_loops_or_is_not_a_module_writes_nothing() {
        let d = dir("fail");
        std::fs::write(d.join("v.txt"), "x").unwrap();
        // returns a code
        let bad = r#"(module (func (export "migrate") (result i32) i32.const 7))"#;
        assert!(apply(&d, bad.as_bytes()).unwrap_err().contains("code 7"));
        // never ends: out of fuel
        let spin = r#"(module (func (export "migrate") (loop br 0)))"#;
        assert!(apply(&d, spin.as_bytes()).unwrap_err().contains("time budget"));
        // not a module
        assert!(apply(&d, b"not wasm").is_err());
        // no entry point
        assert!(apply(&d, br#"(module (func (export "other")))"#).is_err());
        assert!(!d.join("v2.txt").exists());
        assert_eq!(std::fs::read_to_string(d.join("v.txt")).unwrap(), "x");
    }

    #[test]
    fn a_migrator_without_the_grant_cannot_write_and_a_big_folder_is_refused() {
        let d = dir("big");
        // writes with token slot never filled: denied, returns the code
        let no_tokens = r#"(module
          (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (data (i32.const 256) "/etc/passwd")
          (func (export "migrate") (result i32) (call $fw (i32.const 0) (i32.const 256) (i32.const 11) (i32.const 512) (i32.const 1))))"#;
        assert!(apply(&d, no_tokens.as_bytes()).is_err());
        for i in 0..(MAX_FILES + 1) {
            std::fs::write(d.join(format!("f{i}")), "").unwrap();
        }
        assert!(apply(&d, COPY.as_bytes()).unwrap_err().contains("larger than a migrator"));
    }

    #[test]
    fn a_path_outside_the_data_folder_is_never_written() {
        assert!(safe_rel("/data/a/b.json").is_some());
        assert!(safe_rel("/data/../etc/x").is_none());
        assert!(safe_rel("/other/x").is_none());
    }
}

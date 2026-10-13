//! The per-user disk cache of compiled code: the compiled script engine, and the compiled form of big page scripts.
//!
//! Files live in a private directory (`$IYAKKI_JS_CACHE`, else `$XDG_CACHE_HOME/iyakki-js`, else
//! `~/.cache/iyakki-js`, else a per-user directory under the temp dir). A file is trusted only if we own it and
//! nobody else can write to it or to its directory, and its checksum matches what was stored. Writes go to a
//! temporary file that is renamed into place, so a reader never sees half a file. Anything doubtful is a miss.

use std::path::{Path, PathBuf};

/// A fast checksum against torn or damaged files (not against an attacker: that is what the directory's mode is for).
pub fn checksum(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut words = bytes.chunks_exact(8);
    for w in &mut words {
        h = (h ^ u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]])).wrapping_mul(0x100000001b3).rotate_left(29);
    }
    for &b in words.remainder() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h ^ bytes.len() as u64
}

#[cfg(unix)]
fn my_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::metadata("/proc/self").ok()?.uid())
}

/// The cache directory, created if need be; `None` when there is no safe place for it.
#[cfg(unix)]
pub fn dir() -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let me = my_uid()?;
        let base = match std::env::var_os("IYAKKI_JS_CACHE") {
            Some(d) => PathBuf::from(d),
            None => match (std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME")) {
                (Some(x), _) if !x.is_empty() => PathBuf::from(x).join("iyakki-js"),
                (_, Some(h)) if !h.is_empty() => PathBuf::from(h).join(".cache").join("iyakki-js"),
                _ => std::env::temp_dir().join(format!("iyakki-js-{me}")),
            },
        };
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&base);
        let m = std::fs::symlink_metadata(&base).ok()?;
        (m.is_dir() && m.uid() == me && m.mode() & 0o022 == 0).then_some(base)
    })
    .clone()
}

#[cfg(not(unix))]
pub fn dir() -> Option<PathBuf> {
    None
}

/// The contents of a cached file, if it is ours, private and intact.
#[cfg(unix)]
pub fn read(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path).ok()?;
    if !m.is_file() || m.uid() != my_uid()? || m.mode() & 0o022 != 0 {
        return None;
    }
    let mut data = std::fs::read(path).ok()?;
    if data.len() < 8 || data[..8] != checksum(&data[8..]).to_le_bytes() {
        return None;
    }
    data.drain(..8);
    Some(data)
}

#[cfg(not(unix))]
pub fn read(_: &Path) -> Option<Vec<u8>> {
    None
}

/// Store a file (best effort).
pub fn write(path: &Path, body: &[u8]) {
    let mut file = checksum(body).to_le_bytes().to_vec();
    file.extend_from_slice(body);
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&tmp, file).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Delete the oldest `*.qbc` files until the ones left take at most 3/4 of `max` bytes, if they take more than `max`.
pub fn prune(dir: &Path, ext: &str, max: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == ext))
        .filter_map(|e| e.metadata().ok().map(|m| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path())))
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= max / 4 * 3 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_damage_is_a_miss() {
        let d = std::env::temp_dir().join(format!("iyakki-js-cache-test-{}", std::process::id()));
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d).unwrap();
        }
        let f = d.join("a.qbc");
        write(&f, b"hello compiled world");
        assert_eq!(read(&f).as_deref(), Some(&b"hello compiled world"[..]));
        // a flipped byte, a short file and a world-writable file are all refused
        let mut raw = std::fs::read(&f).unwrap();
        raw[12] ^= 1;
        std::fs::write(&f, &raw).unwrap();
        assert!(read(&f).is_none());
        std::fs::write(&f, b"abc").unwrap();
        assert!(read(&f).is_none());
        write(&f, b"again");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o666)).unwrap();
        }
        assert!(read(&f).is_none(), "a file others can write is not trusted");
        // the oldest files go when the directory is over its size
        for i in 0..4 {
            write(&d.join(format!("s{i}.qbc")), &vec![i as u8; 1000]);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        prune(&d, "qbc", 3000);
        let left: Vec<String> = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!left.iter().any(|n| n == "s0.qbc") && left.iter().any(|n| n == "s3.qbc"), "{left:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

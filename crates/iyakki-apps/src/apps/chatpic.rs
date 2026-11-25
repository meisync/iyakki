//! The chat transcript's pictures live in the UI-layer crate `iyakki-chatview` (re-exported here under the old path); only the one
//! function that writes a file stays in the shell, because it asks the person's policy.

pub use iyakki_chatview::chatpic::*;

use iyakki_sec::policy::Policy;
use std::path::{Path, PathBuf};

/// Writes `bytes` as `dir/name` (a number is added when the name is taken) after checking the person may write there.
pub fn save_file(policy: &Policy, user: &str, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    crate::file_scope().check_write(policy, user, dir)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make the folder {}: {e}", dir.display()))?;
    let real = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    crate::file_scope().check_write(policy, user, &real)?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..1000 {
        let file = if n == 0 { name.to_string() } else { format!("{stem}-{}{ext}", n + 1) };
        let path = real.join(&file);
        crate::file_scope().check_write(policy, user, &path)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(bytes).map_err(|e| e.to_string())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not save {}: {e}", path.display())),
        }
    }
    Err("too many files with that name".into())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_stays_in_scope_with_unique_names() {
        let dir = std::env::temp_dir().join(format!("iyakki-chatpic-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let mut pol = iyakki_sec::policy::Role::User.template();
        pol.fs_write = vec![dir.join("home").display().to_string()];
        let a = save_file(&pol, "u", &dir.join("home/Pictures"), "x.svg", b"one").unwrap();
        let b = save_file(&pol, "u", &dir.join("home/Pictures"), "x.svg", b"two").unwrap();
        let c = save_file(&pol, "u", &dir.join("home/Pictures"), "x.svg", b"three").unwrap();
        assert_eq!([a.file_name().unwrap().to_str().unwrap(), b.file_name().unwrap().to_str().unwrap(), c.file_name().unwrap().to_str().unwrap()], ["x.svg", "x-2.svg", "x-3.svg"]);
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        // outside the write scope: refused, nothing created
        let e = save_file(&pol, "u", &dir.join("elsewhere"), "x.svg", b"no").unwrap_err();
        assert!(e.contains("permission denied"), "{e}");
        assert!(!dir.join("elsewhere").exists());
        // a link inside the folder that leads out is caught
        std::os::unix::fs::symlink(dir.join("elsewhere2"), dir.join("home/out")).ok();
        std::fs::create_dir_all(dir.join("elsewhere2")).unwrap();
        assert!(save_file(&pol, "u", &dir.join("home/out"), "y.svg", b"no").is_err());
        assert!(!dir.join("elsewhere2/y.svg").exists());
    }
}

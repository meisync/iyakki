//! Making the optional browser package (F-BROW-40): the engine's helper program plus the signed description the Store card and the install
//! read (`<store>/browser/<arch>/webview-helper` and `manifest.json`, see [`crate::browserpkg`]). This is the publisher's side: it runs on the
//! build host with the store's signing key, which is read from a file whose path is given at run time and never printed or kept. Nothing here
//! uploads anything: the two files land in a folder the publisher puts on the store.
//!
//! `iyakki browser-package build --helper FILE --version V --key-file KEY --out DIR [--arch A]` writes `DIR/browser/<arch>/`.
//! `iyakki browser-package pubkey --key-file KEY` prints the public key (the value a person pins in Settings, Store).

use std::path::{Path, PathBuf};

use crate::browserpkg::{self, Manifest};

const ELF: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// Make the package files in `out`. Returns the folder written (`out/browser/<arch>`). A helper that is not an ELF program, is empty or is
/// above [`browserpkg::MAX_SIZE`], and a version or architecture with odd characters, are refused; the files are written whole through
/// part files so a cut leaves no half package.
pub fn build(helper: &Path, version: &str, arch: &str, key: &ed25519_dalek::SigningKey, out: &Path) -> Result<PathBuf, String> {
    let plain = |s: &str, max: usize| !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b));
    if !plain(version, 40) {
        return Err("the version is letters, digits and . _ - + only (at most 40)".into());
    }
    if !plain(arch, 16) {
        return Err("the architecture is letters and digits only (for example x86_64)".into());
    }
    let meta = std::fs::metadata(helper).map_err(|e| format!("{}: {e}", helper.display()))?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > browserpkg::MAX_SIZE {
        return Err(format!("the helper must be a file of 1 byte to {} MB", browserpkg::MAX_SIZE >> 20));
    }
    let mut head = [0u8; 4];
    {
        use std::io::Read;
        std::fs::File::open(helper).and_then(|mut f| f.read_exact(&mut head)).map_err(|e| e.to_string())?;
    }
    if head != ELF {
        return Err("the helper is not a program (no ELF header): refusing to describe it".into());
    }
    let sha256 = crate::devmode::files::sha256_file(helper)?;
    let dir = out.join("browser").join(arch);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join("webview-helper.part");
    let result = (|| -> Result<(), String> {
        std::fs::copy(helper, &part).map_err(|e| e.to_string())?;
        std::fs::rename(&part, dir.join("webview-helper")).map_err(|e| e.to_string())?;
        let doc = browserpkg::manifest_doc(&Manifest { version: version.into(), size: meta.len(), sha256, arch: arch.into() }, key);
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        let mpart = dir.join("manifest.json.part");
        std::fs::write(&mpart, format!("{text}\n")).map_err(|e| e.to_string())?;
        std::fs::rename(&mpart, dir.join("manifest.json")).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(dir.join("manifest.json.part"));
    }
    result.map(|_| dir)
}

fn read_key(path: &str) -> Result<ed25519_dalek::SigningKey, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read the key file: {e}"))?;
    crate::pkg::signing_key_from_hex(&text).map_err(|e| e.to_string())
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// `iyakki browser-package ...`; the exit status. The key's bytes are never printed.
pub fn cli_main(args: &[String]) -> i32 {
    let usage = "usage: iyakki browser-package build --helper FILE --version V --key-file KEY --out DIR [--arch A] | pubkey --key-file KEY";
    match args.first().map(String::as_str) {
        Some("build") => {
            let (Some(helper), Some(version), Some(keyf), Some(out)) = (flag(args, "--helper"), flag(args, "--version"), flag(args, "--key-file"), flag(args, "--out")) else {
                crate::qeprintln!("{usage}");
                return 2;
            };
            let arch = flag(args, "--arch").unwrap_or(std::env::consts::ARCH);
            let key = match read_key(keyf) {
                Ok(k) => k,
                Err(e) => {
                    crate::qeprintln!("{e}");
                    return 1;
                }
            };
            match build(Path::new(helper), version, arch, &key, Path::new(out)) {
                Ok(dir) => {
                    crate::qprintln!("wrote {} (webview-helper and manifest.json); the public key to pin: {}", dir.display(), crate::pkg::public_key_hex(&key));
                    0
                }
                Err(e) => {
                    crate::qeprintln!("{e}");
                    1
                }
            }
        }
        Some("pubkey") => match flag(args, "--key-file").map(read_key) {
            Some(Ok(k)) => {
                crate::qprintln!("{}", crate::pkg::public_key_hex(&k));
                0
            }
            Some(Err(e)) => {
                crate::qeprintln!("{e}");
                1
            }
            None => {
                crate::qeprintln!("{usage}");
                2
            }
        },
        _ => {
            crate::qeprintln!("{usage}");
            2
        }
    }
}

#[cfg(test)]
#[path = "browserpkg_build_tests.rs"]
mod tests;

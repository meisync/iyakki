//! Releases: the images (ISO, disk images) together with a list of SHA-256 sums that is signed with an ed25519 key, and a
//! plain download page that says how to check them. Anyone can verify a download with `sha256sum -c SHA256SUMS` (for the
//! hashes) and `iyakki release verify` (for the signature), and only the holder of the secret key can make a list that
//! verifies.

use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const SUMS: &str = "SHA256SUMS";
pub const SIG: &str = "SHA256SUMS.sig";
pub const PAGE: &str = "index.html";
/// The signed manifest: version, date, files with sizes and sums, commit, kernel, variants, minimum requirements.
pub const MANIFEST: &str = "release.json";
pub const MANIFEST_SIG: &str = "release.json.sig";

/// What the manifest says besides the files. Every field may be empty (it is then written as an empty string).
#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// `YYYY-MM-DD`; empty means today (UTC).
    pub date: String,
    pub git_commit: String,
    pub git_describe: String,
    pub kernel: String,
}

/// Civil date (UTC) of a Unix time in seconds, as `YYYY-MM-DD` (Howard Hinnant's days-to-civil).
pub fn civil_date(secs: u64) -> String {
    let z = (secs / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Which variant a release file belongs to: `web` (the big image with the full web engine), `mac`, `macweb` (both, for Intel Macs), `window` (desktop tarball) or
/// `standard`.
pub fn variant_of(name: &str) -> &'static str {
    if name.contains("-macweb-") {
        "macweb"
    } else if name.contains("-web-") {
        "web"
    } else if name.contains("-mac-") {
        "mac"
    } else if name.contains("window") || name.contains("desktop") || name.ends_with(".tar.gz") {
        "window"
    } else {
        "standard"
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Make a signing key and keep its secret half in `path` (readable by the owner only). Returns the public half in hex.
pub fn keygen(path: &Path) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut seed = [0u8; 32];
    iyakki_sec::random_fill(&mut seed).map_err(|e| format!("no randomness: {e:?}"))?;
    let key = SigningKey::from_bytes(&seed);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(hex(&seed).as_bytes()).map_err(|e| e.to_string())?;
    Ok(hex(key.verifying_key().as_bytes()))
}

fn load_key(path: &Path) -> Result<SigningKey, String> {
    let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    unhex::<32>(&t).map(|s| SigningKey::from_bytes(&s)).ok_or_else(|| format!("{} is not a signing key made by `iyakki release keygen`", path.display()))
}

pub fn public_key_of(path: &Path) -> Result<String, String> {
    Ok(hex(load_key(path)?.verifying_key().as_bytes()))
}

/// SHA-256 of a file, read in pieces.
pub fn hash_file(p: &Path) -> Result<(u64, String), String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let (mut h, mut n, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 20]);
    loop {
        let k = f.read(&mut buf).map_err(|e| format!("{}: {e}", p.display()))?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, hex(&h.finalize())))
}

/// The files that belong to a release: plain files in the folder, except the list, its signature and the page.
fn release_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| { let n = n.to_string_lossy(); !n.starts_with('.') && n != SUMS && n != SIG && n != PAGE && n != MANIFEST && n != MANIFEST_SIG && !n.ends_with(".tmp") }))
        .collect();
    v.sort();
    Ok(v)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn human(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.2} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{:.1} MB", b as f64 / (1u64 << 20) as f64) }
}

fn page(title: &str, version: &str, public_key: &str, files: &[Entry]) -> String {
    let rows: String = files
        .iter()
        .map(|f| format!("<tr><td><a href=\"{n}\" download>{n}</a></td><td>{}</td><td><code>{}</code></td></tr>\n", human(f.bytes), f.sha256, n = esc(&f.name)))
        .collect();
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{t} {v}</title>\n<style>body{{font:16px/1.5 system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem;color:#1d2433}}table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid #ccd;padding:.4rem .6rem;text-align:left;vertical-align:top}}code{{word-break:break-all;font-size:.8rem}}pre{{background:#f2f4fa;padding:.8rem;overflow:auto}}</style></head><body>\n<h1>{t} {v}</h1>\n<p>Downloads. Check them before you use them: the list of sums is signed, so a changed file or a changed list is noticed.</p>\n<table><tr><th>File</th><th>Size</th><th>SHA-256</th></tr>\n{rows}</table>\n<h2>Check a download</h2>\n<pre>sha256sum -c {SUMS} --ignore-missing\niyakki release verify . --pubkey {k}</pre>\n<p>The first line checks that the files match the list; the second checks that the list (and {MANIFEST}) was signed by the key below. Compare the key with the one published in the project README before you trust it.</p>\n<p>Signing key (ed25519, public half): <code>{k}</code><br>Files: <a href=\"{SUMS}\">{SUMS}</a> · <a href=\"{SIG}\">{SIG}</a> · <a href=\"{MANIFEST}\">{MANIFEST}</a> · <a href=\"{MANIFEST_SIG}\">{MANIFEST_SIG}</a></p>\n<p>This is an alpha: read the release notes and the known issues first, and never write an image to a drive that holds anything you need.</p>\n<h2>Write it to a USB stick</h2>\n<p>Use <code>iyakki-usb</code> (it asks which drive, shows its name and size, and erases only what you confirm), or write the image yourself with a tool you trust. Everything on the stick is erased.</p>\n</body></html>\n",
        t = esc(title),
        v = esc(version),
        k = public_key
    )
}

/// Sum every file in `dir`, sign the list with the key in `key_file`, and write `SHA256SUMS`, `SHA256SUMS.sig` and `index.html`
/// there. Returns the files that were listed.
pub fn sign_dir(dir: &Path, key_file: &Path, title: &str, version: &str) -> Result<Vec<Entry>, String> {
    sign_dir_with(dir, key_file, title, version, &Meta::default())
}

/// Like [`sign_dir`], and also writes the signed manifest `release.json` (+ `release.json.sig`).
pub fn sign_dir_with(dir: &Path, key_file: &Path, title: &str, version: &str, meta: &Meta) -> Result<Vec<Entry>, String> {
    let key = load_key(key_file)?;
    let files = release_files(dir)?;
    if files.is_empty() {
        return Err(format!("{} has no files to release", dir.display()));
    }
    let mut entries = Vec::new();
    for p in &files {
        let (bytes, sha256) = hash_file(p)?;
        entries.push(Entry { name: p.file_name().unwrap_or_default().to_string_lossy().into_owned(), bytes, sha256 });
    }
    let sums: String = entries.iter().map(|e| format!("{}  {}\n", e.sha256, e.name)).collect();
    let sig = key.sign(sums.as_bytes());
    let pubhex = hex(key.verifying_key().as_bytes());
    std::fs::write(dir.join(SUMS), &sums).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(SIG), format!("{}\n", hex(&sig.to_bytes()))).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(PAGE), page(title, version, &pubhex, &entries)).map_err(|e| e.to_string())?;
    let manifest = manifest_json(title, version, &pubhex, &entries, meta);
    std::fs::write(dir.join(MANIFEST), &manifest).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(MANIFEST_SIG), format!("{}\n", hex(&key.sign(manifest.as_bytes()).to_bytes()))).map_err(|e| e.to_string())?;
    Ok(entries)
}

/// The text of `release.json` (pretty, with a trailing newline; the signature covers exactly these bytes).
pub fn manifest_json(title: &str, version: &str, public_key: &str, files: &[Entry], meta: &Meta) -> String {
    let date = if meta.date.is_empty() { civil_date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())) } else { meta.date.clone() };
    let mut variants: Vec<&str> = files.iter().map(|f| variant_of(&f.name)).collect();
    variants.sort();
    variants.dedup();
    let v = serde_json::json!({
        "schema": 1,
        "title": title,
        "version": version,
        "date": date,
        "git_commit": meta.git_commit,
        "git_describe": meta.git_describe,
        "kernel": meta.kernel,
        "signing_key_ed25519": public_key,
        "variants": variants,
        "requirements": {
            "x86_64": "64-bit x86 PC (BIOS or UEFI), 2 GB RAM, 4 GB USB stick or disk for the install image",
            "aarch64": "64-bit ARM machine or VM with UEFI, 3 GB RAM",
            "web_variant": "16 GB RAM recommended; the image is large (see the download page)",
            "window": "Linux x86_64 desktop with OpenGL (the window mode tarball)"
        },
        "files": files.iter().map(|f| serde_json::json!({"name": f.name, "bytes": f.bytes, "sha256": f.sha256, "variant": variant_of(&f.name)})).collect::<Vec<_>>(),
    });
    format!("{}\n", serde_json::to_string_pretty(&v).unwrap_or_default())
}

/// Check `release.json` in `dir`: signature first, then size and sum of every file it lists. Returns the version it names.
pub fn verify_manifest(dir: &Path, public_key_hex: &str) -> Result<(String, Vec<Verified>), String> {
    let key = unhex::<32>(public_key_hex).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or("that is not an ed25519 public key (64 hexadecimal characters)")?;
    let text = std::fs::read(dir.join(MANIFEST)).map_err(|e| format!("{}: {e}", dir.join(MANIFEST).display()))?;
    let sig = std::fs::read_to_string(dir.join(MANIFEST_SIG)).ok().and_then(|t| unhex::<64>(&t)).map(|b| Signature::from_bytes(&b)).ok_or("the manifest signature is missing or damaged")?;
    key.verify(&text, &sig).map_err(|_| "the manifest signature does not match: it was not signed with that key, or it was changed".to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&text).map_err(|e| format!("release.json: {e}"))?;
    let mut out = Vec::new();
    for f in v["files"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let (name, want, bytes) = (f["name"].as_str().unwrap_or(""), f["sha256"].as_str().unwrap_or(""), f["bytes"].as_u64().unwrap_or(0));
        if name.is_empty() || name.contains('/') || name.contains("..") {
            out.push(Verified { name: name.into(), ok: false, why: "the name is not a plain file name".into() });
            continue;
        }
        out.push(match hash_file(&dir.join(name)) {
            Ok((n, got)) if got == want && n == bytes => Verified { name: name.into(), ok: true, why: String::new() },
            Ok(_) => Verified { name: name.into(), ok: false, why: "the file does not match the manifest".into() },
            Err(_) => Verified { name: name.into(), ok: false, why: "the file is missing".into() },
        });
    }
    Ok((v["version"].as_str().unwrap_or("").into(), out))
}

/// What a manifest's text says about the file called `name`: `(bytes, sha256)`. No signature check here; callers that have the
/// public key use [`verify_manifest`].
pub fn manifest_entry(manifest: &str, name: &str) -> Option<(u64, String)> {
    let v: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let f = v["files"].as_array()?.iter().find(|f| f["name"].as_str() == Some(name))?;
    Some((f["bytes"].as_u64()?, f["sha256"].as_str()?.to_string()))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Verified {
    pub name: String,
    pub ok: bool,
    pub why: String,
}

/// Check a release folder against a public key: the signature on `SHA256SUMS` first (nothing else is trusted until it holds),
/// then every file in the list. Errors only when the signature does not verify.
pub fn verify_dir(dir: &Path, public_key_hex: &str) -> Result<Vec<Verified>, String> {
    let key = unhex::<32>(public_key_hex).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or("that is not an ed25519 public key (64 hexadecimal characters)")?;
    let sums = std::fs::read(dir.join(SUMS)).map_err(|e| format!("{}: {e}", dir.join(SUMS).display()))?;
    let sig = std::fs::read_to_string(dir.join(SIG)).ok().and_then(|t| unhex::<64>(&t)).map(|b| Signature::from_bytes(&b)).ok_or("the signature file is missing or damaged")?;
    key.verify(&sums, &sig).map_err(|_| "the signature does not match: this list was not signed with that key, or it was changed".to_string())?;
    let text = String::from_utf8(sums).map_err(|_| "the list of sums is not text".to_string())?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Some((want, name)) = line.split_once("  ") else {
            out.push(Verified { name: line.into(), ok: false, why: "a line of the list is not understood".into() });
            continue;
        };
        // a name from the list may never leave the folder
        if name.contains('/') || name.contains("..") || name.is_empty() {
            out.push(Verified { name: name.into(), ok: false, why: "the name is not a plain file name".into() });
            continue;
        }
        let r = match hash_file(&dir.join(name)) {
            Ok((_, got)) if got == want => Verified { name: name.into(), ok: true, why: String::new() },
            Ok((_, got)) => Verified { name: name.into(), ok: false, why: format!("the file does not match its sum (it hashes to {}…)", &got[..16]) },
            Err(_) => Verified { name: name.into(), ok: false, why: "the file is missing".into() },
        };
        out.push(r);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-release-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("iyakki-x86_64.iso"), vec![7u8; 3_000_000]).unwrap();
        std::fs::write(d.join("iyakki-aarch64.img"), b"disk image bytes").unwrap();
        std::fs::write(d.join(".hidden"), "x").unwrap();
        d
    }

    #[test]
    fn a_release_is_summed_signed_and_described_on_a_page_and_verifies_with_the_public_key() {
        let d = folder("ok");
        let key = d.join(".release.key");
        let public = keygen(&key).unwrap();
        assert_eq!(public.len(), 64);
        assert_eq!(public_key_of(&key).unwrap(), public);
        assert!(keygen(&key).is_err(), "an existing key is never overwritten");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        let entries = sign_dir(&d, &key, "IYAKKI <test>", "0.1.0").unwrap();
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["iyakki-aarch64.img", "iyakki-x86_64.iso"], "the key and hidden files are not part of it");
        let sums = std::fs::read_to_string(d.join(SUMS)).unwrap();
        assert!(sums.contains("  iyakki-x86_64.iso\n") && sums.lines().count() == 2);
        let page = std::fs::read_to_string(d.join(PAGE)).unwrap();
        assert!(page.contains("IYAKKI &lt;test&gt; 0.1.0") && page.contains("iyakki-x86_64.iso") && page.contains(&public) && page.contains("sha256sum -c") && page.contains("2.9 MB"), "{page}");
        let v = verify_dir(&d, &public).unwrap();
        assert!(v.iter().all(|x| x.ok) && v.len() == 2, "{v:?}");
        // the manifest round trip
        let (ver, mv) = verify_manifest(&d, &public).unwrap();
        assert_eq!(ver, "0.1.0");
        assert!(mv.len() == 2 && mv.iter().all(|x| x.ok), "{mv:?}");
        let m = std::fs::read_to_string(d.join(MANIFEST)).unwrap();
        let j: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(j["schema"], 1);
        assert_eq!(j["signing_key_ed25519"], public.as_str());
        assert!(j["date"].as_str().unwrap().len() == 10 && j["variants"][0] == "standard", "{m}");
        assert_eq!(manifest_entry(&m, "iyakki-aarch64.img").unwrap().0, 16);
        assert!(manifest_entry(&m, "nothing").is_none());
        assert!(!sums.contains(MANIFEST), "the manifest is not part of its own list");
        // a changed manifest is refused, and so is another key
        std::fs::write(d.join(MANIFEST), m.replace("0.1.0", "9.9.9")).unwrap();
        assert!(verify_manifest(&d, &public).unwrap_err().contains("does not match"));
        std::fs::write(d.join(MANIFEST), &m).unwrap();
        assert!(verify_manifest(&d, &keygen(&d.join(".other2")).unwrap()).is_err());
        // the same list checks out with the standard tool
        if let Ok(o) = std::process::Command::new("sha256sum").args(["-c", SUMS]).current_dir(&d).output() {
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn civil_dates_and_variants() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(951_782_400), "2000-02-29");
        assert_eq!(variant_of("iyakki-web-x86_64.iso"), "web");
        assert_eq!(variant_of("iyakki-macweb-x86_64.iso"), "macweb");
        assert_eq!(variant_of("iyakki-mac-install-x86_64.img"), "mac");
        assert_eq!(variant_of("iyakki-0.1.0-alpha.1-linux-x86_64-window.tar.gz"), "window");
        assert_eq!(variant_of("iyakki-x86_64.iso"), "standard");
    }

    #[test]
    fn a_changed_file_a_changed_list_a_wrong_key_and_a_missing_file_are_all_caught() {
        let d = folder("bad");
        let key = d.join(".k");
        let public = keygen(&key).unwrap();
        sign_dir(&d, &key, "IYAKKI", "1").unwrap();
        // a file changed after signing
        std::fs::write(d.join("iyakki-aarch64.img"), b"disk image bytes, tampered").unwrap();
        let v = verify_dir(&d, &public).unwrap();
        assert!(v.iter().any(|x| x.name == "iyakki-aarch64.img" && !x.ok && x.why.contains("does not match")));
        assert!(v.iter().any(|x| x.name == "iyakki-x86_64.iso" && x.ok));
        // a file gone
        std::fs::remove_file(d.join("iyakki-x86_64.iso")).unwrap();
        assert!(verify_dir(&d, &public).unwrap().iter().any(|x| x.why == "the file is missing"));
        // someone else's key
        let other = keygen(&d.join(".other")).unwrap();
        assert!(verify_dir(&d, &other).unwrap_err().contains("does not match"));
        assert!(verify_dir(&d, "zz").unwrap_err().contains("not an ed25519"));
        // the list changed to match the tampered file: the signature no longer holds
        let (_, h) = hash_file(&d.join("iyakki-aarch64.img")).unwrap();
        std::fs::write(d.join(SUMS), format!("{h}  iyakki-aarch64.img\n")).unwrap();
        assert!(verify_dir(&d, &public).unwrap_err().contains("does not match"));
        // names that climb out of the folder are refused even when signed
        let evil_key = SigningKey::from_bytes(&[9u8; 32]);
        let evil = "0000000000000000000000000000000000000000000000000000000000000000  ../etc/passwd\n";
        std::fs::write(d.join(SUMS), evil).unwrap();
        std::fs::write(d.join(SIG), format!("{}\n", hex(&evil_key.sign(evil.as_bytes()).to_bytes()))).unwrap();
        let v = verify_dir(&d, &hex(evil_key.verifying_key().as_bytes())).unwrap();
        assert!(!v[0].ok && v[0].why.contains("plain file name"));
        assert!(sign_dir(&d.join("nonexistent"), &key, "x", "1").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

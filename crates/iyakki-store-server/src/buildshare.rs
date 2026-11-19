//! `iyakki-buildshare`: the small program behind `dist/release/publish-build.sh` and `fetch-build.sh`. It does the parts that need
//! the update key or a verdict (make and sign the build list, verify a list, select a build, check a downloaded folder); the scripts
//! do the HTTP. The signing key is read from a file here and never printed or sent. Docs: docs/development/BUILD-SHARING.md.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use iyakki_agent::pkg::{new_signing_key, public_key_hex, signing_key_from_hex, signing_key_hex};
use iyakki_agent::store::buildshare as bs;

const USAGE: &str = "\
iyakki-buildshare: sign, verify and select shared builds (used by publish-build.sh and fetch-build.sh)

  keygen --out FILE                         make an update key (64 hex characters, mode 0600); prints the public key
  pubkey --key FILE                         print the public key of a key file
  verify-pack --pack DIR                    check update.json and every file of an update pack
  sign-add --key FILE --list OLD|- --pack DIR --variant V --channel C --build-id ID [--commit C] [--keep N] [--store-id S] --out NEW
                                            add a build to the list (keep N newest per machine type and channel, default 1) and sign it;
                                            prints `FILE <sha256> <size> <path>` per file and `DROPPED <id>` per build that fell out
  promote --key FILE --list OLD --variant V --from CHANNEL --to CHANNEL [--build ID|latest] [--keep N] [--store-id S] --out NEW
                                            make the build of one channel the newest of another (same files, nothing to upload) and sign the
                                            list; prints `PROMOTED <id> <from> <to>` and `DROPPED` lines
  verify --list FILE --pin KEY              check a list's signature against the pinned public key (hex or a file); prints a summary
  select --list FILE --pin KEY --variant V --channel C [--build ID|latest] [--held FILE]
                                            --held: the layer hashes the device holds, one per line; a layered build whose base the device
                                            lacks is skipped (the newest full build is chosen), and the error says so when none fits
                                            prints `BUILD <id> <version> <commit> <published> <total>` and `FILE <sha256> <size> <path>`
  check-dir --list FILE --pin KEY --variant V --channel C --build ID --dir D
                                            every file of the build in D: size, SHA-256, then the pack's own check
";

fn out(s: &str) {
    let _ = std::io::stdout().write_all(s.as_bytes());
}

fn flags(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut m = HashMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let f = a.strip_prefix("--").ok_or_else(|| format!("unexpected {a}"))?;
        m.insert(f.to_string(), it.next().ok_or_else(|| format!("--{f} needs a value"))?.clone());
    }
    Ok(m)
}

fn need<'a>(m: &'a HashMap<String, String>, k: &str) -> Result<&'a str, String> {
    m.get(k).map(String::as_str).ok_or_else(|| format!("--{k} is required"))
}

/// A public key given as hex or as a file holding it.
fn pin_of(v: &str) -> Result<String, String> {
    let t = if Path::new(v).is_file() { std::fs::read_to_string(v).map_err(|e| format!("{v}: {e}"))? } else { v.to_string() };
    let t = t.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if t.len() != 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the pinned key is 64 hex characters (or a file holding them)".into());
    }
    Ok(t)
}

fn load_key(path: &str) -> Result<ed25519_dalek::SigningKey, String> {
    use std::os::unix::fs::PermissionsExt;
    let md = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if md.permissions().mode() & 0o077 != 0 {
        return Err(format!("{path} must not be readable by others (chmod 600)"));
    }
    signing_key_from_hex(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?).map_err(|e| e.to_string())
}

fn read_list(path: &str, pin: &str) -> Result<bs::List, String> {
    bs::verify(&std::fs::read(path).map_err(|e| format!("{path}: {e}"))?, pin)
}

fn print_build(b: &bs::Build) {
    out(&format!("BUILD {} {} {} {} {}\n", b.build_id, b.version, if b.commit.is_empty() { "-" } else { &b.commit }, b.published, b.total));
    for f in &b.files {
        out(&format!("FILE {} {} {}\n", f.sha256, f.size, f.path));
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or("a command is needed (--help)")?;
    let m = flags(rest)?;
    match cmd.as_str() {
        "keygen" => {
            use std::os::unix::fs::OpenOptionsExt;
            let p = need(&m, "out")?;
            let k = new_signing_key().map_err(|e| e.to_string())?;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).map_err(|e| format!("{p}: {e} (an existing key is never overwritten)"))?;
            f.write_all(format!("{}\n", signing_key_hex(&k)).as_bytes()).map_err(|e| e.to_string())?;
            out(&format!("{}\n", public_key_hex(&k)));
        }
        "pubkey" => out(&format!("{}\n", public_key_hex(&load_key(need(&m, "key")?)?))),
        "verify-pack" => {
            let (version, variant, files) = bs::read_pack(Path::new(need(&m, "pack")?))?;
            out(&format!("PACK {version} {variant} {} files\n", files.len()));
        }
        "sign-add" => {
            let key = load_key(need(&m, "key")?)?;
            let pin = public_key_hex(&key);
            let old = match need(&m, "list")? {
                "-" => None,
                p => Some(read_list(p, &pin).map_err(|e| format!("the stored list: {e}"))?),
            };
            let (version, pack_variant, files) = bs::read_pack(Path::new(need(&m, "pack")?))?;
            let keep: usize = m.get("keep").map_or(Ok(1), |k| k.parse().map_err(|_| "--keep is a number".to_string()))?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            let b = bs::Build {
                variant: need(&m, "variant")?.into(),
                channel: need(&m, "channel")?.into(),
                build_id: need(&m, "build-id")?.into(),
                version,
                pack_variant,
                commit: m.get("commit").cloned().unwrap_or_default(),
                published: now,
                seq: 0,
                total: 0,
                files,
                needs: bs::pack_needs(Path::new(need(&m, "pack")?))?,
            };
            let (list, dropped) = bs::add_build(old.as_ref(), m.get("store-id").map_or("iyakki-builds", String::as_str), b, keep, now)?;
            let doc = bs::build_doc(&list, &key);
            bs::verify(doc.as_bytes(), &pin)?;
            let o = need(&m, "out")?;
            std::fs::write(o, doc).map_err(|e| format!("{o}: {e}"))?;
            let new = list.builds.iter().max_by_key(|b| b.seq).ok_or("no build")?;
            for f in &new.files {
                out(&format!("FILE {} {} {}\n", f.sha256, f.size, f.path));
            }
            for d in dropped {
                out(&format!("DROPPED {} {} {}\n", d.variant, d.channel, d.build_id));
            }
        }
        "promote" => {
            let key = load_key(need(&m, "key")?)?;
            let pin = public_key_hex(&key);
            let old = read_list(need(&m, "list")?, &pin).map_err(|e| format!("the stored list: {e}"))?;
            let keep: usize = m.get("keep").map_or(Ok(1), |k| k.parse().map_err(|_| "--keep is a number".to_string()))?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            let (list, dropped, b) = bs::promote(&old, m.get("store-id").map_or("iyakki-builds", String::as_str), need(&m, "variant")?, need(&m, "from")?, need(&m, "to")?, m.get("build").map_or("latest", String::as_str), keep, now)?;
            let doc = bs::build_doc(&list, &key);
            bs::verify(doc.as_bytes(), &pin)?;
            let o = need(&m, "out")?;
            std::fs::write(o, doc).map_err(|e| format!("{o}: {e}"))?;
            out(&format!("PROMOTED {} {} {}\n", b.build_id, need(&m, "from")?, b.channel));
            for d in dropped {
                out(&format!("DROPPED {} {} {}\n", d.variant, d.channel, d.build_id));
            }
        }
        "verify" => {
            let l = read_list(need(&m, "list")?, &pin_of(need(&m, "pin")?)?)?;
            out(&format!("LIST serial {} builds {} key {}\n", l.serial, l.builds.len(), l.key));
            for b in &l.builds {
                out(&format!("{}/{} {} {} {} {} {}\n", b.variant, b.channel, b.build_id, b.version, if b.commit.is_empty() { "-" } else { &b.commit }, b.published, b.total));
            }
        }
        "select" => {
            let l = read_list(need(&m, "list")?, &pin_of(need(&m, "pin")?)?)?;
            let which = m.get("build").map_or("latest", String::as_str);
            // `--held FILE`: the SHA-256 of every layer the target device holds, one per line (empty file: a single-root device)
            let held: Option<std::collections::BTreeSet<String>> = match m.get("held") {
                Some(f) => Some(std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect()),
                None => None,
            };
            let b = match &held {
                Some(h) => bs::choose_for_device(&l, need(&m, "variant")?, need(&m, "channel")?, which, Some(h))?,
                None => bs::find(&l, need(&m, "variant")?, need(&m, "channel")?, which)?,
            };
            print_build(b);
        }
        "check-dir" => {
            let l = read_list(need(&m, "list")?, &pin_of(need(&m, "pin")?)?)?;
            let b = bs::find(&l, need(&m, "variant")?, need(&m, "channel")?, need(&m, "build")?)?;
            bs::verify_dir(&PathBuf::from(need(&m, "dir")?), b)?;
            out(&format!("OK {} files\n", b.files.len()));
        }
        _ => return Err(format!("unknown command {cmd}")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), None | Some("--help" | "-h" | "help")) {
        out(USAGE);
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "iyakki-buildshare: {e}");
            ExitCode::FAILURE
        }
    }
}

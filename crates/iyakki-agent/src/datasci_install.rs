//! Installing the data science toolkit (F-FLOW-31, docs/development/DATA-SCIENCE-ADDON-PLAN.md section 3.4), the part that runs on the
//! machine: check the pin table, download exactly the pinned files from the allowed hosts, check every SHA-256 before a file is opened,
//! unpack with the archive crate's strict rules, take away everything that could run at import or start up (`*.pth`, `sitecustomize`,
//! `usercustomize`, `*.data/scripts`), run the self-test, and only then put the folder in place. No pip, no setup.py, no hook runs.
//!
//! FAILS CLOSED: with the shipped table (addresses and hashes not filled in, see `iyakki_shared::datasci`) [`install`] returns the plain
//! sentence before the first byte is fetched. The self-test needs the Linux sandbox's runner; the real one is not wired yet (`no_runner`),
//! so even a filled table cannot mark the item installed by accident.

use iyakki_shared::datasci::{self, Kind, Pin};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The self-test of the real machine: not wired to the sandbox runner yet, so it says so and the install stops before it is marked.
pub fn no_runner(_data: &Path) -> Result<String, String> {
    Err("the toolkit self-test needs the Linux sandbox runner with no network, which is not wired to this install yet; the folder was not put in place".into())
}

fn file_name_of(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string()
}

fn follow(st: &dyn iyakki_proxy::Streamer, url: &str) -> Result<iyakki_proxy::StreamHead, String> {
    let mut url = url.to_string();
    for _ in 0..6 {
        if !datasci::host_allowed(&url) {
            return Err(format!("{url} is not on the toolkit's download hosts; refused"));
        }
        let head = st.open(&url, &[("User-Agent".into(), "IYAKKI".into()), ("Accept-Encoding".into(), "identity".into())]).map_err(|e| format!("could not reach {url}: {e}"))?;
        if (300..400).contains(&head.status) {
            let loc = head.header("location").ok_or("a redirect without a place to go")?.to_string();
            url = if loc.starts_with("https://") { loc } else if loc.starts_with('/') { format!("https://{}{loc}", url.trim_start_matches("https://").split('/').next().unwrap_or("")) } else { return Err("an unusable redirect".into()) };
            continue;
        }
        if head.status != 200 {
            return Err(format!("{url} answered HTTP {}", head.status));
        }
        return Ok(head);
    }
    Err("too many redirects".into())
}

/// Download one pinned file into `dest`, hashing as it arrives; a wrong or oversized file is deleted and nothing is kept.
pub fn download(st: &dyn iyakki_proxy::Streamer, pin: &Pin, dest: &Path) -> Result<(), String> {
    let head = follow(st, pin.url)?;
    let cap = (pin.mb as u64 * 5 / 4 + 1) << 20;
    if head.header("content-length").and_then(|v| v.parse::<u64>().ok()).is_some_and(|n| n > cap) {
        return Err(format!("{} is far bigger than the {} MB expected; refused", pin.name, pin.mb));
    }
    let part = dest.with_extension("part");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?);
    let (mut h, mut body, mut got) = (Sha256::new(), head.body, 0u64);
    let mut buf = vec![0u8; 256 * 1024];
    let r = (|| -> Result<(), String> {
        loop {
            let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
            if n == 0 {
                return out.flush().map_err(|e| e.to_string());
            }
            got += n as u64;
            if got > cap {
                return Err("the download is larger than expected; stopped".into());
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
    })();
    drop(out);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let have: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if have != pin.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the checksum of {} ({have}) is not the one pinned in IYAKKI ({}); the file was deleted", pin.name, pin.sha256));
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

fn hash_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 64 * 1024]);
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect());
        }
        h.update(&buf[..n]);
    }
}

/// Take out of an unpacked wheel tree whatever could run code at import or start-up, and the console scripts.
pub fn prune_site(site: &Path) -> usize {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(site) else { return 0 };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir && (name.ends_with(".pth") || name == "sitecustomize.py" || name == "usercustomize.py") || is_dir && name == "bin" {
            n += (if is_dir { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }).is_ok() as usize;
        } else if is_dir && name.ends_with(".data") {
            for sub in ["scripts", "headers"] {
                n += std::fs::remove_dir_all(p.join(sub)).is_ok() as usize;
            }
        }
    }
    n
}

/// Take away from the unpacked Python what a no-network runner never needs (tests, the IDE, Tk, pip's bootstrap).
pub fn prune_python(py: &Path) -> usize {
    let mut n = 0;
    let lib = py.join("lib");
    if let Ok(rd) = std::fs::read_dir(&lib) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("python3.") {
                for sub in ["test", "idlelib", "tkinter", "ensurepip", "turtledemo"] {
                    n += std::fs::remove_dir_all(e.path().join(sub)).is_ok() as usize;
                }
            }
        }
    }
    n
}

/// The whole install into `<data>/datasci`. `selftest` runs the fixed script inside the sandbox with no network and says what it saw.
pub fn install(st: &dyn iyakki_proxy::Streamer, data: &Path, pins: &[Pin], selftest: &mut dyn FnMut(&Path) -> Result<String, String>, say: &mut dyn FnMut(&str)) -> Result<String, String> {
    let n = pins.len();
    let total = 3 + n * 2;
    say(&format!("[1/{total}] checking the download table"));
    datasci::check(pins)?;
    let root = datasci::dir(data);
    let stage = data.join("datasci.new");
    let dl = data.join("datasci.download");
    let _ = std::fs::remove_dir_all(&stage);
    for d in [&dl, &stage.join("site"), &stage.join("scratch")] {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut run = || -> Result<Vec<String>, String> {
        let mut files: Vec<(PathBuf, &Pin)> = Vec::new();
        for (i, p) in pins.iter().enumerate() {
            let dest = dl.join(file_name_of(p.url));
            if dest.exists() && hash_file(&dest).is_ok_and(|h| h == p.sha256) {
                say(&format!("[{}/{total}] {} {} is already downloaded and checked", 2 + i, p.name, p.version));
            } else {
                say(&format!("[{}/{total}] downloading {} {} (about {} MB)", 2 + i, p.name, p.version, p.mb));
                download(st, p, &dest)?;
            }
            files.push((dest, p));
        }
        let mut kept = Vec::new();
        for (i, (f, p)) in files.iter().enumerate() {
            say(&format!("[{}/{total}] checking and unpacking {} {}", 2 + n + i, p.name, p.version));
            // checked again right before it is opened: nothing reads an unchecked file
            let have = hash_file(f)?;
            if have != p.sha256 {
                return Err(format!("{} changed after it was downloaded ({have}); stopped", p.name));
            }
            match p.kind {
                Kind::Python => {
                    iyakki_archive::archive::untar_gz(f, &stage, &stage.join("scratch"))?;
                    prune_python(&stage.join("python"));
                }
                Kind::Wheel => {
                    iyakki_archive::archive::unzip(f, &stage.join("site"))?;
                }
            }
            kept.push(format!("{} {} {}", p.name, p.version, p.sha256));
        }
        prune_site(&stage.join("site"));
        Ok(kept)
    };
    let kept = match run() {
        Ok(k) => k,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
    };
    let _ = std::fs::remove_dir_all(stage.join("scratch"));
    say(&format!("[{}/{total}] testing the toolkit in the sandbox with no network", total - 1));
    // the self-test sees the finished tree where it will live: it is moved first and moved back when the test fails
    let _ = std::fs::remove_dir_all(&root);
    std::fs::rename(&stage, &root).map_err(|e| format!("{}: {e}", root.display()))?;
    let seen = match selftest(data) {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&root);
            return Err(e);
        }
    };
    let st_json = serde_json::to_string(&datasci::State { files: kept, selftest: seen.clone() }).map_err(|e| e.to_string())?;
    std::fs::write(root.join("state.json"), st_json).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dl);
    say(&format!("[{total}/{total}] done: {seen}"));
    Ok(seen)
}

#[cfg(test)]
#[path = "datasci_install_tests.rs"]
mod tests;

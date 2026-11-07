//! The config drive of a cloud node (docs/CLOUD-IMAGES.md, "The config drive"), like cloud-init's NoCloud: a small volume labelled
//! `IYAKKI-CONFIG` (ISO 9660, ext4) or `IYAKKI-CFG` (FAT, whose labels hold 11 characters), or a folder `iyakki-config/` on the data
//! partition, holds the first-boot settings:
//!
//! | file | what |
//! |---|---|
//! | `iyakki-cloud.cfg` | `key = value` lines: `hostname`, `admin.name`, `admin.hash` (an Argon2id PHC string), `net.dns`, `net.static.IFACE = ADDR/LEN gw GATEWAY`, `net.wifi.ssid`, `net.wifi.psk` |
//! | `cert.pem`, `key.pem` (or `tls/`) | the web front end's certificate |
//! | `tenants.json` | the tenant directory (`cloud/tenants.json`), written only when missing |
//! | `tenant-config.json` | the operator's tenant configuration (`cloud/config/tenant-config.json`), refreshed at every boot |
//!
//! Nothing here runs unless the node is a cloud node (`iyakki.cloud=1` or `iyakki.mode=cloud` on the kernel line, or a cloud variant).
//! Parsing and planning are pure; [`apply`] writes under a root; [`boot`] finds the drive. Also here: the guarded self-install
//! (`iyakki.cloud.install=DISK` with `iyakki.cloud.confirm=ERASE-DISK`), which only builds the arguments of the existing installer.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

pub const LABELS: [&str; 2] = ["IYAKKI-CONFIG", "IYAKKI-CFG"];
pub const FOLDER: &str = "iyakki-config";
pub const CFG_FILE: &str = "iyakki-cloud.cfg";
const MAX_FILE: u64 = 1 << 20;

pub fn is_config_label(l: &str) -> bool {
    LABELS.iter().any(|x| x.eq_ignore_ascii_case(l.trim()))
}

fn trim_label(b: &[u8]) -> Option<String> {
    let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The volume label from the first 64 KiB of a device: ISO 9660, FAT32, FAT12/16 or ext2/3/4.
pub fn label_of(head: &[u8]) -> Option<String> {
    if head.len() >= 0x8000 + 72 && &head[0x8001..0x8006] == b"CD001" {
        return trim_label(&head[0x8028..0x8048]);
    }
    if head.len() >= 90 && &head[82..87] == b"FAT32" {
        return trim_label(&head[71..82]).filter(|l| l != "NO NAME");
    }
    if head.len() >= 62 && (&head[54..57] == b"FAT") {
        return trim_label(&head[43..54]).filter(|l| l != "NO NAME");
    }
    if head.len() >= 1160 && head[1080] == 0x53 && head[1081] == 0xEF {
        return trim_label(&head[1144..1160]);
    }
    None
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Seed {
    pub hostname: Option<String>,
    pub admin: Option<(String, String)>,
    /// lines for `/etc/iyakki/net.conf`
    pub net: Vec<String>,
    pub cert: Option<Vec<u8>>,
    pub key: Option<Vec<u8>>,
    pub tenants: Option<Vec<u8>>,
    pub tenant_config: Option<Vec<u8>>,
}

fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 63 && !h.starts_with('-') && !h.ends_with('-') && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_static(v: &str) -> bool {
    let mut it = v.split_whitespace();
    let Some(a) = it.next() else { return false };
    let Some((ip, len)) = a.split_once('/') else { return false };
    let Ok(n) = len.parse::<u8>() else { return false };
    let ok_ip = if ip.parse::<Ipv4Addr>().is_ok() { n <= 32 } else { ip.parse::<Ipv6Addr>().is_ok() && n <= 128 };
    if !ok_ip {
        return false;
    }
    match (it.next(), it.next(), it.next()) {
        (None, _, _) => true,
        (Some("gw"), Some(g), None) => g.parse::<Ipv4Addr>().is_ok() || g.parse::<Ipv6Addr>().is_ok(),
        _ => false,
    }
}

fn argon_hash(h: &str) -> bool {
    h.starts_with("$argon2id$") && h.len() < 400 && h.chars().all(|c| c.is_ascii_graphic())
}

/// Read `files` (name to content; `tls/cert.pem` is the same as `cert.pem`). Every problem is reported, none is skipped silently.
pub fn parse(files: &BTreeMap<String, Vec<u8>>) -> Result<Seed, Vec<String>> {
    let mut s = Seed::default();
    let mut errs = Vec::new();
    if let Some(cfg) = files.get(CFG_FILE) {
        let text = String::from_utf8_lossy(cfg);
        let (mut aname, mut ahash) = (None, None);
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                errs.push(format!("{CFG_FILE} line {}: not key = value", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "hostname" if valid_host(v) => s.hostname = Some(v.into()),
                "hostname" => errs.push(format!("hostname \"{v}\": lower-case letters, digits and - only, at most 63")),
                "admin.name" if !v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => aname = Some(v.to_string()),
                "admin.name" => errs.push("admin.name: letters, digits, - and _ (at most 32)".into()),
                "admin.hash" if argon_hash(v) => ahash = Some(v.to_string()),
                "admin.hash" => errs.push("admin.hash must be an Argon2id string ($argon2id$...); a plain password is never accepted here".into()),
                "net.dns" if v.split(',').all(|d| d.trim().parse::<std::net::IpAddr>().is_ok()) => s.net.push(format!("dns = {v}")),
                "net.dns" => errs.push(format!("net.dns \"{v}\": addresses separated by commas")),
                "net.wifi.ssid" if !v.is_empty() && v.len() <= 32 => s.net.push(format!("wifi.ssid = {v}")),
                "net.wifi.psk" if (8..=63).contains(&v.len()) => s.net.push(format!("wifi.psk = {v}")),
                "net.wifi.ssid" | "net.wifi.psk" => errs.push(format!("{k}: wrong length")),
                _ if k.starts_with("net.static.") => {
                    let iface = &k["net.static.".len()..];
                    if !iface.is_empty() && iface.len() <= 15 && iface.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') && valid_static(v) {
                        s.net.push(format!("static.{iface} = {v}"));
                    } else {
                        errs.push(format!("{k} = {v}: expected ADDRESS/LENGTH [gw GATEWAY]"));
                    }
                }
                _ => errs.push(format!("{CFG_FILE}: unknown key \"{k}\"")),
            }
        }
        match (aname, ahash) {
            (Some(n), Some(h)) => s.admin = Some((n, h)),
            (None, None) => {}
            (Some(_), None) => errs.push("admin.name needs admin.hash".into()),
            (None, Some(h)) => s.admin = Some(("admin".into(), h)),
        }
    }
    let get = |a: &str, b: &str| files.get(a).or_else(|| files.get(b)).cloned();
    s.cert = get("cert.pem", "tls/cert.pem");
    s.key = get("key.pem", "tls/key.pem");
    match (&s.cert, &s.key) {
        (Some(c), Some(k)) => {
            if !String::from_utf8_lossy(c).contains("-----BEGIN CERTIFICATE-----") {
                errs.push("cert.pem has no certificate".into());
            }
            if !String::from_utf8_lossy(k).contains("PRIVATE KEY-----") {
                errs.push("key.pem has no private key".into());
            }
        }
        (None, None) => {}
        _ => errs.push("cert.pem and key.pem go together".into()),
    }
    for (name, slot) in [("tenants.json", &mut s.tenants), ("tenant-config.json", &mut s.tenant_config)] {
        if let Some(b) = files.get(name) {
            match serde_json::from_slice::<serde_json::Value>(b) {
                Ok(v) if v.is_object() => *slot = Some(b.clone()),
                Ok(_) => errs.push(format!("{name}: must be a JSON object")),
                Err(e) => errs.push(format!("{name}: {e}")),
            }
        }
    }
    if errs.is_empty() {
        Ok(s)
    } else {
        Err(errs)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Write {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
    /// leave an existing file alone
    pub keep_existing: bool,
}

pub fn plan(s: &Seed, data_dir: &str) -> Vec<Write> {
    let mut v = Vec::new();
    let w = |path: String, bytes: Vec<u8>, mode: u32, keep_existing: bool| Write { path, bytes, mode, keep_existing };
    if let Some(h) = &s.hostname {
        v.push(w("/etc/hostname".into(), format!("{h}\n").into_bytes(), 0o644, false));
    }
    if !s.net.is_empty() {
        v.push(w("/etc/iyakki/net.conf".into(), format!("{}\n", s.net.join("\n")).into_bytes(), 0o600, false));
    }
    if let Some((n, h)) = &s.admin {
        let j = serde_json::json!({"users": [{"name": n, "hash": h, "role": "admin"}]});
        v.push(w(format!("{data_dir}/users.json"), j.to_string().into_bytes(), 0o600, true));
    }
    if let (Some(c), Some(k)) = (&s.cert, &s.key) {
        v.push(w(format!("{data_dir}/tls/cert.pem"), c.clone(), 0o644, true));
        v.push(w(format!("{data_dir}/tls/key.pem"), k.clone(), 0o600, true));
    }
    if let Some(t) = &s.tenants {
        v.push(w(format!("{data_dir}/cloud/tenants.json"), t.clone(), 0o600, true));
    }
    if let Some(t) = &s.tenant_config {
        v.push(w(format!("{data_dir}/cloud/config/tenant-config.json"), t.clone(), 0o600, false));
    }
    v
}

/// Write the plan under `root` (`/` on a node). Returns one line per file.
pub fn apply(p: &[Write], root: &Path) -> Vec<String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut log = Vec::new();
    for w in p {
        let dest = root.join(w.path.trim_start_matches('/'));
        if w.keep_existing && dest.exists() {
            log.push(format!("kept {} (it exists)", w.path));
            continue;
        }
        let r = (|| -> std::io::Result<()> {
            if let Some(d) = dest.parent() {
                std::fs::create_dir_all(d)?;
            }
            let tmp = dest.with_extension("seedtmp");
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(w.mode).open(&tmp)?;
            f.write_all(&w.bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &dest)
        })();
        log.push(match r {
            Ok(()) => format!("wrote {}", w.path),
            Err(e) => format!("FAILED {}: {e}", w.path),
        });
    }
    log
}

/// The files of a mounted drive or folder (known names only, at most 1 MiB each).
pub fn read_folder(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    for name in [CFG_FILE, "cert.pem", "key.pem", "tls/cert.pem", "tls/key.pem", "tenants.json", "tenant-config.json"] {
        let p = dir.join(name);
        if std::fs::metadata(&p).is_ok_and(|md| md.is_file() && md.len() <= MAX_FILE) {
            if let Ok(b) = std::fs::read(&p) {
                m.insert(name.to_string(), b);
            }
        }
    }
    m
}

/// Is the seed read at this boot? Cloud nodes only.
pub fn wanted(cmdline: &str, variant: &str) -> bool {
    cmdline.split_whitespace().any(|t| t == "iyakki.cloud=1" || t == "iyakki.mode=cloud") || variant.contains("cloud")
}

/// The arguments for `iyakki install` when the kernel line asks for a self-install, `Err` when it asks wrongly, `None` when it does not ask.
/// Both parameters are needed and must agree: `iyakki.cloud.install=sda iyakki.cloud.confirm=ERASE-sda`. Nothing is ever installed otherwise.
pub fn install_request(cmdline: &str) -> Option<Result<Vec<String>, String>> {
    let get = |k: &str| cmdline.split_whitespace().find_map(|t| t.strip_prefix(k));
    let (disk, confirm) = (get("iyakki.cloud.install="), get("iyakki.cloud.confirm="));
    let disk = disk?;
    let good = !disk.is_empty() && disk.len() <= 20 && disk.chars().all(|c| c.is_ascii_alphanumeric());
    if !good {
        return Some(Err(format!("iyakki.cloud.install=\"{disk}\" is not a disk name like sda or nvme0n1")));
    }
    match confirm {
        Some(c) if c == format!("ERASE-{disk}") => Some(Ok(vec!["--disk".into(), disk.into(), "--confirm".into(), disk.into(), "--yes".into()])),
        _ => Some(Err(format!("iyakki.cloud.install={disk} is ignored: add iyakki.cloud.confirm=ERASE-{disk} to erase it"))),
    }
}

/// At boot, on a cloud node: read the first drive labelled for us (and the `iyakki-config/` folder of the data partition) and apply it.
/// Returns log lines. Needs `mount`; not covered by unit tests (the QEMU test in dist/cloud/build-cloud.sh covers it).
pub fn boot(cmdline: &str, variant: &str, data_dir: &str) -> Vec<String> {
    if !wanted(cmdline, variant) {
        return Vec::new();
    }
    let mut log = Vec::new();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // the folder on the data partition first; a labelled drive overrides it
    for (k, v) in read_folder(&Path::new(data_dir).join(FOLDER)) {
        files.insert(k, v);
    }
    if let Ok(rd) = std::fs::read_dir("/sys/class/block") {
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with("loop") && !n.starts_with("ram") && !n.starts_with("zram")).collect();
        names.sort();
        for n in names {
            let dev = format!("/dev/{n}");
            let mut head = vec![0u8; 0x8000 + 0x100];
            let ok = std::fs::File::open(&dev).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok();
            if !ok || !label_of(&head).is_some_and(|l| is_config_label(&l)) {
                continue;
            }
            let mnt = "/run/iyakki-config";
            let _ = std::fs::create_dir_all(mnt);
            let m = std::process::Command::new("mount").args(["-o", "ro", &dev, mnt]).status();
            if m.is_ok_and(|s| s.success()) {
                for (k, v) in read_folder(Path::new(mnt)) {
                    files.insert(k, v);
                }
                let _ = std::process::Command::new("umount").arg(mnt).status();
                log.push(format!("config drive {dev} read"));
            } else {
                log.push(format!("config drive {dev}: could not be mounted"));
            }
            break;
        }
    }
    if files.is_empty() {
        log.push("no config drive (DHCP and the defaults)".into());
        return log;
    }
    match parse(&files) {
        Ok(seed) => log.extend(apply(&plan(&seed, data_dir), Path::new("/"))),
        Err(e) => log.extend(e.into_iter().map(|e| format!("config drive refused: {e}"))),
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.as_bytes().to_vec())).collect()
    }

    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA";

    #[test]
    fn labels_are_read_from_iso_fat_and_ext() {
        let mut iso = vec![0u8; 0x8100];
        iso[0x8001..0x8006].copy_from_slice(b"CD001");
        iso[0x8028..0x8028 + 13].copy_from_slice(b"IYAKKI-CONFIG");
        assert_eq!(label_of(&iso).as_deref(), Some("IYAKKI-CONFIG"));
        let mut fat = vec![0u8; 0x8100];
        fat[82..90].copy_from_slice(b"FAT32   ");
        fat[71..82].copy_from_slice(b"IYAKKI-CFG ");
        assert_eq!(label_of(&fat).as_deref(), Some("IYAKKI-CFG"));
        let mut f16 = vec![0u8; 0x8100];
        f16[54..62].copy_from_slice(b"FAT16   ");
        f16[43..54].copy_from_slice(b"IYAKKI-CFG ");
        assert_eq!(label_of(&f16).as_deref(), Some("IYAKKI-CFG"));
        let mut ext = vec![0u8; 0x8100];
        ext[1080] = 0x53;
        ext[1081] = 0xEF;
        ext[1144..1157].copy_from_slice(b"IYAKKI-CONFIG");
        assert_eq!(label_of(&ext).as_deref(), Some("IYAKKI-CONFIG"));
        assert_eq!(label_of(&vec![0u8; 0x8100]), None);
        assert!(is_config_label("iyakki-config") && is_config_label("IYAKKI-CFG") && !is_config_label("cidata"));
    }

    #[test]
    fn a_good_drive_is_parsed_and_planned() {
        let cfg = format!("# node\nhostname = node1\nadmin.name = ops\nadmin.hash = {HASH}\nnet.dns = 1.1.1.1, 9.9.9.9\nnet.static.eth0 = 192.168.1.50/24 gw 192.168.1.1\n");
        let f = files(&[(CFG_FILE, &cfg), ("tls/cert.pem", "-----BEGIN CERTIFICATE-----\nx"), ("tls/key.pem", "-----BEGIN PRIVATE KEY-----\nx"), ("tenants.json", "{\"tenants\":[]}"), ("tenant-config.json", "{\"schema\":1,\"items\":[]}")]);
        let s = parse(&f).unwrap();
        assert_eq!(s.hostname.as_deref(), Some("node1"));
        assert_eq!(s.net, vec!["dns = 1.1.1.1, 9.9.9.9", "static.eth0 = 192.168.1.50/24 gw 192.168.1.1"]);
        let p = plan(&s, "/var/lib/iyakki");
        let paths: Vec<&str> = p.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(paths, ["/etc/hostname", "/etc/iyakki/net.conf", "/var/lib/iyakki/users.json", "/var/lib/iyakki/tls/cert.pem", "/var/lib/iyakki/tls/key.pem", "/var/lib/iyakki/cloud/tenants.json", "/var/lib/iyakki/cloud/config/tenant-config.json"]);
        assert!(p.iter().find(|w| w.path.ends_with("key.pem")).unwrap().mode == 0o600);
        assert!(!p.iter().find(|w| w.path.ends_with("tenant-config.json")).unwrap().keep_existing, "the operator's config is refreshed");
        let u: serde_json::Value = serde_json::from_slice(&p[2].bytes).unwrap();
        assert_eq!(u["users"][0]["role"], "admin");
    }

    #[test]
    fn bad_input_is_refused_with_reasons() {
        let cfg = "hostname = Bad_Name\nadmin.hash = hunter2\nnet.static.eth0 = 300.1.1.1/24\nnet.dns = nope\nfoo = 1\nnonsense\n";
        let e = parse(&files(&[(CFG_FILE, cfg), ("cert.pem", "-----BEGIN CERTIFICATE-----"), ("tenants.json", "[1]")])).unwrap_err();
        let all = e.join("\n");
        for want in ["hostname", "Argon2id", "net.static.eth0", "net.dns", "unknown key", "not key = value", "go together", "JSON object"] {
            assert!(all.contains(want), "{want} in {all}");
        }
        assert!(parse(&BTreeMap::new()).is_ok(), "no files, no problem");
    }

    #[test]
    fn apply_writes_modes_and_keeps_existing_accounts() {
        let root = std::env::temp_dir().join(format!("iyakki-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let f = files(&[(CFG_FILE, &format!("hostname = n1\nadmin.hash = {HASH}\n")), ("tenant-config.json", "{\"schema\":1}")]);
        let p = plan(&parse(&f).unwrap(), "/var/lib/iyakki");
        let log = apply(&p, &root);
        assert!(log.iter().all(|l| l.starts_with("wrote")), "{log:?}");
        assert_eq!(std::fs::read_to_string(root.join("etc/hostname")).unwrap(), "n1\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(root.join("var/lib/iyakki/users.json")).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(root.join("var/lib/iyakki/users.json"), "{\"users\":[]}").unwrap();
        let log = apply(&p, &root);
        assert!(log.iter().any(|l| l.starts_with("kept /var/lib/iyakki/users.json")), "{log:?}");
        assert_eq!(std::fs::read_to_string(root.join("var/lib/iyakki/users.json")).unwrap(), "{\"users\":[]}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_folder_takes_only_known_names() {
        let d = std::env::temp_dir().join(format!("iyakki-seedf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("tls")).unwrap();
        std::fs::write(d.join(CFG_FILE), "hostname = a").unwrap();
        std::fs::write(d.join("tls/cert.pem"), "c").unwrap();
        std::fs::write(d.join("other.txt"), "x").unwrap();
        let m = read_folder(&d);
        assert_eq!(m.keys().map(String::as_str).collect::<Vec<_>>(), [CFG_FILE, "tls/cert.pem"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_seed_is_read_only_on_a_cloud_node() {
        assert!(wanted("console=ttyS0 iyakki.cloud=1", "") && wanted("iyakki.mode=cloud", "") && wanted("", "cloud-x86"));
        assert!(!wanted("quiet", "web") && !wanted("iyakki.cloud=0", ""));
    }

    #[test]
    fn self_install_needs_both_parameters_and_they_must_agree() {
        assert_eq!(install_request("quiet"), None);
        assert!(install_request("iyakki.cloud.install=sda").unwrap().unwrap_err().contains("ERASE-sda"));
        assert!(install_request("iyakki.cloud.install=sda iyakki.cloud.confirm=ERASE-sdb").unwrap().is_err());
        assert!(install_request("iyakki.cloud.confirm=ERASE-sda").is_none(), "a confirmation alone does nothing");
        assert!(install_request("iyakki.cloud.install=../x iyakki.cloud.confirm=ERASE-../x").unwrap().is_err());
        assert_eq!(install_request("iyakki.cloud.install=sda iyakki.cloud.confirm=ERASE-sda").unwrap().unwrap(), ["--disk", "sda", "--confirm", "sda", "--yes"]);
    }
}

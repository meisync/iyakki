//! Snapshots and backups of a tenant's volume (docs/development/CLOUD-MULTITENANT-PLAN.md, 3b): `iyakki cloud snapshot create | list |
//! verify | restore | prune | cronjob`.
//!
//! * A snapshot is a folder `<snapshots>/<id>/` holding `data.tar.zst` (a ustar archive, zstd compressed with `ruzstd`), `manifest.json`
//!   (every file and folder with size, mode and SHA-256; the archive's SHA-256; links that were skipped) and `manifest.sha256`. It is built
//!   under a temporary name and renamed, so a half-made snapshot never shows up in the list.
//! * The default snapshot folder is `<data>/cloud/snapshots`, a sibling of the live `<data>/cloud/files`, never inside it (refused if the
//!   two are nested). A second claim mounted elsewhere can be used with `--dir`.
//! * Links are never followed and never archived (they are listed as skipped), so a snapshot cannot carry another person's data out of a
//!   folder, and a restore cannot plant a link.
//! * Restore checks the manifest and the archive first, unpacks into a staging folder inside the snapshot, checks every file against the
//!   manifest and only then copies. It writes into a NEW folder by default (`<data>/cloud/restored/<id>`, refused if it exists); into any
//!   other folder a file that is already there stops the restore unless `--overwrite` is given. Every destination path goes through the
//!   path jail (no `..`, no link on the way).
//! * Not done: roles lists and audit are not part of a snapshot (restoring them over live ones would be a privilege decision); no
//!   incremental snapshots; a file that changes while it is read stops the snapshot (run again) rather than being stored half-written.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloudstorage;

pub const FORMAT: u32 = 1;
const ARCHIVE: &str = "data.tar.zst";
const MANIFEST: &str = "manifest.json";
const MANIFEST_SUM: &str = "manifest.sha256";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub path: String,
    /// `file` or `dir`.
    pub kind: String,
    pub size: u64,
    pub mode: u32,
    /// Hex SHA-256 of a file's contents (empty for a folder).
    #[serde(default)]
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub id: String,
    /// Seconds since the epoch.
    pub created: u64,
    pub archive: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub items: Vec<Item>,
    #[serde(default)]
    pub skipped: Vec<String>,
}

impl Manifest {
    pub fn files(&self) -> usize {
        self.items.iter().filter(|i| i.kind == "file").count()
    }
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha_file(p: &Path) -> Result<String, String> {
    let mut f = BufReader::new(File::open(p).map_err(|e| format!("{}: {e}", p.display()))?);
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 64 * 1024]);
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

/// `20261103T041500Z` from seconds since the epoch (UTC), so ids sort by time.
pub fn id_for(now: u64) -> String {
    let (days, secs) = ((now / 86_400) as i64, now % 86_400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// An id this module made: `YYYYMMDDTHHMMSSZ`, optionally `-N`. Anything else is never a snapshot folder.
pub fn id_ok(s: &str) -> bool {
    let (main, n) = s.split_once('-').map_or((s, None), |(a, b)| (a, Some(b)));
    let b = main.as_bytes();
    b.len() == 16 && b[8] == b'T' && b[15] == b'Z' && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit) && n.is_none_or(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------------------------------------------------------- tar

fn put(field: &mut [u8], v: u64) {
    let s = format!("{:0width$o}\0", v, width = field.len() - 1);
    field.copy_from_slice(s.as_bytes());
}

/// One ustar header; names up to 255 bytes are split into prefix and name at a `/`.
fn header(name: &str, kind: u8, mode: u32, size: u64) -> Result<[u8; 512], String> {
    let (prefix, base) = if name.len() <= 100 {
        ("", name)
    } else {
        let i = name.char_indices().filter(|(i, c)| *c == '/' && *i <= 155 && name.len() - i - 1 <= 100).map(|(i, _)| i).last().ok_or_else(|| format!("{name}: the path is too long for the archive"))?;
        (&name[..i], &name[i + 1..])
    };
    let mut h = [0u8; 512];
    h[..base.len()].copy_from_slice(base.as_bytes());
    put(&mut h[100..108], u64::from(mode & 0o7777));
    put(&mut h[108..116], 0);
    put(&mut h[116..124], 0);
    put(&mut h[124..136], size);
    put(&mut h[136..148], 0);
    h[148..156].copy_from_slice(b"        ");
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

struct Walk {
    tar: BufWriter<File>,
    items: Vec<Item>,
    skipped: Vec<String>,
}

impl Walk {
    fn dir(&mut self, at: &Path, rel: &str) -> Result<(), String> {
        let mut names: Vec<_> = std::fs::read_dir(at).map_err(|e| format!("{}: {e}", at.display()))?.flatten().map(|e| e.file_name()).collect();
        names.sort();
        for n in names {
            let Some(name) = n.to_str() else {
                self.skipped.push(format!("{rel}/{} (name is not UTF-8)", n.to_string_lossy()));
                continue;
            };
            let (p, r) = (at.join(&n), if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") });
            let m = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let mode = mode_of(&m);
            if m.is_dir() {
                self.tar.write_all(&header(&format!("{r}/"), b'5', mode, 0)?).map_err(|e| e.to_string())?;
                self.items.push(Item { path: r.clone(), kind: "dir".into(), size: 0, mode, sha256: String::new() });
                self.dir(&p, &r)?;
            } else if m.is_file() {
                self.file(&p, &r, mode)?;
            } else {
                self.skipped.push(format!("{r} ({})", if m.file_type().is_symlink() { "link" } else { "not a regular file" }));
            }
        }
        Ok(())
    }

    fn file(&mut self, p: &Path, rel: &str, mode: u32) -> Result<(), String> {
        let mut f = File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        self.tar.write_all(&header(rel, b'0', mode, size)?).map_err(|e| e.to_string())?;
        let (mut h, mut left, mut buf) = (Sha256::new(), size, vec![0u8; 64 * 1024]);
        while left > 0 {
            let want = buf.len().min(left as usize);
            let n = f.read(&mut buf[..want]).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err(format!("{rel} became shorter while it was read; run the snapshot again"));
            }
            h.update(&buf[..n]);
            self.tar.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            left -= n as u64;
        }
        self.tar.write_all(&vec![0u8; ((512 - size % 512) % 512) as usize]).map_err(|e| e.to_string())?;
        self.items.push(Item { path: rel.to_string(), kind: "file".into(), size, mode, sha256: hex(&h.finalize()) });
        Ok(())
    }
}

#[cfg(unix)]
fn mode_of(m: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o777
}
#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> u32 {
    0o600
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode & 0o777));
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

// ---------------------------------------------------------------------------------------------------------------- create

fn nested(a: &Path, b: &Path) -> bool {
    let (a, b) = (std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf()), std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf()));
    a.starts_with(&b) || b.starts_with(&a)
}

/// Snapshot `source` (the tenant's files folder; a missing one is an empty tenant) into a new folder under `snaps`.
pub fn create(source: &Path, snaps: &Path, now: u64) -> Result<Manifest, String> {
    if snaps.exists() && source.exists() && nested(source, snaps) {
        return Err("the snapshot folder and the files folder must not be inside each other".into());
    }
    std::fs::create_dir_all(snaps).map_err(|e| format!("{}: {e}", snaps.display()))?;
    set_mode(snaps, 0o700);
    if source.exists() && nested(source, snaps) {
        return Err("the snapshot folder and the files folder must not be inside each other".into());
    }
    let mut id = id_for(now);
    let mut n = 1;
    while snaps.join(&id).exists() {
        id = format!("{}-{n}", id_for(now));
        n += 1;
    }
    let tmp = snaps.join(format!(".making-{id}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let r = build(source, &tmp, &id, now);
    match r {
        Ok(m) => {
            std::fs::rename(&tmp, snaps.join(&id)).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp);
                e.to_string()
            })?;
            Ok(m)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

fn build(source: &Path, tmp: &Path, id: &str, now: u64) -> Result<Manifest, String> {
    let raw = tmp.join("data.tar");
    let mut w = Walk { tar: BufWriter::new(File::create(&raw).map_err(|e| e.to_string())?), items: vec![], skipped: vec![] };
    if source.is_dir() {
        w.dir(source, "")?;
    }
    w.tar.write_all(&[0u8; 1024]).map_err(|e| e.to_string())?;
    w.tar.flush().map_err(|e| e.to_string())?;
    let Walk { items, skipped, .. } = w;
    let arch = tmp.join(ARCHIVE);
    {
        let src = BufReader::new(File::open(&raw).map_err(|e| e.to_string())?);
        let mut out = BufWriter::new(File::create(&arch).map_err(|e| e.to_string())?);
        ruzstd::encoding::compress(src, &mut out, ruzstd::encoding::CompressionLevel::Fastest);
        out.flush().map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&raw);
    let m = Manifest { format: FORMAT, id: id.into(), created: now, archive: ARCHIVE.into(), archive_sha256: sha_file(&arch)?, archive_bytes: std::fs::metadata(&arch).map_err(|e| e.to_string())?.len(), items, skipped };
    let text = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join(MANIFEST), &text).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join(MANIFEST_SUM), format!("{}  {MANIFEST}\n", hex(&Sha256::digest(text.as_bytes())))).map_err(|e| e.to_string())?;
    for f in [ARCHIVE, MANIFEST, MANIFEST_SUM] {
        set_mode(&tmp.join(f), 0o600);
    }
    set_mode(tmp, 0o700);
    Ok(m)
}

// ---------------------------------------------------------------------------------------------------------------- list, verify

/// The manifest of snapshot `id`, after checking the manifest's own checksum and its names.
pub fn read_manifest(snaps: &Path, id: &str) -> Result<Manifest, String> {
    if !id_ok(id) {
        return Err(format!("\"{id}\" is not a snapshot id"));
    }
    let dir = snaps.join(id);
    let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("{id}: {e}"))?;
    let sum = std::fs::read_to_string(dir.join(MANIFEST_SUM)).map_err(|e| format!("{id}: {e}"))?;
    if sum.split_whitespace().next() != Some(hex(&Sha256::digest(text.as_bytes())).as_str()) {
        return Err(format!("{id}: the manifest does not match its checksum"));
    }
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("{id}: {e}"))?;
    if m.format != FORMAT || m.id != id || m.archive != ARCHIVE {
        return Err(format!("{id}: a manifest this version does not understand"));
    }
    for i in &m.items {
        let sane = !i.path.is_empty() && Path::new(&i.path).components().all(|c| matches!(c, std::path::Component::Normal(_)));
        let kind_ok = matches!(i.kind.as_str(), "file" | "dir");
        let sum_ok = i.kind == "dir" || (i.sha256.len() == 64 && i.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
        if !sane || !kind_ok || !sum_ok {
            return Err(format!("{id}: the manifest names a path or checksum that is not valid ({})", i.path.chars().take(60).collect::<String>()));
        }
    }
    Ok(m)
}

/// The manifest, and the archive checked against its checksum and size.
pub fn verify(snaps: &Path, id: &str) -> Result<Manifest, String> {
    let m = read_manifest(snaps, id)?;
    let arch = snaps.join(id).join(&m.archive);
    let len = std::fs::metadata(&arch).map_err(|e| format!("{id}: {e}"))?.len();
    if len != m.archive_bytes || sha_file(&arch)? != m.archive_sha256 {
        return Err(format!("{id}: the archive does not match the manifest (damaged or changed)"));
    }
    Ok(m)
}

/// Every snapshot folder, oldest first; one that cannot be read is listed with the reason.
pub fn list(snaps: &Path) -> Vec<(String, Result<Manifest, String>)> {
    let mut ids: Vec<String> = std::fs::read_dir(snaps).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| id_ok(n)).collect();
    ids.sort();
    ids.into_iter().map(|id| (id.clone(), read_manifest(snaps, &id))).collect()
}

// ---------------------------------------------------------------------------------------------------------------- restore

#[derive(Debug, PartialEq, Eq)]
pub struct Restored {
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
}

/// Restore snapshot `id` into `target`. Without `overwrite`, any file that already exists there stops the restore before anything is
/// written. Staging is inside the snapshot's folder and is always removed.
pub fn restore(snaps: &Path, id: &str, target: &Path, overwrite: bool) -> Result<Restored, String> {
    let m = verify(snaps, id)?;
    std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
    // 1. conflicts, before anything is written (and a link on the way is a refusal)
    let mut conflicts = Vec::new();
    for i in &m.items {
        let p = cloudstorage::jail(target, &i.path)?;
        match std::fs::symlink_metadata(&p) {
            Ok(e) if i.kind == "file" || !e.is_dir() => conflicts.push(i.path.clone()),
            _ => {}
        }
    }
    if !conflicts.is_empty() && !overwrite {
        let shown: Vec<_> = conflicts.iter().take(5).cloned().collect();
        return Err(format!("{} file(s) already exist in {} (for example {}); nothing was written. Restore into a new folder, or give --overwrite", conflicts.len(), target.display(), shown.join(", ")));
    }
    // 2. unpack into staging and check every file against the manifest
    let stage = snaps.join(id).join(format!(".restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    let r = (|| -> Result<Restored, String> {
        crate::archive::untar_zst(&snaps.join(id).join(&m.archive), &stage, m.bytes() + (1 << 20))?;
        for i in m.items.iter().filter(|i| i.kind == "file") {
            let p = stage.join(&i.path);
            let md = std::fs::symlink_metadata(&p).map_err(|_| format!("{}: missing from the archive", i.path))?;
            if !md.is_file() || md.len() != i.size || sha_file(&p)? != i.sha256 {
                return Err(format!("{}: does not match the manifest; nothing was restored", i.path));
            }
        }
        // 3. copy, folder by folder, file by file (a copy beside the target, then a rename)
        let mut out = Restored { files: 0, dirs: 0, bytes: 0 };
        for i in &m.items {
            let dest = cloudstorage::jail(target, &i.path)?;
            if i.kind == "dir" {
                std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                set_mode(&dest, i.mode | 0o700);
                out.dirs += 1;
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                let tmp = dest.with_file_name(format!(".iyakki-restore-{}", std::process::id()));
                std::fs::copy(stage.join(&i.path), &tmp).map_err(|e| format!("{}: {e}", i.path))?;
                set_mode(&tmp, i.mode | 0o600);
                std::fs::rename(&tmp, &dest).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    format!("{}: {e}", i.path)
                })?;
                out.files += 1;
                out.bytes += i.size;
            }
        }
        Ok(out)
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

// ---------------------------------------------------------------------------------------------------------------- prune

/// Keep the newest `keep` snapshots (at least 1), remove the rest. Returns the ids removed (or that would be, with `dry_run`). Only
/// folders whose names are snapshot ids are touched, and only after they were read as snapshots (an unreadable one is kept and left
/// for a person to look at).
pub fn prune(snaps: &Path, keep: usize, dry_run: bool) -> Result<Vec<String>, String> {
    if keep == 0 {
        return Err("--keep must be at least 1".into());
    }
    let all: Vec<String> = list(snaps).into_iter().filter(|(_, m)| m.is_ok()).map(|(id, _)| id).collect();
    let drop = all.len().saturating_sub(keep);
    let gone: Vec<String> = all.into_iter().take(drop).collect();
    if !dry_run {
        for id in &gone {
            std::fs::remove_dir_all(snaps.join(id)).map_err(|e| format!("{id}: {e}"))?;
        }
    }
    Ok(gone)
}

// ---------------------------------------------------------------------------------------------------------------- Kubernetes

/// A CronJob that runs the snapshot (and the prune) in the tenant's namespace. The pod mounts the tenant's data claim like the service
/// does (the claim is ReadWriteOnce, so the pod is placed on the node that runs the service), and, when `backup_claim` is given, a second
/// claim for the snapshots, so they leave the volume they protect.
pub struct CronSpec<'a> {
    pub tenant: &'a str,
    /// Five fields of digits, `*`, `/`, `,` and `-`.
    pub schedule: &'a str,
    pub image: &'a str,
    pub keep: u32,
    pub backup_claim: Option<&'a str>,
}

pub fn cronjob(s: &CronSpec) -> Result<String, String> {
    if !crate::cloudtenant::dns_label(s.tenant) || s.tenant.len() > 40 {
        return Err(format!("tenant \"{}\": use lower-case letters, digits and -", s.tenant));
    }
    let fields: Vec<&str> = s.schedule.split(' ').collect();
    if fields.len() != 5 || fields.iter().any(|f| f.is_empty() || !f.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'*' | b'/' | b',' | b'-'))) {
        return Err("the schedule is five fields such as \"17 3 * * *\"".into());
    }
    if s.image.is_empty() || s.image.len() > 200 || s.image.starts_with(['-', '/', ':']) || !s.image.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/' | b':' | b'@')) {
        return Err("that is not an image name".into());
    }
    if s.keep == 0 || s.keep > 1000 {
        return Err("keep is between 1 and 1000".into());
    }
    if let Some(c) = s.backup_claim {
        if !crate::cloudtenant::dns_subdomain(c) {
            return Err(format!("backup claim \"{c}\" is not a claim name"));
        }
    }
    let (t, ns) = (s.tenant, format!("iyakki-{}", s.tenant));
    let dir_arg = if s.backup_claim.is_some() { ", \"--dir\", \"/backup\"" } else { "" };
    let (backup_mount, backup_vol) = match s.backup_claim {
        Some(c) => ("            - { name: backup, mountPath: /backup }\n".to_string(), format!("            - name: backup\n              persistentVolumeClaim: {{ claimName: {c} }}\n")),
        None => (String::new(), String::new()),
    };
    Ok(format!(
        "apiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: iyakki-snapshot\n  namespace: {ns}\n  labels:\n    app.kubernetes.io/name: iyakki-snapshot\n    iyakki.dev/tenant: {t}\nspec:\n  schedule: \"{sched}\"\n  concurrencyPolicy: Forbid\n  successfulJobsHistoryLimit: 1\n  failedJobsHistoryLimit: 3\n  jobTemplate:\n    spec:\n      backoffLimit: 1\n      template:\n        metadata:\n          labels:\n            app.kubernetes.io/name: iyakki-snapshot\n            iyakki.dev/tenant: {t}\n        spec:\n          restartPolicy: Never\n          automountServiceAccountToken: false\n          securityContext:\n            runAsNonRoot: true\n            runAsUser: 65532\n            runAsGroup: 65532\n            fsGroup: 65532\n            seccompProfile:\n              type: RuntimeDefault\n          affinity:\n            podAffinity:\n              requiredDuringSchedulingIgnoredDuringExecution:\n                - topologyKey: kubernetes.io/hostname\n                  labelSelector:\n                    matchLabels:\n                      app.kubernetes.io/name: iyakki\n                      app.kubernetes.io/instance: {t}\n          containers:\n            - name: snapshot\n              image: {img}\n              imagePullPolicy: IfNotPresent\n              args: [\"cloud\", \"snapshot\", \"create\", \"--data\", \"/var/lib/iyakki\"{dir_arg}, \"--keep\", \"{keep}\"]\n              securityContext:\n                allowPrivilegeEscalation: false\n                readOnlyRootFilesystem: true\n                capabilities:\n                  drop: [\"ALL\"]\n              resources:\n                requests: {{ cpu: 50m, memory: 64Mi }}\n                limits: {{ cpu: \"1\", memory: 256Mi }}\n              volumeMounts:\n                - {{ name: data, mountPath: /var/lib/iyakki }}\n{backup_mount}                - {{ name: tmp, mountPath: /tmp }}\n          volumes:\n            - name: data\n              persistentVolumeClaim: {{ claimName: iyakki-data }}\n{backup_vol}            - name: tmp\n              emptyDir: {{}}\n",
        sched = s.schedule,
        img = s.image,
        keep = s.keep,
    ))
}

// ---------------------------------------------------------------------------------------------------------------- command

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1e6)
}

/// `iyakki cloud snapshot create | list | verify | restore | prune | cronjob` (the arguments after `snapshot`).
pub fn cli(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let data = PathBuf::from(opt(a, "--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let files = data.join("cloud").join("files");
    let snaps = opt(a, "--dir").map(PathBuf::from).unwrap_or_else(|| data.join("cloud").join("snapshots"));
    let id = || a.get(1).filter(|x| !x.starts_with("--")).cloned().ok_or_else(|| "give the snapshot id (see: iyakki cloud snapshot list)".to_string());
    match a.first().map(String::as_str) {
        Some("create") => {
            let m = create(&files, &snaps, now())?;
            out(&format!("snapshot {}: {} files, {} of data, {} archived, in {}", m.id, m.files(), mb(m.bytes()), mb(m.archive_bytes), snaps.join(&m.id).display()));
            for s in &m.skipped {
                out(&format!("  not archived: {s}"));
            }
            if let Some(k) = opt(a, "--keep") {
                let keep: usize = k.parse().map_err(|_| "--keep takes a number".to_string())?;
                for g in prune(&snaps, keep, false)? {
                    out(&format!("  pruned {g}"));
                }
            }
            Ok(())
        }
        Some("list") => {
            let l = list(&snaps);
            if l.is_empty() {
                out(&format!("no snapshots in {}", snaps.display()));
            }
            for (id, m) in l {
                match m {
                    Ok(m) => out(&format!("{id}  {} files  {}  archive {}", m.files(), mb(m.bytes()), mb(m.archive_bytes))),
                    Err(e) => out(&format!("{id}  UNREADABLE: {e}")),
                }
            }
            Ok(())
        }
        Some("verify") => {
            let m = verify(&snaps, &id()?)?;
            out(&format!("snapshot {} is intact ({} files, archive checksum {})", m.id, m.files(), &m.archive_sha256[..12]));
            Ok(())
        }
        Some("restore") => {
            let id = id()?;
            let target = match (opt(a, "--to"), a.iter().any(|x| x == "--live")) {
                (Some(_), true) => return Err("give --to or --live, not both".into()),
                (Some(t), false) => PathBuf::from(t),
                (None, true) => files.clone(),
                (None, false) => {
                    let t = data.join("cloud").join("restored").join(&id);
                    if t.exists() {
                        return Err(format!("{} exists already; choose another folder with --to", t.display()));
                    }
                    t
                }
            };
            let r = restore(&snaps, &id, &target, a.iter().any(|x| x == "--overwrite"))?;
            out(&format!("restored {} files and {} folders ({}) into {}", r.files, r.dirs, mb(r.bytes), target.display()));
            Ok(())
        }
        Some("prune") => {
            let keep: usize = opt(a, "--keep").ok_or("usage: iyakki cloud snapshot prune --keep N [--dry-run]")?.parse().map_err(|_| "--keep takes a number".to_string())?;
            let dry = a.iter().any(|x| x == "--dry-run");
            let gone = prune(&snaps, keep, dry)?;
            out(&format!("{} {} snapshot(s)", if dry { "would remove" } else { "removed" }, gone.len()));
            for g in gone {
                out(&format!("  {g}"));
            }
            Ok(())
        }
        Some("cronjob") => {
            let tenant = opt(a, "--tenant").ok_or("usage: iyakki cloud snapshot cronjob --tenant T [--schedule \"17 3 * * *\"] [--keep 7] [--image REF] [--backup-claim NAME]")?;
            let keep = opt(a, "--keep").map(|k| k.parse::<u32>().map_err(|_| "--keep takes a number".to_string())).transpose()?.unwrap_or(7);
            let backup = opt(a, "--backup-claim");
            let image = opt(a, "--image").unwrap_or_else(|| "iyakki:latest".into());
            let schedule = opt(a, "--schedule").unwrap_or_else(|| "17 3 * * *".into());
            out(&cronjob(&CronSpec { tenant: &tenant, schedule: &schedule, image: &image, keep, backup_claim: backup.as_deref() })?);
            Ok(())
        }
        _ => Err("usage: iyakki cloud snapshot <create | list | verify ID | restore ID | prune | cronjob>  [--data DIR (default /var/lib/iyakki)] [--dir SNAPSHOT_FOLDER]\n  \
                  create [--keep N]   restore ID [--to DIR | --live] [--overwrite]   prune --keep N [--dry-run]   cronjob --tenant T [--schedule S] [--keep N] [--image REF] [--backup-claim NAME]"
            .into()),
    }
}

#[cfg(test)]
#[path = "cloudsnapshot_tests.rs"]
mod tests;

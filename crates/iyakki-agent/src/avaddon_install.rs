//! Installing the browser's audio and video add-on (Store -> Setup -> Browser audio and video, `avaddon.rs` uses the result).
//!
//! **IYAKKI neither ships nor hosts FFmpeg.** The image carries the inactive LGPL bridge plugin, `iyakki-av-check` and the recipe
//! (`/usr/share/iyakki/av-addon/`). The libraries come from the administrator, by one of three ways, tried in this order:
//!
//! 1. **A local package**: `ffmpeg-libs-*-linux-musl-<arch>.tar.gz` (the layout `package-av-addon.sh` makes) in `/local/packages/` (a link
//!    to `<data>/local/packages/`, administrators only) or, read-only, on removable media (`/media/LOCAL-PACKAGES`, `/media/*/iyakki-packages`).
//!    Scanning never follows a link out of the folder. No network is used.
//! 2. **The libraries of an installed `ffmpeg`** (the CLI catalogue's folder) when they are shared libraries with the six sonames, built for
//!    this processor and for musl, with the h264, aac and mp3 decoders and a configure line without `--enable-gpl` or `--enable-nonfree`.
//!    They are COPIED, never linked. Most vendor builds are static or glibc and fail these checks: that is the point of the checks.
//! 3. **A mirror** address the administrator typed (https only; no default exists), with an optional SHA-256.
//!
//! Whatever is chosen, the administrator first reads the review (what is inside, the licence, the patent note) and confirms
//! ([`CONFIRM_TEXT`]); nothing is installed before that click.

use std::path::{Path, PathBuf};

use crate::avaddon;

/// The local package folder as the person sees it (a link to `<data>/local/packages`).
pub const LOCAL_PACKAGES: &str = "/local/packages";
/// What the administrator confirms.
pub const CONFIRM_TEXT: &str = "I built or obtained these libraries and accept their licences and any patent obligations in my country.";
/// The honest answer for a static, single-file ffmpeg.
pub const STATIC_TEXT: &str = "Your ffmpeg is one file without shared libraries, so the browser cannot use it. Put the add-on libraries in /local/packages (see the recipe in /usr/share/iyakki/av-addon/README) or set a mirror.";
/// The command that makes the archive on a build machine.
pub const RECIPE_COMMAND: &str = "On a build machine with the IYAKKI source: WEB_AV_ADDON=1 dist/buildroot/smart-build.sh web && dist/buildroot/package-av-addon.sh (copy of the recipe: /usr/share/iyakki/av-addon/README)";

/// `<data>/local/packages`.
pub fn local_dir(data: &Path) -> PathBuf {
    data.join("local").join("packages")
}

/// The folders scanned for a package: the data volume's (writable by administrators only) and removable media (read-only).
pub fn package_dirs(data: &Path, media_root: &Path) -> Vec<PathBuf> {
    let mut v = vec![local_dir(data), media_root.join("LOCAL-PACKAGES")];
    if let Ok(rd) = std::fs::read_dir(media_root) {
        let mut m: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("iyakki-packages")).collect();
        m.sort();
        v.extend(m);
    }
    v
}

/// Does root own this file? (What the device's own service writes; not what a sandboxed shell or a normal account can leave behind.)
fn owned_by_root(f: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(f).is_ok_and(|m| m.uid() == 0)
}

/// The archives of this processor in `dirs`: plain files only (a link is skipped, so a folder cannot point outside itself), named
/// `ffmpeg-libs-<version>-linux-musl-<arch>.tar.gz`. Sorted by name; the last one is the newest by name.
pub fn find_local(dirs: &[PathBuf], arch: &str) -> Vec<PathBuf> {
    let tail = format!("-linux-musl-{arch}.tar.gz");
    let mut out = Vec::new();
    for d in dirs {
        let Ok(root) = std::fs::canonicalize(d) else { continue };
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !(n.starts_with("ffmpeg-libs-") && n.ends_with(&tail)) {
                continue;
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            // a plain file inside the (resolved) folder; never a link, never a folder
            if m.file_type().is_file() && e.path().parent() == Some(root.as_path()) {
                out.push(e.path());
            }
        }
    }
    out.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    out
}

/// What was read from an archive or an installation and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub version: String,
    pub configure: String,
    pub notice: String,
}

/// The licence checks on a NOTICE: the FFmpeg version, and a configure line that says `--disable-gpl` and `--disable-nonfree` without any
/// `--enable-gpl`, `--enable-nonfree`, libx264, libx265 or libfdk-aac.
pub fn check_notice(notice: &str) -> Result<Inspection, String> {
    let version = notice.lines().find_map(|l| l.strip_prefix("FFmpeg version:")).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).ok_or("the NOTICE does not name the FFmpeg version")?;
    let configure = notice.lines().find_map(|l| l.trim().strip_prefix("./configure")).map(|c| c.trim().to_string()).ok_or("the NOTICE has no configure line")?;
    check_configure(&configure)?;
    if !configure.contains("--disable-gpl") || !configure.contains("--disable-nonfree") {
        return Err("the configure line does not say --disable-gpl and --disable-nonfree".into());
    }
    Ok(Inspection { version, configure, notice: notice.to_string() })
}

/// A configure line must not enable GPL, non-free or the encoder libraries.
pub fn check_configure(c: &str) -> Result<(), String> {
    for bad in ["--enable-gpl", "--enable-nonfree", "--enable-libx264", "--enable-libx265", "--enable-libfdk-aac"] {
        if c.split_whitespace().any(|w| w == bad) {
            return Err(format!("it was configured with {bad}: not an LGPL-only build"));
        }
    }
    Ok(())
}

/// Unpacks `archive` into `scratch` and runs every check: sonames, legal files, plain files, the NOTICE's licence checks. Returns the unpacked
/// folder and what the NOTICE says.
pub fn inspect_archive(archive: &Path, scratch: &Path, sonames: &[String]) -> Result<(PathBuf, Inspection), String> {
    let (dest, tmp) = (scratch.join("unpacked"), scratch.join("tmp"));
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(&dest).and_then(|_| std::fs::create_dir_all(&tmp)).map_err(|e| e.to_string())?;
    crate::archive::untar_gz(archive, &dest, &tmp).map_err(|e| format!("{} cannot be unpacked: {e}", archive.display()))?;
    let tops: Vec<PathBuf> = std::fs::read_dir(&dest).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    let root = match tops.as_slice() {
        [one] if one.is_dir() => one.clone(),
        _ => return Err("the archive must hold one folder (ffmpeg-libs-<version>/)".into()),
    };
    avaddon::check_unpacked(&root, sonames)?;
    let notice = std::fs::read_to_string(root.join("NOTICE")).map_err(|e| e.to_string())?;
    let ins = check_notice(&notice)?;
    Ok((root, ins))
}

// ------------------------------------------------------------------------------------------------ an installed ffmpeg

/// What the CLI catalogue's ffmpeg folder offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliLibs {
    NotInstalled,
    /// An `ffmpeg` program and no shared libraries: [`STATIC_TEXT`].
    StaticOnly,
    /// Not usable, and why.
    Refused(String),
    /// All checks passed: the libraries to copy (soname, real file) and what was read.
    Usable { libs: Vec<(String, PathBuf)>, folder: PathBuf, configure: String },
}

/// The parts of an ELF file the checks need (64-bit little endian only).
#[derive(Debug)]
pub struct Elf {
    pub machine: u16,
    pub shared: bool,
    pub needed: Vec<String>,
}

pub fn read_elf(b: &[u8]) -> Result<Elf, String> {
    let u16at = |o: usize| b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]]));
    let u32at = |o: usize| b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()));
    let u64at = |o: usize| b.get(o..o + 8).map(|x| u64::from_le_bytes(x.try_into().unwrap()));
    if b.get(0..4) != Some(b"\x7fELF") || b.get(4) != Some(&2) || b.get(5) != Some(&1) {
        return Err("not a 64-bit little-endian ELF file".into());
    }
    let bad = || "a damaged ELF file".to_string();
    let (etype, machine) = (u16at(16).ok_or_else(bad)?, u16at(18).ok_or_else(bad)?);
    let (phoff, phentsize, phnum) = (u64at(32).ok_or_else(bad)? as usize, u16at(54).ok_or_else(bad)? as usize, u16at(56).ok_or_else(bad)? as usize);
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum.min(64) {
        let o = phoff + i * phentsize;
        let (t, off, va, sz) = (u32at(o).ok_or_else(bad)?, u64at(o + 8).ok_or_else(bad)?, u64at(o + 16).ok_or_else(bad)?, u64at(o + 32).ok_or_else(bad)?);
        match t {
            1 => loads.push((off, va, sz)),
            2 => dynamic = Some((off as usize, sz as usize)),
            _ => {}
        }
    }
    let to_off = |v: u64| loads.iter().find(|(_, va, sz)| v >= *va && v < va + sz).map(|(off, va, _)| (off + (v - va)) as usize);
    let mut needed_offs = Vec::new();
    let mut strtab = None;
    if let Some((doff, dsz)) = dynamic {
        for k in 0..dsz / 16 {
            let (tag, val) = (u64at(doff + k * 16).ok_or_else(bad)?, u64at(doff + k * 16 + 8).ok_or_else(bad)?);
            match tag {
                0 => break,
                1 => needed_offs.push(val as usize),
                5 => strtab = to_off(val),
                _ => {}
            }
        }
    }
    let mut needed = Vec::new();
    if let Some(st) = strtab {
        for n in needed_offs {
            let s = b.get(st + n..).ok_or_else(bad)?;
            needed.push(String::from_utf8_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())]).into_owned());
        }
    }
    Ok(Elf { machine, shared: etype == 3, needed })
}

fn machine_code(machine: &str) -> Option<u16> {
    match machine {
        "x86_64" => Some(0x3e),
        "aarch64" => Some(0xb7),
        _ => None,
    }
}

/// Does `hay` contain `needle`?
fn has(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// The configure line a libavcodec carries (the NUL-delimited string around `--prefix=`).
pub fn configure_in(libavcodec: &[u8]) -> Option<String> {
    let at = libavcodec.windows(9).position(|w| w == b"--prefix=")?;
    let start = libavcodec[..at].iter().rposition(|c| *c == 0).map_or(0, |p| p + 1);
    let end = libavcodec[at..].iter().position(|c| *c == 0).map_or(libavcodec.len(), |p| at + p);
    Some(String::from_utf8_lossy(&libavcodec[start..end]).into_owned())
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_dir() => walk(&p, depth - 1, out),
            Ok(_) => out.push(p),
            Err(_) => {}
        }
    }
}

/// Looks in the folder of the CLI catalogue's ffmpeg for libraries the engine can use. `system_dirs` are where the image's own libraries live.
pub fn check_cli_ffmpeg(folder: &Path, machine: &str, sonames: &[String], system_dirs: &[PathBuf]) -> CliLibs {
    let Ok(root) = std::fs::canonicalize(folder) else { return CliLibs::NotInstalled };
    let mut files = Vec::new();
    walk(&root, 6, &mut files);
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // a soname resolves to a file that stays inside the folder (a link to a versioned file is fine; one out of the folder is not)
    let mut libs: Vec<(String, PathBuf)> = Vec::new();
    for s in sonames {
        let found = files.iter().find(|p| name(p) == *s).and_then(|p| std::fs::canonicalize(p).ok()).filter(|r| r.starts_with(&root) && r.is_file());
        if let Some(r) = found {
            libs.push((s.clone(), r));
        }
    }
    if libs.is_empty() {
        return if files.iter().any(|p| name(p) == "ffmpeg") { CliLibs::StaticOnly } else { CliLibs::NotInstalled };
    }
    if libs.len() < sonames.len() {
        let miss: Vec<&str> = sonames.iter().filter(|s| !libs.iter().any(|(l, _)| l == *s)).map(|s| s.as_str()).collect();
        return CliLibs::Refused(format!("your ffmpeg lacks {} (the browser engine needs exactly {})", miss.join(", "), sonames.join(", ")));
    }
    let Some(code) = machine_code(machine) else { return CliLibs::Refused(format!("no libraries are checked for the processor {machine}")) };
    let local_names: Vec<String> = files.iter().map(|p| name(p)).collect();
    let mut configure = String::new();
    for (so, path) in &libs {
        let Ok(bytes) = std::fs::read(path) else { return CliLibs::Refused(format!("{so} cannot be read")) };
        let elf = match read_elf(&bytes) {
            Ok(e) => e,
            Err(e) => return CliLibs::Refused(format!("{so}: {e}")),
        };
        if !elf.shared || elf.machine != code {
            return CliLibs::Refused(format!("{so} is not a shared library for this processor ({machine})"));
        }
        for n in &elf.needed {
            if n == "libc.so.6" || n.starts_with("ld-linux") || n == "libm.so.6" || n == "libpthread.so.0" || n == "libdl.so.2" || n == "librt.so.1" || n.starts_with("libstdc++") || n.starts_with("libgcc_s") {
                return CliLibs::Refused(format!("{so} is built for glibc (it needs {n}); IYAKKI images use musl"));
            }
            let ok = sonames.contains(n) || n == "libc.so" || n.starts_with("libc.musl-") || local_names.contains(n) || system_dirs.iter().any(|d| d.join(n).exists());
            if !ok {
                return CliLibs::Refused(format!("{so} needs {n}, which is not there"));
            }
        }
        if so.starts_with("libavcodec") {
            // the three decoders (codec names are stored in the library), and the licence of the build
            let mp3 = has(&bytes, b"\0mp3float\0") || has(&bytes, b"\0mp3\0");
            if !(has(&bytes, b"\0h264\0") && has(&bytes, b"\0aac\0") && mp3) {
                return CliLibs::Refused("libavcodec has no h264, aac and mp3 decoders".into());
            }
            let Some(c) = configure_in(&bytes) else { return CliLibs::Refused("the build's configure line cannot be read from libavcodec, so its licence cannot be checked".into()) };
            if let Err(e) = check_configure(&c) {
                return CliLibs::Refused(e);
            }
            configure = c;
        }
    }
    CliLibs::Usable { libs, folder: root, configure }
}

/// Copies the usable libraries (real files, never links) into a new tree with the legal files found next to them and a NOTICE made here;
/// returns the tree's folder (under `scratch`).
pub fn stage_cli_libs(libs: &[(String, PathBuf)], folder: &Path, configure: &str, scratch: &Path) -> Result<PathBuf, String> {
    let root = scratch.join("cli-libs");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("lib")).map_err(|e| e.to_string())?;
    for (so, real) in libs {
        std::fs::copy(real, root.join("lib").join(so)).map_err(|e| format!("could not copy {so}: {e}"))?;
    }
    let mut files = Vec::new();
    walk(folder, 3, &mut files);
    let mut copied = Vec::new();
    for f in files {
        let n = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if (n.starts_with("COPYING.LGPL") || n.starts_with("LICENSE")) && std::fs::symlink_metadata(&f).is_ok_and(|m| m.file_type().is_file()) && !copied.contains(&n) {
            std::fs::copy(&f, root.join(&n)).map_err(|e| e.to_string())?;
            copied.push(n);
        }
    }
    let notice = format!("FFmpeg version: unknown (libraries copied from this machine's own ffmpeg installation, sonames {})\nLicence: not stated by the build; IYAKKI checked only that its configure line does not enable GPL or non-free parts.\nConfigure line read from libavcodec:\n  ./configure {}\nLegal files found next to the libraries: {}\nPatents: H.264 and AAC are patent-licensed formats in some countries; using them is the administrator's choice.\n", libs.iter().map(|l| l.0.as_str()).collect::<Vec<_>>().join(" "), configure.trim_start_matches("./configure").trim(), if copied.is_empty() { "none".into() } else { copied.join(", ") });
    std::fs::write(root.join("NOTICE"), notice).map_err(|e| e.to_string())?;
    std::fs::write(root.join("SOURCE-OFFER.txt"), "These libraries were copied from the ffmpeg installation of this machine, which the administrator installed. The source offer is that build's vendor's, not IYAKKI's.\n").map_err(|e| e.to_string())?;
    Ok(root)
}

// ------------------------------------------------------------------------------------------------ the mirror

/// An address the administrator typed, and an optional SHA-256. There is no default.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mirror {
    pub url: String,
    pub sha256: String,
}

/// `https://host/path` only: no user info, no port tricks, no spaces or control characters, no redirect assumptions.
pub fn validate_mirror(url: &str, sha256: &str) -> Result<Mirror, String> {
    let rest = url.strip_prefix("https://").ok_or("the mirror address must start with https://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) {
        return Err("the mirror address is not a plain https://host/path address".into());
    }
    let s = sha256.trim().to_ascii_lowercase();
    if !s.is_empty() && !(s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err("the SHA-256 must be 64 hexadecimal characters (or empty)".into());
    }
    Ok(Mirror { url: url.trim().to_string(), sha256: s })
}

fn mirror_path(data: &Path) -> PathBuf {
    avaddon::addon_dir(data).parent().unwrap_or(data).join("mirror.json")
}

pub fn save_mirror(data: &Path, url: &str, sha256: &str) -> Result<(), String> {
    let m = validate_mirror(url, sha256)?;
    let p = mirror_path(data);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, serde_json::to_vec(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn load_mirror(data: &Path) -> Option<Mirror> {
    let m: Mirror = serde_json::from_slice(&std::fs::read(mirror_path(data)).ok()?).ok()?;
    validate_mirror(&m.url, &m.sha256).ok()
}

// ------------------------------------------------------------------------------------------------ the plan and the run

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    InstalledFfmpeg { libs: Vec<(String, PathBuf)>, folder: PathBuf, configure: String },
    Mirror(Mirror),
}

/// What an install would do, and the review the administrator reads first.
pub struct Plan {
    pub source: Option<Source>,
    pub review: Vec<String>,
}

pub struct Places {
    pub data: PathBuf,
    pub media_root: PathBuf,
    /// The folder `iyakki dev files push` writes to (developer mode, files scope, set up by an administrator). An archive there counts only when
    /// it belongs to root, which is how the device's own service writes it: a file made by the sandboxed developer shell (user nobody) is ignored.
    pub dev_drop: Option<PathBuf>,
    /// `iyakki-packages/` on the boot volume (mounted read-only at `/.stage1/boot`): a package put on an install stick travels with the clone of the
    /// boot volume to every disk installed from that stick, and is found there too. Whoever can write the boot volume is physically at the machine.
    pub boot_packages: Option<PathBuf>,
    pub machine: String,
    pub system_dirs: Vec<PathBuf>,
    pub sonames: Vec<String>,
}

impl Places {
    /// The places of this machine.
    pub fn here(data: &Path) -> Places {
        let abi = std::fs::read_to_string(avaddon::ABI_FILE).unwrap_or_default();
        Places { data: data.into(), media_root: "/media".into(), dev_drop: Some(crate::devmode::files::DEV_TMP.into()), boot_packages: Some("/.stage1/boot/iyakki-packages".into()), machine: std::env::consts::ARCH.into(), system_dirs: vec!["/lib".into(), "/usr/lib".into()], sonames: avaddon::required_sonames(&abi) }
    }
    fn cli_folder(&self) -> PathBuf {
        self.data.join("cli").join("pkgs").join("ffmpeg")
    }
    fn scratch(&self) -> PathBuf {
        self.data.join("addons").join(".inspect")
    }
}

/// Decides the source and writes the review. Reads files only; the mirror is never contacted here.
pub fn plan(p: &Places) -> Plan {
    let mut review = vec!["Browser audio and video: H.264 video and AAC sound for the BROWSER tile.".to_string(), "IYAKKI does not provide FFmpeg: it ships only the inactive bridge plugin and the recipe. You supply the libraries.".to_string(), String::new()];
    // created at first use, for administrators only
    if std::fs::create_dir_all(local_dir(&p.data)).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(local_dir(&p.data), std::fs::Permissions::from_mode(0o750));
    }
    let mut local = find_local(&package_dirs(&p.data, &p.media_root), &p.machine);
    if let Some(d) = &p.dev_drop {
        local.extend(find_local(std::slice::from_ref(d), &p.machine).into_iter().filter(|f| owned_by_root(f)));
        local.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    }
    if let Some(d) = &p.boot_packages {
        local.extend(find_local(std::slice::from_ref(d), &p.machine));
        local.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    }
    let mut last_problem = None;
    for archive in local.iter().rev() {
        match inspect_archive(archive, &p.scratch(), &p.sonames) {
            Ok((root, ins)) => {
                review.push(format!("Found: {}", archive.display()));
                review.push(format!("FFmpeg version: {}", ins.version));
                review.push("Checks: every library the engine needs is there as a plain file; the licence texts and the source offer are there; configured --disable-gpl --disable-nonfree (read from the archive's NOTICE).".into());
                review.push(String::new());
                review.extend(ins.notice.lines().map(|l| l.to_string()));
                review.push(String::new());
                review.push(CONFIRM_TEXT.to_string());
                let _ = std::fs::remove_dir_all(root.parent().and_then(Path::parent).unwrap_or(&root));
                return Plan { source: Some(Source::Local(archive.clone())), review };
            }
            Err(e) => last_problem = Some(format!("{}: {e}", archive.display())),
        }
    }
    let _ = std::fs::remove_dir_all(p.scratch());
    if let Some(e) = &last_problem {
        review.push(format!("A package was found but is refused: {e}"));
    }
    match check_cli_ffmpeg(&p.cli_folder(), &p.machine, &p.sonames, &p.system_dirs) {
        CliLibs::Usable { libs, folder, configure } => {
            review.push("Use the libraries of your installed ffmpeg.".into());
            review.push(format!("They are copied (not linked) from {} to {}; checks passed: the six sonames, this processor, musl, the h264, aac and mp3 decoders, and a configure line without --enable-gpl or --enable-nonfree:", folder.display(), avaddon::addon_dir(&p.data).display()));
            review.push(format!("  ./configure {}", configure.trim_start_matches("./configure").trim()));
            review.push(String::new());
            review.push(CONFIRM_TEXT.to_string());
            return Plan { source: Some(Source::InstalledFfmpeg { libs, folder, configure }), review };
        }
        CliLibs::StaticOnly => review.push(STATIC_TEXT.to_string()),
        CliLibs::Refused(why) => review.push(format!("Your installed ffmpeg cannot be used: {why}.")),
        CliLibs::NotInstalled => {}
    }
    if let Some(m) = load_mirror(&p.data) {
        review.push(format!("Mirror set by an administrator: {}", m.url));
        review.push(if m.sha256.is_empty() { "No SHA-256 is set for it: the file is not pinned. The archive is still checked in full and its NOTICE is read before anything is installed.".to_string() } else { format!("Pinned SHA-256: {}", m.sha256) });
        review.push(String::new());
        review.push(CONFIRM_TEXT.to_string());
        return Plan { source: Some(Source::Mirror(m)), review };
    }
    review.push(format!("Nothing to install from. Put ffmpeg-libs-<version>-linux-musl-{}.tar.gz in {LOCAL_PACKAGES}/ (or on a stick named LOCAL-PACKAGES), or set a mirror address (https, inside your organisation) in Settings -> Browser add-on.", p.machine));
    review.push(RECIPE_COMMAND.to_string());
    Plan { source: None, review }
}

/// Installs from the plan's source (the caller has the administrator's confirmation). `fetch(url, file)` downloads the mirror's file and
/// returns its SHA-256; it is called only for a mirror, so a local package or an installed ffmpeg uses no network.
pub fn run(p: &Places, confirmed: bool, fetch: &dyn Fn(&str, &Path) -> Result<String, String>) -> Result<String, String> {
    if !confirmed {
        return Err("the administrator has not confirmed the licences".into());
    }
    let plan = plan(p);
    let dest = avaddon::addon_dir(&p.data);
    let scratch = p.scratch();
    let out = match plan.source {
        None => Err(plan.review.iter().filter(|l| !l.is_empty()).cloned().collect::<Vec<_>>().join(" ")),
        Some(Source::Local(a)) => inspect_archive(&a, &scratch, &p.sonames).and_then(|(root, ins)| {
            avaddon::install_tree(&root, &dest).map(|_| {
                // a package built here (avbuild.rs) carries the stamp of the map entry and GStreamer it was built for
                if let Some(n) = a.file_name() {
                    crate::avbuild::stamp_after_install(&p.data, &n.to_string_lossy(), &dest);
                }
                format!("FFmpeg {} from {}", ins.version, a.display())
            })
        }),
        Some(Source::InstalledFfmpeg { libs, folder, configure }) => stage_cli_libs(&libs, &folder, &configure, &scratch).and_then(|root| avaddon::install_tree(&root, &dest)).map(|_| "libraries of the installed ffmpeg".to_string()),
        Some(Source::Mirror(m)) => {
            std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
            let file = scratch.join("mirror.tar.gz");
            fetch(&m.url, &file).and_then(|got| {
                if !m.sha256.is_empty() && got != m.sha256 {
                    return Err(format!("the download does not match the SHA-256 you set (expected {}…, got {}…); nothing was installed", &m.sha256[..12], &got[..12.min(got.len())]));
                }
                let dir = scratch.join("check");
                inspect_archive(&file, &dir, &p.sonames).and_then(|(root, ins)| avaddon::install_tree(&root, &dest).map(|_| format!("FFmpeg {} from your mirror", ins.version)))
            })
        }
    };
    let _ = std::fs::remove_dir_all(&scratch);
    out
}

/// The check before the install is recorded: the libraries now in the add-on folder pass the same check an archive must pass (every
/// soname a real file, the licence texts and the source offer beside them), and the source is named: `built here` (the package this
/// IYAKKI built in k3s, BUILD-STAMP), `supplied` (a package, a mirror) or `existing ffmpeg` (the libraries of the installed ffmpeg).
/// `what` is the line [`run`] returned.
pub fn verify_installed(data: &Path, sonames: &[String], what: &str) -> Result<String, String> {
    let dir = avaddon::addon_dir(data);
    avaddon::check_unpacked(&dir, sonames).map_err(|e| format!("the installed add-on in {} fails its check: {e}", dir.display()))?;
    Ok(format!("verified: {} library file(s) and the licence files are in place; source: {}", sonames.len(), source_name(&dir, what)))
}

fn source_name(dir: &Path, what: &str) -> &'static str {
    if dir.join("BUILD-STAMP").is_file() {
        "built here"
    } else if what.starts_with("libraries of the installed ffmpeg") {
        "existing ffmpeg"
    } else {
        "supplied"
    }
}

/// What Setup shows for the item: the installed add-on's version line, if its NOTICE is there.
pub fn installed_note(data: &Path) -> Option<String> {
    let n = std::fs::read_to_string(avaddon::addon_dir(data).join("NOTICE")).ok()?;
    Some(n.lines().next().unwrap_or("installed").trim().to_string())
}

//! Platform services that run a program built on this machine (the private store's server): where that program comes from.
//!
//! A `FROM scratch` image can only hold a **static** executable of the **cluster's CPU architecture**. The program that is running
//! (the desktop window, or any dynamically linked build) can never be it, so it is never chosen. The order is:
//!
//! 1. shipped with the install (`/usr/share/iyakki/<sub>/<arch>/<file>` on the images, `<sub>/` beside the window binary or in the
//!    release tarball),
//! 2. the cache in the services directory (`<data>/<sub>/`), where a chosen or built file is kept,
//! 3. the source tree's `target/<musl triple>/release/<file>` when the tree is found,
//! 4. built now: a detached job (`cargo build --release --target <musl triple> -p <package>`) when the musl target, a linker and the
//!    source tree exist,
//! 5. otherwise a plain list of what is missing, and a file the person points at.
//!
//! A file is judged by reading its ELF headers (no program interpreter, no `DT_NEEDED`, the right machine); it is never run.
//! Nothing here starts a process except through the [`Runner`](crate::wsexec::Runner) it is given (`rustup`, `rustc`).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::wsexec::Runner;

// ------------------------------------------------------------------------------------------------ architectures

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// From a Kubernetes node's `architecture` (`amd64`, `arm64`) or a `uname -m` word.
    pub fn parse(s: &str) -> Option<Arch> {
        match s.trim() {
            "amd64" | "x86_64" | "x86-64" => Some(Arch::X86_64),
            "arm64" | "aarch64" => Some(Arch::Aarch64),
            _ => None,
        }
    }
    pub fn host() -> Arch {
        Arch::parse(std::env::consts::ARCH).unwrap_or(Arch::X86_64)
    }
    /// `x86_64` / `aarch64`: the folder and file-name word.
    pub fn uname(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
    /// `amd64` / `arm64`: the container and Kubernetes word.
    pub fn node(self) -> &'static str {
        match self {
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "arm64",
        }
    }
    pub fn triple(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64-unknown-linux-musl",
            Arch::Aarch64 => "aarch64-unknown-linux-musl",
        }
    }
    pub fn from_machine(m: u16) -> Option<Arch> {
        match m {
            62 => Some(Arch::X86_64),
            183 => Some(Arch::Aarch64),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------------------------------------ the ELF reader

/// What the headers of an executable say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElfInfo {
    pub machine: u16,
    pub arch: Option<Arch>,
    /// No program interpreter and no needed library: it can run in an image with nothing else in it.
    pub is_static: bool,
    /// Why it is not static (the interpreter or the first needed library).
    pub dynamic_because: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfError {
    Unreadable(String),
    NotElf,
    Truncated,
    Unsupported(String),
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfError::Unreadable(e) => write!(f, "cannot be read ({e})"),
            ElfError::NotElf => write!(f, "is not a Linux executable (no ELF header)"),
            ElfError::Truncated => write!(f, "is cut short (the ELF headers are incomplete)"),
            ElfError::Unsupported(w) => write!(f, "is not supported ({w})"),
        }
    }
}

const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_NEEDED: u64 = 1;
/// Most program headers and dynamic entries looked at; a real binary has a dozen.
const MAX_PH: u64 = 256;
const MAX_DYN: u64 = 4096;

/// Read the headers of an ELF stream (64-bit, little-endian: the only kind a x86_64 or aarch64 Linux cluster runs).
pub fn parse_elf<R: Read + Seek>(r: &mut R) -> Result<ElfInfo, ElfError> {
    let mut h = [0u8; 64];
    let mut got = 0;
    while got < h.len() {
        match r.read(&mut h[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(ElfError::Unreadable(e.to_string())),
        }
    }
    if got < 4 || &h[..4] != b"\x7fELF" {
        return Err(ElfError::NotElf);
    }
    if got < 64 {
        return Err(ElfError::Truncated);
    }
    if h[4] != 2 {
        return Err(ElfError::Unsupported("32-bit".into()));
    }
    if h[5] != 1 {
        return Err(ElfError::Unsupported("big-endian".into()));
    }
    let u16at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let u64at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
    let machine = u16at(18);
    let (phoff, phentsize, phnum) = (u64at(&h, 32), u16at(54) as u64, u16at(56) as u64);
    let mut info = ElfInfo { machine, arch: Arch::from_machine(machine), is_static: true, dynamic_because: None };
    if phnum == 0 {
        return Ok(info); // no program headers at all: nothing is loaded dynamically
    }
    if phentsize < 56 || phnum > MAX_PH {
        return Err(ElfError::Unsupported("odd program header table".into()));
    }
    let mut dynamic: Option<(u64, u64)> = None;
    for i in 0..phnum {
        let mut ph = [0u8; 56];
        r.seek(SeekFrom::Start(phoff + i * phentsize)).map_err(|_| ElfError::Truncated)?;
        r.read_exact(&mut ph).map_err(|_| ElfError::Truncated)?;
        match u32::from_le_bytes([ph[0], ph[1], ph[2], ph[3]]) {
            PT_INTERP => {
                let (off, len) = (u64at(&ph, 8), u64at(&ph, 32).min(256));
                let mut name = vec![0u8; len as usize];
                let named = r.seek(SeekFrom::Start(off)).is_ok() && r.read_exact(&mut name).is_ok();
                let path = if named { String::from_utf8_lossy(name.split(|b| *b == 0).next().unwrap_or(&[])).to_string() } else { String::new() };
                info.is_static = false;
                info.dynamic_because = Some(if path.is_empty() { "it names a program interpreter (a dynamic loader)".to_string() } else { format!("it needs the dynamic loader {path}") });
                return Ok(info);
            }
            PT_DYNAMIC => dynamic = Some((u64at(&ph, 8), u64at(&ph, 32))),
            _ => {}
        }
    }
    // a static-pie executable has a dynamic section too (for its own relocations) but needs no library
    if let Some((off, size)) = dynamic {
        let n = (size / 16).min(MAX_DYN);
        r.seek(SeekFrom::Start(off)).map_err(|_| ElfError::Truncated)?;
        for _ in 0..n {
            let mut e = [0u8; 16];
            r.read_exact(&mut e).map_err(|_| ElfError::Truncated)?;
            let tag = u64at(&e, 0);
            if tag == 0 {
                break;
            }
            if tag == DT_NEEDED {
                info.is_static = false;
                info.dynamic_because = Some("it lists shared libraries it needs (DT_NEEDED)".into());
                return Ok(info);
            }
        }
    }
    Ok(info)
}

/// The headers of the file at `path`.
pub fn read_elf(path: &Path) -> Result<ElfInfo, ElfError> {
    let mut f = std::fs::File::open(path).map_err(|e| ElfError::Unreadable(e.to_string()))?;
    if !f.metadata().map(|m| m.is_file()).unwrap_or(false) {
        return Err(ElfError::Unreadable("not a regular file".into()));
    }
    parse_elf(&mut f)
}

/// The verdict on one file for a wanted architecture: `Ok` only for a static executable of that machine.
pub fn judge(path: &Path, want: Arch) -> Result<ElfInfo, String> {
    let name = path.display();
    let i = read_elf(path).map_err(|e| format!("{name} {e}"))?;
    if !i.is_static {
        return Err(format!("{name} is dynamically linked ({}), so it cannot run in a FROM scratch image", i.dynamic_because.clone().unwrap_or_default()));
    }
    match i.arch {
        Some(a) if a == want => Ok(i),
        Some(a) => Err(format!("{name} is built for {} but the cluster is {}", a.uname(), want.uname())),
        None => Err(format!("{name} is built for another CPU (ELF machine {}), not {}", i.machine, want.uname())),
    }
}

// ------------------------------------------------------------------------------------------------ what a service needs

/// A program a service builds its image from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalBinary {
    /// The file name, e.g. `iyakki-store-server`.
    pub file: &'static str,
    /// The cargo package that builds it.
    pub package: &'static str,
    /// The folder under the data directory (the cache) and under `/usr/share/iyakki/` (shipped).
    pub sub: &'static str,
    /// The name an older cache used for the same program (any static `iyakki` works for the store).
    pub legacy: &'static [&'static str],
    /// What to read.
    pub docs: &'static str,
}

/// Where to look; built from the machine ([`Places::real`]) or by a test.
#[derive(Clone, Debug, Default)]
pub struct Places {
    /// Folders holding shipped copies, in order. Each is searched as `<dir>/<arch>/<file>`, `<dir>/<file>-<arch>` and `<dir>/<file>`.
    pub shipped: Vec<PathBuf>,
    /// The cache folder (`<data>/<sub>`).
    pub cache: PathBuf,
    /// The source tree, if found.
    pub source: Option<PathBuf>,
    /// Replaces the build command (tests give a harmless one); the output must still be at `target/<triple>/release/<file>`.
    pub build_override: Option<Vec<String>>,
}

impl Places {
    pub fn real(data: &Path, lb: &LocalBinary) -> Places {
        let mut shipped = vec![PathBuf::from("/usr/share/iyakki").join(lb.sub)];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                shipped.push(d.join(lb.sub));
                shipped.push(d.to_path_buf());
            }
        }
        Places { shipped, cache: data.join(lb.sub), source: find_source(data, lb), build_override: None }
    }
}

/// The source tree: `$IYAKKI_SOURCE`, else the folder written in `<data>/<sub>/source` (the word `none` turns the search off), else the
/// first folder above the running program or the current folder that holds the package.
pub fn find_source(data: &Path, lb: &LocalBinary) -> Option<PathBuf> {
    let is_root = |d: &Path| d.join("Cargo.toml").is_file() && d.join("crates").join(lb.package).join("Cargo.toml").is_file();
    if let Some(s) = std::env::var_os("IYAKKI_SOURCE").map(PathBuf::from) {
        if is_root(&s) {
            return Some(s);
        }
    }
    if let Ok(t) = std::fs::read_to_string(data.join(lb.sub).join("source")) {
        let t = t.trim();
        if t == "none" {
            return None;
        }
        let p = expand_typed(t);
        if is_root(&p) {
            return Some(p);
        }
    }
    let starts = [std::env::current_exe().ok(), std::env::current_dir().ok()];
    starts.into_iter().flatten().find_map(|s| s.ancestors().skip(1).take(8).find(|d| is_root(d)).map(Path::to_path_buf))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Shipped,
    Cache,
    Source,
}

impl Origin {
    pub fn word(self) -> &'static str {
        match self {
            Origin::Shipped => "shipped with this install",
            Origin::Cache => "kept in your IYAKKI data folder",
            Origin::Source => "built in the source tree",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub origin: Origin,
    pub arch: Arch,
}

/// How to build it here, or what stops that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildState {
    Ready(BuildPlan),
    /// Each entry is one missing thing, with the exact way to get it.
    Missing(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildPlan {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// Where the finished file will be.
    pub output: PathBuf,
    pub arch: Arch,
}

#[derive(Clone, Debug)]
pub struct Discovery {
    pub want: Arch,
    pub found: Option<Found>,
    /// Files that exist but cannot be used, with the reason (dynamic, wrong CPU, not ELF).
    pub rejected: Vec<(PathBuf, String)>,
    /// Static files for another CPU: the person may have the wrong one.
    pub other_arch: Vec<(PathBuf, Arch)>,
    /// Every place looked at, in order.
    pub searched: Vec<PathBuf>,
    pub build: BuildState,
}

/// Names tried inside one folder, in order.
fn names(lb: &LocalBinary, arch: Arch, dir_has_arch: bool) -> Vec<String> {
    let mut v = Vec::new();
    if dir_has_arch {
        v.push(format!("{}/{}", arch.uname(), lb.file));
    }
    v.push(format!("{}-{}", lb.file, arch.uname()));
    v.push(lb.file.to_string());
    for l in lb.legacy {
        v.push((*l).to_string());
    }
    v
}

/// The first usable file for `want`, and everything learned on the way.
pub fn discover(run: &dyn Runner, lb: &LocalBinary, places: &Places, want: Arch) -> Discovery {
    let mut d = Discovery { want, found: None, rejected: vec![], other_arch: vec![], searched: vec![], build: BuildState::Missing(vec![]) };
    let mut spots: Vec<(Origin, PathBuf)> = Vec::new();
    for dir in &places.shipped {
        for n in names(lb, want, true).iter().take(3) {
            spots.push((Origin::Shipped, dir.join(n)));
        }
    }
    for n in names(lb, want, false) {
        spots.push((Origin::Cache, places.cache.join(n)));
    }
    if let Some(src) = &places.source {
        spots.push((Origin::Source, src.join("target").join(want.triple()).join("release").join(lb.file)));
    }
    for (origin, p) in spots {
        if !p.is_file() {
            continue;
        }
        d.searched.push(p.clone());
        match judge(&p, want) {
            Ok(i) => {
                d.found = Some(Found { path: p, origin, arch: i.arch.unwrap_or(want) });
                break;
            }
            Err(why) => {
                if let Ok(i) = read_elf(&p) {
                    if let (true, Some(a)) = (i.is_static, i.arch) {
                        d.other_arch.push((p.clone(), a));
                    }
                }
                d.rejected.push((p, why));
            }
        }
    }
    if d.found.is_none() {
        d.build = build_state(run, lb, places, want);
    }
    d
}

fn sysroot_has_lld(run: &dyn Runner) -> bool {
    let o = run.run(Path::new("/"), &["rustc".into(), "--print".into(), "sysroot".into()], &[], std::time::Duration::from_secs(10), 4096, None);
    if !o.ok() {
        return false;
    }
    let root = PathBuf::from(o.stdout.trim());
    std::fs::read_dir(root.join("lib/rustlib")).map(|rd| rd.flatten().any(|e| e.path().join("bin/rust-lld").is_file() || e.path().join("bin/gcc-ld/ld.lld").is_file())).unwrap_or(false)
}

/// Can the musl build run here? Needs the source tree, `cargo`, the installed musl target and a linker.
pub fn build_state(run: &dyn Runner, lb: &LocalBinary, places: &Places, want: Arch) -> BuildState {
    let mut missing = Vec::new();
    let triple = want.triple();
    let Some(src) = places.source.clone() else {
        return BuildState::Missing(vec![format!("the IYAKKI source tree (it holds crates/{}); building needs it. Put the prebuilt file in place instead, see below", lb.package)]);
    };
    if !run.has("cargo") {
        missing.push("cargo (the Rust build tool): install Rust from rustup.rs".to_string());
    } else {
        let o = run.run(&src, &["rustup".into(), "target".into(), "list".into(), "--installed".into()], &[], std::time::Duration::from_secs(20), 16 * 1024, None);
        if !(o.ok() && o.stdout.lines().any(|l| l.trim() == triple)) {
            missing.push(format!("the Rust target {triple}: run `rustup target add {triple}`"));
        }
    }
    let host = Arch::host();
    let cross_ok = || {
        std::env::var_os("IYAKKI_AARCH64_CC").is_some()
            || run.has("aarch64-linux-musl-gcc")
            || run.has("aarch64-linux-gnu-gcc")
            || std::fs::read_dir(src.join("target/buildroot-libre-aarch64/host/bin")).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with("-linux-musl-gcc"))).unwrap_or(false)
    };
    let linker = if want == host { run.has("musl-gcc") || run.has("cc") || run.has("gcc") || sysroot_has_lld(run) } else { cross_ok() };
    if !linker {
        missing.push(if want == host {
            "a linker: install a C compiler (gcc, or musl-tools for musl-gcc)".to_string()
        } else {
            format!("a cross C compiler for {triple}: install gcc-aarch64-linux-gnu, or build the arm image once (./run.sh build-arm), or set IYAKKI_AARCH64_CC; or build on a {} machine", want.uname())
        });
    }
    if !missing.is_empty() {
        return BuildState::Missing(missing);
    }
    // the script sets the cross linker where one is needed; without it, plain cargo
    let script = src.join("dist/store-server/build-static.sh");
    let default: Vec<String> = if script.is_file() && run.has("bash") {
        vec!["bash".into(), script.display().to_string(), want.uname().into()]
    } else {
        ["cargo", "build", "--release", "--locked", "--target", triple, "-p", lb.package].iter().map(|s| s.to_string()).collect()
    };
    let argv = places.build_override.clone().unwrap_or(default);
    BuildState::Ready(BuildPlan { argv, cwd: src.clone(), output: src.join("target").join(triple).join("release").join(lb.file), arch: want })
}

/// Copy a file the person chose into the cache, after checking it. It is never run. Returns where it is kept.
pub fn adopt(chosen: &Path, lb: &LocalBinary, places: &Places, want: Arch) -> Result<Found, String> {
    judge(chosen, want)?;
    std::fs::create_dir_all(&places.cache).map_err(|e| format!("{}: {e}", places.cache.display()))?;
    let dest = places.cache.join(format!("{}-{}", lb.file, want.uname()));
    let tmp = places.cache.join(format!(".{}.part", lb.file));
    std::fs::copy(chosen, &tmp).map_err(|e| format!("copying {}: {e}", chosen.display()))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(Found { path: dest, origin: Origin::Cache, arch: want })
}

/// `~/x` and relative-free paths typed by a person.
pub fn expand_typed(t: &str) -> PathBuf {
    let t = t.trim().trim_matches('"').trim_matches('\'');
    match t.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|| PathBuf::from(t)),
        None => PathBuf::from(t),
    }
}

impl Discovery {
    /// The plain statement of what is missing and the ways out, for the wizard card and the assistant.
    pub fn problem(&self, lb: &LocalBinary, cache: &Path) -> String {
        let mut t = format!("No static {} for {} was found.", lb.file, self.want.uname());
        for (_, why) in &self.rejected {
            t.push_str(&format!("\n- not used: {why}"));
        }
        for (p, a) in &self.other_arch {
            t.push_str(&format!("\n- {} is a static {} build, but this cluster needs {}: build or download the {} one", p.display(), a.uname(), self.want.uname(), self.want.uname()));
        }
        match &self.build {
            BuildState::Ready(b) => t.push_str(&format!("\nIt can be built here: `{}` (Build now).", b.argv.join(" "))),
            BuildState::Missing(m) => {
                t.push_str("\nBuilding it here is not possible yet. Missing:");
                for x in m {
                    t.push_str(&format!("\n- {x}"));
                }
            }
        }
        t.push_str(&format!(
            "\nOr: download {}-{} from the release page (it is signed with the release) and choose it, or put it at {}; or build the image yourself from dist/store-server/Dockerfile ({}).",
            lb.file,
            self.want.uname(),
            cache.join(format!("{}-{}", lb.file, self.want.uname())).display(),
            lb.docs
        ));
        t
    }
}

#[cfg(test)]
#[path = "localbin_tests.rs"]
mod tests;

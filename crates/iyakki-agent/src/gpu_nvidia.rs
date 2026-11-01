//! Stage 3: the NVIDIA vendor driver for local models (F-GPU-03; docs/GPU-K3S.md).
//!
//! What IYAKKI ships and what it does not:
//!
//! * **In the image (optional, off by default)**: NVIDIA's *open* GPU kernel modules (`dist/buildroot/package/nvidia-open`, dual
//!   MIT / GPL-2.0), built against the image kernel. Version [`NVIDIA_VERSION`].
//! * **Never in an image**: the vendor's user-space libraries (libcuda, NVML, the PTX JIT, the Vulkan ICD ...), `nvidia-smi` and the GSP
//!   firmware, which are under NVIDIA's licence. They must match the modules' version exactly, so there is one constant and one release
//!   file: `NVIDIA-Linux-x86_64-<version>.run`. The installer downloads it **for the person, at their click**, after showing the licence
//!   notice ([`licence_notice`]) and the size, checks the SHA-256 against the value pinned in this file *and* against the checksum NVIDIA
//!   publishes next to it, runs `sh <file> --extract-only`, and keeps only the files a container needs ([`stage`]) under
//!   `<data>/gpu/nvidia/<version>/stage/`. The `.run` and the extracted tree are deleted afterwards.
//! * **CDI**: [`cdi_spec`] turns that folder and the device nodes into a CDI spec (see `gpucdi.rs`): device nodes `/dev/nvidia*`, the
//!   libraries and `nvidia-smi` as read-only mounts under `/usr/local/nvidia` (where NVIDIA's CUDA images already look), the Vulkan ICD,
//!   and `LD_LIBRARY_PATH`. No glibc hook runs on the musl host.
//! * **Device plugin**: the NVIDIA k8s device plugin (Apache-2.0), pulled by the cluster at the person's click, advertises
//!   `nvidia.com/gpu` and answers allocations with CDI names ([`plugin_manifest`]).
//!
//! The file system root and the data folder are parameters, so the tests run on a folder of fakes.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The driver version: the open modules' release tag AND the vendor `.run` file. `dist/buildroot/package/nvidia-open/nvidia-open.mk`
/// must carry the same text (the test `nvidia_version_pins_agree` reads it).
pub const NVIDIA_VERSION: &str = "595.99.02";
/// SHA-256 of the `.run` file, from NVIDIA's published `<file>.sha256sum` on the day it was pinned.
pub const RUN_SHA256: &str = "e87477958bf763070549324bd5ad6c948eba6ed210e44005b3eff84940f6e1ec";
/// Size in bytes (Content-Length when pinned): shown before the click, and a sanity limit for the download.
pub const RUN_BYTES: u64 = 423_333_820;
/// The hosts the download may come from (redirects included).
pub const RUN_HOSTS: &[&str] = &["download.nvidia.com", "us.download.nvidia.com"];

pub fn run_file() -> String {
    format!("NVIDIA-Linux-x86_64-{NVIDIA_VERSION}.run")
}
pub fn run_url() -> String {
    format!("https://download.nvidia.com/XFree86/Linux-x86_64/{NVIDIA_VERSION}/{}", run_file())
}
/// NVIDIA's published checksum file for the download (one line: `<sha256>  <file>`).
pub fn run_sha_url() -> String {
    format!("{}.sha256sum", run_url())
}

/// Pinned images. IYAKKI ships none of them: the cluster pulls each at the person's click.
pub const PLUGIN_IMAGE: &str = "nvcr.io/nvidia/k8s-device-plugin:v0.20.1";
pub const CUDA_BASE_IMAGE: &str = "docker.io/nvidia/cuda:13.3.1-base-ubuntu24.04";
pub const LLAMA_CUDA_IMAGE: &str = "ghcr.io/ggml-org/llama.cpp:full-cuda-b11749";
pub const OLLAMA_IMAGE: &str = "docker.io/ollama/ollama:0.40.6";

/// What the person reads before the download starts (the review text of Quick install and the first lines of the job's log).
pub fn licence_notice() -> String {
    format!(
        "NVIDIA GPU driver (CUDA), version {v}\n\
         \n\
         What is installed, and under which licences:\n\
         - Kernel modules: NVIDIA's open GPU kernel modules, dual MIT / GPL-2.0. They are part of this PC image when it was built with them.\n\
         - User-space libraries, nvidia-smi and the GSP firmware: NOT part of IYAKKI. They are NVIDIA's proprietary software under NVIDIA's\n  \
         licence for the driver (https://www.nvidia.com/en-us/drivers/nvidia-license/). IYAKKI downloads them from NVIDIA for you now, when you\n  \
         press Install: {file}, {mb} MB, from download.nvidia.com, checked against the checksum NVIDIA publishes and the one pinned in IYAKKI.\n  \
         By pressing Install you accept NVIDIA's licence for these files. They stay in the data folder, are never part of an image, and are\n  \
         shared with containers on this machine only (read-only mounts).\n\
         - NVIDIA device plugin (Apache-2.0) and the CUDA container images: pulled by the cluster from NVIDIA's and Docker's registries.\n\
         - CDI, the Container Device Interface that hands the GPU to containers, is an open specification.\n",
        v = NVIDIA_VERSION,
        file = run_file(),
        mb = RUN_BYTES >> 20
    )
}

// ------------------------------------------------------------------------------------------------ folders

/// `<data>/gpu/nvidia/<version>`.
#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(data: &Path) -> Layout {
        Layout { root: data.join("gpu/nvidia").join(NVIDIA_VERSION) }
    }
    pub fn run(&self) -> PathBuf {
        self.root.join(run_file())
    }
    pub fn extracted(&self) -> PathBuf {
        self.root.join("extracted")
    }
    pub fn stage(&self) -> PathBuf {
        self.root.join("stage")
    }
    pub fn lib(&self) -> PathBuf {
        self.stage().join("lib64")
    }
    pub fn bin(&self) -> PathBuf {
        self.stage().join("bin")
    }
    pub fn firmware(&self) -> PathBuf {
        self.stage().join("firmware")
    }
    pub fn vulkan(&self) -> PathBuf {
        self.stage().join("vulkan")
    }
    pub fn done_marker(&self) -> PathBuf {
        self.stage().join("STAGED")
    }
    /// The stage is complete: the marker is there and the main library and the tool are in it.
    pub fn staged(&self) -> bool {
        self.done_marker().is_file() && self.bin().join("nvidia-smi").is_file() && std::fs::read_dir(self.lib()).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with(&format!("libcuda.so.{NVIDIA_VERSION}")))
    }
}

// ------------------------------------------------------------------------------------------------ download and checks

/// SHA-256 of a file as lower-case hex.
pub fn sha256_file(p: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The hash in a published checksum file (`<64 hex>  <file name>`), for `file`.
pub fn published_sha(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        let (h, f) = (w.next()?, w.next()?);
        (f.trim_start_matches('*') == file && h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
    })
}

/// A download may only come from NVIDIA's hosts.
pub fn host_allowed(url: &str) -> bool {
    let rest = url.strip_prefix("https://").unwrap_or("");
    let host = rest.split('/').next().unwrap_or("").split(':').next().unwrap_or("");
    RUN_HOSTS.contains(&host)
}

/// Follow redirects to NVIDIA's hosts only; returns the open answer.
pub fn open_run(st: &dyn iyakki_proxy::Streamer, url: &str) -> Result<iyakki_proxy::StreamHead, String> {
    let mut url = url.to_string();
    for _ in 0..5 {
        if !host_allowed(&url) {
            return Err(format!("{url} is not on NVIDIA's download hosts; refused"));
        }
        let head = st.open(&url, &[]).map_err(|e| format!("could not reach {url}: {e}"))?;
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

/// Download the `.run` into `dest`, hashing as it arrives, calling `progress(done, total)` about every 5 percent; the hash must equal
/// [`RUN_SHA256`] and, when `published` is given, NVIDIA's published value too. A wrong file is deleted.
pub fn download_run(st: &dyn iyakki_proxy::Streamer, dest: &Path, published: Option<&str>, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    download_to(st, &run_url(), dest, RUN_SHA256, RUN_BYTES, published, progress)
}

/// [`download_run`] with the address, the pinned hash and the expected size as parameters (the tests use small fakes).
pub fn download_to(st: &dyn iyakki_proxy::Streamer, url: &str, dest: &Path, pinned: &str, expect_bytes: u64, published: Option<&str>, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    use std::io::{Read, Write};
    let head = open_run(st, url)?;
    let total = head.header("content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(expect_bytes);
    let slack = expect_bytes / 6 + 1;
    if total > expect_bytes + slack {
        return Err(format!("the download is {} MB, far more than the {} MB expected; refused", total >> 20, expect_bytes >> 20));
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let part = dest.with_extension("part");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?);
    let (mut h, mut body, mut got, mut last) = (Sha256::new(), head.body, 0u64, 0u64);
    let mut buf = vec![0u8; 256 * 1024];
    let r = (|| -> Result<(), String> {
        loop {
            let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > expect_bytes + slack {
                return Err("the download is larger than expected; stopped".into());
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if got >= last + total / 20 + 1 {
                last = got;
                progress(got, total);
            }
        }
        out.flush().map_err(|e| e.to_string())
    })();
    drop(out);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if total > 0 && got != total {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the download stopped at {} of {} MB", got >> 20, total >> 20));
    }
    let have: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if have != pinned {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the checksum of the download ({have}) is not the one pinned in IYAKKI ({pinned}); the file was deleted"));
    }
    if let Some(p) = published {
        if p != have {
            let _ = std::fs::remove_file(&part);
            return Err(format!("NVIDIA's published checksum ({p}) differs from the download ({have}); the file was deleted"));
        }
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    progress(got, total);
    Ok(())
}

// ------------------------------------------------------------------------------------------------ ELF soname

/// `DT_SONAME` of a 64-bit little-endian ELF shared object, or `None`.
pub fn elf_soname(b: &[u8]) -> Option<String> {
    let u16at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u64);
    let u32at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64);
    let u64at = |o: usize| b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()));
    if b.get(0..4)? != b"\x7fELF" || *b.get(4)? != 2 || *b.get(5)? != 1 {
        return None;
    }
    let (phoff, phentsize, phnum) = (u64at(0x20)? as usize, u16at(0x36)? as usize, u16at(0x38)? as usize);
    let mut loads: Vec<(u64, u64, u64)> = Vec::new(); // vaddr, offset, filesz
    let mut dynamic: Option<(u64, u64)> = None; // offset, size
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let (ty, off, vaddr, filesz) = (u32at(o)?, u64at(o + 8)?, u64at(o + 16)?, u64at(o + 32)?);
        match ty {
            1 => loads.push((vaddr, off, filesz)),
            2 => dynamic = Some((off, filesz)),
            _ => {}
        }
    }
    let (doff, dsize) = dynamic?;
    let to_off = |va: u64| loads.iter().find(|(v, _, s)| va >= *v && va < v + s).map(|(v, o, _)| (va - v + o) as usize);
    let (mut strtab, mut soname) = (None, None);
    for i in 0..(dsize as usize / 16) {
        let (tag, val) = (u64at(doff as usize + i * 16)?, u64at(doff as usize + i * 16 + 8)?);
        match tag {
            0 => break,
            5 => strtab = Some(val),
            14 => soname = Some(val),
            _ => {}
        }
    }
    let s = to_off(strtab?)? + soname? as usize;
    let end = b.get(s..)?.iter().position(|c| *c == 0)?;
    String::from_utf8(b[s..s + end].to_vec()).ok()
}

// ------------------------------------------------------------------------------------------------ staging

/// Manifest types of the 64-bit libraries kept (compute, NVML, the PTX compiler, video, OpenCL, the Vulkan ICD's library and what it loads).
pub const KEEP_LIB_TYPES: &[&str] = &["CUDA_LIB", "UTILITY_LIB", "NVCUVID_LIB", "ENCODEAPI_LIB", "OPENCL_LIB", "OPENGL_LIB", "TLS_LIB"];
/// Library files NOT kept although their type matches: windowing-system glue, the settings tool's toolkits, OpenCL, ray tracing, video decoding
/// and the CUDA debugger: none is needed to run a language model, and together they are about 250 MB.
pub const DROP_LIB_PREFIXES: &[&str] = &["libnvidia-egl", "libEGL_nvidia", "libGLES", "libnvidia-wayland", "libnvidia-ngx", "libnvidia-present", "libnvidia-vksc", "libnvidia-gtk", "libnvidia-opencl", "libnvidia-rtcore", "libnvoptix", "libnvcuvid", "libnvidia-opticalflow", "libcudadebugger"];
/// Library files kept by name when the package has no `.manifest` (an older or different layout): versioned files with these prefixes.
pub const KEEP_LIB_PREFIXES: &[&str] = &["libcuda", "libnvidia-", "libnvcuvid", "libGLX_nvidia", "libnvoptix"];
/// Programs kept (CUDA multi-process service included).
pub const KEEP_BINS: &[&str] = &["nvidia-smi", "nvidia-cuda-mps-control", "nvidia-cuda-mps-server", "nvidia-debugdump"];
/// The GSP firmware files the open modules load (`nvidia/<version>/gsp_*.bin`).
pub const KEEP_FIRMWARE_PREFIX: &str = "gsp_";

/// What [`stage`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Staged {
    pub libs: usize,
    pub links: usize,
    pub bins: usize,
    pub firmware: usize,
    pub icd: bool,
    pub bytes: u64,
}

/// One line of the vendor package's `.manifest`: `<name> <mode> <TYPE> ... [<symlink target>] MODULE:<m>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub name: String,
    pub ty: String,
    /// For `*_SYMLINK` entries: what the link points to.
    pub target: Option<String>,
}

/// The entries of a `.manifest` text (the header lines before the first entry have no type and are skipped).
pub fn parse_manifest(text: &str) -> Vec<ManifestEntry> {
    text.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() < 3 || !w[1].chars().all(|c| c.is_ascii_digit()) || !w[2].chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                return None;
            }
            let target = if w[2].ends_with("_SYMLINK") {
                w.iter().position(|x| x.starts_with("MODULE:")).filter(|i| *i >= 4).map(|i| w[i - 1].to_string()).filter(|t| t != "/" && t != "NATIVE")
            } else {
                None
            };
            Some(ManifestEntry { name: w[0].to_string(), ty: w[2].to_string(), target })
        })
        .collect()
}

/// The 64-bit library files to keep and the links to make, from a manifest: `(files, links as (name, target))`. A link is kept when it
/// leads, through other links, to a kept file.
pub fn wanted_from_manifest(entries: &[ManifestEntry]) -> (Vec<String>, Vec<(String, String)>) {
    let plain = |n: &str| !n.contains('/');
    let mut files: Vec<String> = entries
        .iter()
        .filter(|e| KEEP_LIB_TYPES.contains(&e.ty.as_str()) && plain(&e.name) && !DROP_LIB_PREFIXES.iter().any(|p| e.name.starts_with(p)))
        .map(|e| e.name.clone())
        .collect();
    files.sort();
    files.dedup();
    let mut links: Vec<(String, String)> = Vec::new();
    for e in entries.iter().filter(|e| e.ty.ends_with("_SYMLINK") && plain(&e.name)) {
        let Some(t) = &e.target else { continue };
        // follow the chain: libcuda.so -> libcuda.so.1 -> libcuda.so.<version>
        let (mut cur, mut hops) = (t.clone(), 0);
        while hops < 6 {
            if files.contains(&cur) {
                links.push((e.name.clone(), t.clone()));
                break;
            }
            match entries.iter().find(|x| x.ty.ends_with("_SYMLINK") && x.name == cur).and_then(|x| x.target.clone()) {
                Some(next) => cur = next,
                None => break,
            }
            hops += 1;
        }
    }
    links.sort();
    links.dedup();
    (files, links)
}

/// Unpack a NVIDIA `.run` (a makeself shell archive: a script header of `skip=N` lines, then a zstd-compressed tar) into `dest` without running
/// anything. NVIDIA's own `--extract-only` needs a `zstd` program: it uses the host's, else one it carries inside the file, and that one is
/// built for glibc, so on a musl system ("not found") the unpack fails before a byte is written. Returns the number of files.
///
/// `skip=N` counts lines of the header: the payload starts at line N (`tail -n +N`), that is after N-1 newlines. Everything is read straight
/// through, never more than [`crate::archive::MAX_TOTAL`] bytes of files, and nothing is written outside `dest`.
pub fn extract_makeself_zstd(run: &Path, dest: &Path) -> Result<usize, String> {
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
    let f = std::fs::File::open(run).map_err(|e| format!("{}: {e}", run.display()))?;
    let mut r = BufReader::new(f);
    // the header: the first lines say where the payload starts
    let mut skip: Option<u64> = None;
    let mut line = Vec::new();
    for _ in 0..60 {
        line.clear();
        if r.read_until(b'\n', &mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        if let Some(v) = line.strip_prefix(b"skip=") {
            skip = String::from_utf8_lossy(v).trim().parse().ok();
            break;
        }
    }
    let skip = skip.filter(|n| *n >= 2).ok_or("no `skip=` line in the first lines: not a makeself archive")?;
    // walk to the start of line `skip`: skip-1 newlines from the beginning of the file (the header holds binary data, so count bytes, not lines of text)
    r.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let (mut offset, mut newlines) = (0u64, 0u64);
    let mut buf = vec![0u8; 1 << 16];
    'find: loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the header ends before the payload".into());
        }
        for (i, b) in buf[..n].iter().enumerate() {
            if *b == b'\n' {
                newlines += 1;
                if newlines == skip - 1 {
                    offset += i as u64 + 1;
                    break 'find;
                }
            }
        }
        offset += n as u64;
        if offset > 64 << 20 {
            return Err("the header is far longer than a makeself header".into());
        }
    }
    r.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if magic != [0x28, 0xB5, 0x2F, 0xFD] {
        return Err("the payload is not zstd-compressed (another makeself compression)".into());
    }
    r.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let dec = ruzstd::decoding::StreamingDecoder::new(r).map_err(|e| format!("the payload is not a zstd stream ({e})"))?;
    crate::archive::untar_stream(dec, dest, crate::archive::MAX_TOTAL).map(|u| u.files)
}

/// Copy what containers need from an extracted vendor package (`extracted`, the output of `--extract-only`) to `stage`'s folders.
/// The package must be the pinned version: `libcuda.so.<version>` has to be in it. The package's `.manifest` says which files are
/// libraries and which names are links to them ([`wanted_from_manifest`]); without one the versioned `lib*.so.<version>` files are taken
/// and their links come from each file's `DT_SONAME`.
pub fn stage(extracted: &Path, l: &Layout) -> Result<Staged, String> {
    let v = NVIDIA_VERSION;
    if !extracted.join(format!("libcuda.so.{v}")).is_file() || !extracted.join("nvidia-smi").is_file() {
        return Err(format!("the extracted package has no libcuda.so.{v} or no nvidia-smi: not the pinned version {v}, or the extraction failed"));
    }
    let mut s = Staged::default();
    let _ = std::fs::remove_dir_all(l.stage());
    for d in [l.lib(), l.bin(), l.firmware().join("nvidia").join(v), l.vulkan()] {
        std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let manifest = std::fs::read_to_string(extracted.join(".manifest")).ok().map(|t| parse_manifest(&t)).filter(|m| !m.is_empty());
    let mut names: Vec<String> = std::fs::read_dir(extracted).map_err(|e| e.to_string())?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let copy_lib = |n: &str, s: &mut Staged| -> Result<(), String> {
        let dst = l.lib().join(n);
        std::fs::copy(extracted.join(n), &dst).map_err(|e| format!("{n}: {e}"))?;
        s.bytes += std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
        s.libs += 1;
        Ok(())
    };
    match &manifest {
        Some(m) => {
            let (files, links) = wanted_from_manifest(m);
            for f in files.iter().filter(|f| extracted.join(f).is_file()) {
                copy_lib(f, &mut s)?;
            }
            for (name, target) in links.iter().filter(|(_, t)| l.lib().join(t).exists() || links.iter().any(|(n, _)| n == t)) {
                let link = l.lib().join(name);
                if !link.exists() && std::fs::symlink_metadata(&link).is_err() {
                    std::os::unix::fs::symlink(target, &link).map_err(|e| format!("{name}: {e}"))?;
                    s.links += 1;
                }
            }
        }
        None => {
            let suffix = format!(".so.{v}");
            for n in &names {
                if extracted.join(n).is_file() && n.ends_with(&suffix) && KEEP_LIB_PREFIXES.iter().any(|p| n.starts_with(p)) && !DROP_LIB_PREFIXES.iter().any(|p| n.starts_with(p)) {
                    copy_lib(n, &mut s)?;
                    // the soname link the dynamic loader looks for (libcuda.so.1 -> libcuda.so.<version>)
                    let head = read_head(&extracted.join(n), 1 << 20);
                    if let Some(so) = elf_soname(&head).filter(|so| so != n && !so.contains('/')) {
                        let link = l.lib().join(&so);
                        if !link.exists() {
                            std::os::unix::fs::symlink(n, &link).map_err(|e| format!("{so}: {e}"))?;
                            s.links += 1;
                        }
                    }
                }
            }
        }
    }
    for n in &names {
        let src = extracted.join(n);
        if KEEP_BINS.contains(&n.as_str()) && src.is_file() {
            let dst = l.bin().join(n);
            std::fs::copy(&src, &dst).map_err(|e| format!("{n}: {e}"))?;
            set_mode(&dst, 0o755);
            s.bins += 1;
        } else if n == "nvidia_icd.json" && src.is_file() {
            std::fs::copy(&src, l.vulkan().join(n)).map_err(|e| format!("{n}: {e}"))?;
            s.icd = true;
        }
    }
    for e in std::fs::read_dir(extracted.join("firmware")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with(KEEP_FIRMWARE_PREFIX) && n.ends_with(".bin") {
            let dst = l.firmware().join("nvidia").join(v).join(&n);
            std::fs::copy(e.path(), &dst).map_err(|e| format!("{n}: {e}"))?;
            s.bytes += std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
            s.firmware += 1;
        }
    }
    if s.libs == 0 || s.bins == 0 {
        return Err("no libraries or no programs were found in the package".into());
    }
    std::fs::write(l.done_marker(), format!("{v}\n")).map_err(|e| e.to_string())?;
    Ok(s)
}

fn read_head(p: &Path, max: usize) -> Vec<u8> {
    use std::io::Read;
    let mut v = Vec::new();
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.take(max as u64).read_to_end(&mut v);
    }
    v
}

fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

/// Delete the download and the extracted tree (the stage stays).
pub fn clean_download(l: &Layout) {
    let _ = std::fs::remove_file(l.run());
    let _ = std::fs::remove_file(l.run().with_extension("part"));
    let _ = std::fs::remove_dir_all(l.extracted());
}

// ------------------------------------------------------------------------------------------------ device nodes

/// NVIDIA's character device major.
pub const NVIDIA_MAJOR: u32 = 195;

/// A device node to make: `mknod path c major minor`, mode 0666.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeToMake {
    pub path: String,
    pub major: u32,
    pub minor: u32,
}

/// The nodes `nvidia-modprobe` would make: `/dev/nvidiactl` (195, 255), `/dev/nvidia<minor>` (195, minor) for each GPU and the UVM nodes
/// (the major of `nvidia-uvm` from `/proc/devices`; minor 0 and 1). `gpu_minors` come from `/proc/driver/nvidia/gpus/*/information`
/// (`Device Minor: N`); `proc_devices` is the text of `/proc/devices`.
pub fn nodes_to_make(gpu_minors: &[u32], proc_devices: &str) -> Vec<NodeToMake> {
    let mut v = vec![NodeToMake { path: "/dev/nvidiactl".into(), major: NVIDIA_MAJOR, minor: 255 }];
    for m in gpu_minors {
        v.push(NodeToMake { path: format!("/dev/nvidia{m}"), major: NVIDIA_MAJOR, minor: *m });
    }
    if let Some(uvm) = device_major(proc_devices, "nvidia-uvm") {
        v.push(NodeToMake { path: "/dev/nvidia-uvm".into(), major: uvm, minor: 0 });
        v.push(NodeToMake { path: "/dev/nvidia-uvm-tools".into(), major: uvm, minor: 1 });
    }
    v
}

/// The major number of a character device in `/proc/devices` text.
pub fn device_major(proc_devices: &str, name: &str) -> Option<u32> {
    let mut chars = false;
    for l in proc_devices.lines() {
        if l.starts_with("Character devices") {
            chars = true;
            continue;
        }
        if l.starts_with("Block devices") {
            chars = false;
        }
        if chars {
            let mut w = l.split_whitespace();
            if let (Some(n), Some(d)) = (w.next(), w.next()) {
                if d == name {
                    return n.parse().ok();
                }
            }
        }
    }
    None
}

/// `Device Minor: N` of one `/proc/driver/nvidia/gpus/<pci>/information`.
pub fn parse_minor(info: &str) -> Option<u32> {
    info.lines().find_map(|l| l.strip_prefix("Device Minor:").and_then(|v| v.trim().parse().ok()))
}

/// `GPU UUID: GPU-xxxx` of the same file.
pub fn parse_uuid(info: &str) -> Option<String> {
    info.lines().find_map(|l| l.strip_prefix("GPU UUID:").map(|v| v.trim().to_string()))
}

/// The minors and UUIDs of the GPUs the loaded driver found, sorted by minor: reads `<root>/proc/driver/nvidia/gpus/*/information`.
pub fn driver_gpus(root: &Path) -> Vec<(u32, Option<String>)> {
    let mut v: Vec<(u32, Option<String>)> = Vec::new();
    for e in std::fs::read_dir(root.join("proc/driver/nvidia/gpus")).into_iter().flatten().flatten() {
        if let Ok(t) = std::fs::read_to_string(e.path().join("information")) {
            if let Some(m) = parse_minor(&t) {
                v.push((m, parse_uuid(&t)));
            }
        }
    }
    v.sort();
    v
}

/// The version of the loaded driver (`/proc/driver/nvidia/version`: `NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  595.99.02 ...`).
pub fn loaded_version(root: &Path) -> Option<String> {
    let t = std::fs::read_to_string(root.join("proc/driver/nvidia/version")).ok()?;
    t.split_whitespace().find(|w| w.len() >= 5 && w.chars().all(|c| c.is_ascii_digit() || c == '.') && w.matches('.').count() >= 1 && w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.split('.').next().is_some_and(|m| m.len() == 3)).map(str::to_string)
}

// ------------------------------------------------------------------------------------------------ the CDI spec

/// Where the stage appears inside containers (the path NVIDIA's CUDA images already search).
pub const CONTAINER_ROOT: &str = "/usr/local/nvidia";

/// Module parameters for `nvidia`: fine-grained dynamic power management. The card powers itself down when nothing uses it and wakes when a
/// program asks (about a second), instead of idling at 20 W or more on a laptop. Needs the PCI function's runtime power control on `auto`
/// ([`allow_power_saving`]).
pub const MODULE_PARAMS: [&str; 1] = ["NVreg_DynamicPowerManagement=0x02"];

/// The file that says the person switched the NVIDIA card off to save power: while it exists IYAKKI does not load the driver at start.
pub fn sleep_marker(data: &Path) -> PathBuf {
    data.join("gpu").join("nvidia-sleep")
}

/// The `modprobe` command for one module: `nvidia` gets [`MODULE_PARAMS`].
pub fn modprobe_argv(module: &str) -> Vec<String> {
    let mut v = vec!["modprobe".to_string(), module.to_string()];
    if module == "nvidia" {
        v.extend(MODULE_PARAMS.iter().map(|p| p.to_string()));
    }
    v
}

/// Let the kernel power the NVIDIA cards down when idle: `power/control` = `auto` on every NVIDIA display function and on the other functions
/// of the same slot (the HDMI audio at `.1`, the USB-C controller). Returns what was changed, as `0000:01:00.0` addresses.
pub fn allow_power_saving(root: &Path) -> Vec<String> {
    let dir = root.join("sys/bus/pci/devices");
    let mut changed = Vec::new();
    for g in crate::gpu::detect(root).iter().filter(|g| g.is_nvidia()) {
        let slot = g.pci.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| g.pci.clone());
        let mut names: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&format!("{slot}."))).collect();
        names.sort();
        for n in names {
            let ctl = dir.join(&n).join("power/control");
            if std::fs::read_to_string(&ctl).map(|t| t.trim() != "auto").unwrap_or(false) && std::fs::write(&ctl, "auto\n").is_ok() {
                changed.push(n);
            }
        }
    }
    changed
}

/// Build the CDI spec of the machine for `kind` (use [`crate::gpucdi::NVIDIA_KIND`] and [`crate::gpucdi::PLUGIN_KIND`]).
/// * `root`: the file system root (`/`), where `dev/nvidia*` are looked for;
/// * `l`: the staged vendor files;
/// * `gpus`: `(minor, uuid)` of the GPUs the driver found.
///
/// Devices: one per GPU named by its index (position in `gpus`), one by UUID when known, and `all`. Common edits: the control and UVM
/// nodes, the library and tool mounts (each versioned file at its own name and at every soname link that points to it), the Vulkan ICD
/// and `LD_LIBRARY_PATH`.
pub fn cdi_spec(kind: &str, root: &Path, l: &Layout, gpus: &[(u32, Option<String>)]) -> Result<crate::gpucdi::Spec, String> {
    use crate::gpucdi::{Device, DeviceNode, Edits, Mount, Spec};
    if gpus.is_empty() {
        return Err("the driver reports no GPU (nothing under /proc/driver/nvidia/gpus)".into());
    }
    let present = |p: &str| root.join(p.trim_start_matches('/')).exists();
    let mut common = Edits::default();
    for n in ["/dev/nvidiactl", "/dev/nvidia-uvm", "/dev/nvidia-uvm-tools", "/dev/nvidia-modeset"] {
        if present(n) {
            common.device_nodes.push(DeviceNode::open(n));
        }
    }
    if !present("/dev/nvidiactl") {
        return Err("/dev/nvidiactl does not exist: the driver is not loaded or the device nodes were not made".into());
    }
    // libraries: every regular file with its soname links
    let mut files: Vec<String> = Vec::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for e in std::fs::read_dir(l.lib()).map_err(|e| format!("{}: {e}", l.lib().display()))?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        match std::fs::symlink_metadata(e.path()) {
            Ok(m) if m.file_type().is_symlink() => {
                if let Ok(t) = std::fs::read_link(e.path()) {
                    links.push((n, t.to_string_lossy().into_owned()));
                }
            }
            Ok(m) if m.is_file() => files.push(n),
            _ => {}
        }
    }
    files.sort();
    links.sort();
    for f in &files {
        let host = l.lib().join(f).display().to_string();
        common.mounts.push(Mount::ro(&host, &format!("{CONTAINER_ROOT}/lib64/{f}")));
        for (ln, target) in &links {
            if target == f {
                common.mounts.push(Mount::ro(&host, &format!("{CONTAINER_ROOT}/lib64/{ln}")));
            }
        }
    }
    let mut bins: Vec<String> = std::fs::read_dir(l.bin()).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    bins.sort();
    for b in &bins {
        let host = l.bin().join(b).display().to_string();
        common.mounts.push(Mount::ro(&host, &format!("{CONTAINER_ROOT}/bin/{b}")));
        if b == "nvidia-smi" {
            common.mounts.push(Mount::ro(&host, "/usr/bin/nvidia-smi"));
        }
    }
    let icd = l.vulkan().join("nvidia_icd.json");
    if icd.is_file() {
        common.mounts.push(Mount::ro(&icd.display().to_string(), "/etc/vulkan/icd.d/nvidia_icd.json"));
    }
    common.env.push(format!("LD_LIBRARY_PATH={CONTAINER_ROOT}/lib64"));
    let node = |m: u32| DeviceNode::open(&format!("/dev/nvidia{m}"));
    let mut devices: Vec<Device> = Vec::new();
    for (i, (minor, uuid)) in gpus.iter().enumerate() {
        devices.push(Device { name: i.to_string(), edits: Edits { device_nodes: vec![node(*minor)], ..Default::default() } });
        if let Some(u) = uuid {
            devices.push(Device { name: u.clone(), edits: Edits { device_nodes: vec![node(*minor)], ..Default::default() } });
        }
    }
    devices.push(Device { name: "all".into(), edits: Edits { device_nodes: gpus.iter().map(|(m, _)| node(*m)).collect(), ..Default::default() } });
    let spec = Spec { kind: kind.into(), devices, common };
    spec.validate()?;
    Ok(spec)
}

// ------------------------------------------------------------------------------------------------ the facts and the state machine

/// What is known about the machine, in the order the installer needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub gpu_present: bool,
    /// Turing or newer (the open modules and the GSP firmware path support it).
    pub open_supported: bool,
    /// `nvidia.ko` is in /lib/modules (the PC image was built with the open modules).
    pub module_present: bool,
    pub nouveau_loaded: bool,
    pub nvidia_loaded: bool,
    /// The loaded driver is the pinned version.
    pub version_matches: bool,
    /// The vendor files are downloaded, extracted and staged.
    pub vendor_files: bool,
    pub cdi_written: bool,
    /// k3s's containerd reads CDI specs.
    pub cdi_enabled: bool,
    pub plugin_ready: bool,
    /// The node reports `nvidia.com/gpu` capacity.
    pub node_reports: bool,
}

/// Where the installation stands: the first thing that is missing, or `Ready`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    NoGpu,
    UnsupportedGpu,
    NoModule,
    NouveauInTheWay,
    NeedsDownload,
    NeedsModuleLoad,
    NeedsCdi,
    NeedsContainerdCdi,
    NeedsPlugin,
    WaitingForNode,
    Ready,
}

pub fn next_stage(f: &Facts) -> Stage {
    if !f.gpu_present {
        Stage::NoGpu
    } else if !f.open_supported {
        Stage::UnsupportedGpu
    } else if !f.module_present {
        Stage::NoModule
    } else if f.nouveau_loaded && !f.nvidia_loaded {
        Stage::NouveauInTheWay
    } else if !f.vendor_files {
        Stage::NeedsDownload
    } else if !f.nvidia_loaded || !f.version_matches {
        Stage::NeedsModuleLoad
    } else if !f.cdi_written {
        Stage::NeedsCdi
    } else if !f.cdi_enabled {
        Stage::NeedsContainerdCdi
    } else if !f.plugin_ready {
        Stage::NeedsPlugin
    } else if !f.node_reports {
        Stage::WaitingForNode
    } else {
        Stage::Ready
    }
}

impl Stage {
    /// A plain sentence for the page and the log.
    pub fn words(self) -> &'static str {
        match self {
            Stage::NoGpu => "no NVIDIA GPU found",
            Stage::UnsupportedGpu => "this NVIDIA GPU is older than Turing (GTX 16 series and RTX 20 series are the oldest): NVIDIA's open kernel modules do not support it",
            Stage::NoModule => "this image has no NVIDIA kernel modules: use the PC image built with the open modules (docs/GPU-K3S.md)",
            Stage::NouveauInTheWay => "the nouveau driver is using the card: start with the kernel option modprobe.blacklist=nouveau (docs/GPU-K3S.md, step 9)",
            Stage::NeedsDownload => "the vendor files are not downloaded yet",
            Stage::NeedsModuleLoad => "the NVIDIA modules are not loaded (or are another version)",
            Stage::NeedsCdi => "the CDI spec is not written",
            Stage::NeedsContainerdCdi => "the cluster's containerd does not read CDI specs yet (it restarts once)",
            Stage::NeedsPlugin => "the NVIDIA device plugin is not running",
            Stage::WaitingForNode => "waiting for the node to report nvidia.com/gpu",
            Stage::Ready => "ready: pods can request nvidia.com/gpu",
        }
    }
    /// Stops the installation before it downloads or changes anything.
    pub fn refuses(self) -> bool {
        matches!(self, Stage::NoGpu | Stage::UnsupportedGpu | Stage::NoModule | Stage::NouveauInTheWay)
    }
}

/// Read the machine part of [`Facts`] from files under `root` and `data` (the cluster part is filled by the caller).
pub fn machine_facts(root: &Path, data: &Path, gpus: &[crate::gpu::Gpu]) -> Facts {
    let nv: Vec<&crate::gpu::Gpu> = gpus.iter().filter(|g| g.is_nvidia()).collect();
    let l = Layout::new(data);
    let loaded = crate::gpu::module_loaded(root, "nvidia");
    Facts {
        gpu_present: !nv.is_empty(),
        open_supported: !nv.is_empty() && nv.iter().all(|g| crate::gpu::nvidia_open_supported(g.device_id)),
        module_present: crate::gpu::nvidia_module_path(root).is_some(),
        nouveau_loaded: crate::gpu::module_loaded(root, "nouveau"),
        nvidia_loaded: loaded,
        version_matches: loaded && loaded_version(root).as_deref() == Some(NVIDIA_VERSION),
        vendor_files: l.staged(),
        cdi_written: root.join(crate::gpucdi::SPEC_DIR.trim_start_matches('/')).join(format!("{}.json", crate::gpucdi::NVIDIA_KIND.replace('/', "-"))).is_file(),
        ..Default::default()
    }
}

// ------------------------------------------------------------------------------------------------ cluster objects

/// The NVIDIA device plugin as a DaemonSet in `kube-system`, with CDI annotations and no hooks: it advertises `nvidia.com/gpu` and answers
/// each allocation with the CDI name `k8s.device-plugin.nvidia.com/gpu=<index>`, which containerd resolves in the spec IYAKKI wrote.
/// `kubelet_dir` is the kubelet root of the managed cluster (`<k3s data>/agent/kubelet`).
///
/// The plugin looks for NVML itself: the stage is mounted into the pod at [`CONTAINER_ROOT`], which is also its driver root (it looks for
/// `libcuda`, `libnvidia-ml` and the other driver libraries under `<driver root>/lib64`; a driver root that does not exist makes it stop with
/// "failed to locate libcuda.so"), and `LD_LIBRARY_PATH` points to it. The plugin's own CDI spec goes to
/// a throw-away directory (IYAKKI's spec on the host is the one containerd reads).
pub fn plugin_manifest(kubelet_dir: &str, stage_dir: &str) -> String {
    format!(
        "apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: nvidia-device-plugin
  namespace: kube-system
  labels:
    app.kubernetes.io/name: nvidia-device-plugin
    app.kubernetes.io/managed-by: iyakki
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: nvidia-device-plugin
  updateStrategy:
    type: RollingUpdate
  template:
    metadata:
      labels:
        app.kubernetes.io/name: nvidia-device-plugin
        app.kubernetes.io/managed-by: iyakki
    spec:
      priorityClassName: system-node-critical
      tolerations:
        - key: nvidia.com/gpu
          operator: Exists
          effect: NoSchedule
      containers:
        - name: nvidia-device-plugin
          image: {img}
          env:
            - name: DEVICE_LIST_STRATEGY
              value: cdi-annotations
            - name: DEVICE_ID_STRATEGY
              value: index
            - name: CDI_DISABLE_HOOKS
              value: all
            - name: NVIDIA_DRIVER_ROOT
              value: {croot}
            - name: CONTAINER_DRIVER_ROOT
              value: {croot}
            - name: LD_LIBRARY_PATH
              value: {croot}/lib64
            - name: FAIL_ON_INIT_ERROR
              value: \"false\"
          securityContext:
            privileged: true
          volumeMounts:
            - name: kubelet-device-plugins-dir
              mountPath: /var/lib/kubelet/device-plugins
            - name: nvidia-stage
              mountPath: {croot}
              readOnly: true
            - name: dev
              mountPath: /dev
            - name: cdi
              mountPath: /var/run/cdi
      volumes:
        - name: kubelet-device-plugins-dir
          hostPath:
            path: {kubelet}/device-plugins
            type: Directory
        - name: nvidia-stage
          hostPath:
            path: {stage}
            type: Directory
        - name: dev
          hostPath:
            path: /dev
        - name: cdi
          emptyDir: {{}}
",
        img = PLUGIN_IMAGE,
        croot = CONTAINER_ROOT,
        kubelet = kubelet_dir.trim_end_matches('/'),
        stage = stage_dir.trim_end_matches('/'),
    )
}

#[cfg(test)]
#[path = "gpu_nvidia_tests.rs"]
pub(crate) mod tests;

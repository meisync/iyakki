use super::*;
use crate::gpucdi::tests::{golden, tmp};
use std::sync::Mutex;

// ---- the pins

fn repo_file(rel: &str) -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn nvidia_version_pins_agree() {
    // the Buildroot package of the open kernel modules, its hash file, the fragment and this file name one version
    let mk = repo_file("dist/buildroot/package/nvidia-open/nvidia-open.mk");
    let v = mk.lines().find_map(|l| l.strip_prefix("NVIDIA_OPEN_VERSION = ")).expect("NVIDIA_OPEN_VERSION in nvidia-open.mk").trim();
    assert_eq!(v, NVIDIA_VERSION, "nvidia-open.mk and gpu_nvidia.rs must name the same driver version");
    let hash = repo_file("dist/buildroot/package/nvidia-open/nvidia-open.hash");
    let line = hash.lines().find(|l| l.contains(&format!("nvidia-open-{NVIDIA_VERSION}.tar.gz"))).expect("the hash file lists the pinned tarball");
    let sha = line.split_whitespace().nth(1).unwrap();
    assert!(sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()), "{line}");
    assert!(hash.contains("COPYING"), "the licence file is hashed too");
    assert!(mk.contains("github,NVIDIA,open-gpu-kernel-modules"), "built from NVIDIA's open modules repository");
    assert!(mk.contains("MIT or GPL-2.0"));
    // the download is that version's file, from NVIDIA's host, with a pinned 64-hex checksum
    assert_eq!(run_file(), format!("NVIDIA-Linux-x86_64-{NVIDIA_VERSION}.run"));
    assert_eq!(run_url(), format!("https://download.nvidia.com/XFree86/Linux-x86_64/{NVIDIA_VERSION}/NVIDIA-Linux-x86_64-{NVIDIA_VERSION}.run"));
    assert_eq!(run_sha_url(), format!("{}.sha256sum", run_url()));
    assert!(RUN_SHA256.len() == 64 && RUN_SHA256.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    assert!(host_allowed(&run_url()));
    // the fragment that switches the package on exists and names the package
    assert!(repo_file("dist/buildroot/configs/iyakki_nvidia_open.fragment").lines().any(|l| l == "BR2_PACKAGE_NVIDIA_OPEN=y"));
    assert!(repo_file("dist/buildroot/package/nvidia-open/Config.in").contains("config BR2_PACKAGE_NVIDIA_OPEN"));
    assert!(repo_file("dist/buildroot/Config.in").contains("package/nvidia-open/Config.in"));
    // off by default: no default config selects it
    for c in ["iyakki_x86_64_defconfig", "iyakki_mac.fragment", "iyakki_firmware.fragment", "iyakki_web.fragment"] {
        assert!(!repo_file(&format!("dist/buildroot/configs/{c}")).contains("BR2_PACKAGE_NVIDIA_OPEN"), "{c} must not select the NVIDIA modules");
    }
}

#[test]
fn the_kernel_fragment_carries_what_the_open_modules_and_nouveau_need() {
    let f = repo_file("dist/buildroot/board/iyakki/linux.fragment");
    for o in ["CONFIG_MODULES=y", "CONFIG_DRM=y", "CONFIG_DRM_NOUVEAU=m", "CONFIG_DRM_NOUVEAU_GSP_DEFAULT=y", "CONFIG_DRM_AMDGPU=m", "CONFIG_DRM_I915=m", "CONFIG_DRM_XE=m", "CONFIG_BLK_DEV_NVME=y", "CONFIG_USB_XHCI_PCI=y", "CONFIG_USB_STORAGE=y", "CONFIG_FB_EFI=y", "CONFIG_HSA_AMD=y", "CONFIG_R8169=m", "CONFIG_IGC=m", "CONFIG_IWLMVM=m", "CONFIG_RTW89=m", "CONFIG_MT7921E=m"] {
        assert!(f.lines().any(|l| l.trim() == o), "linux.fragment lacks {o}");
    }
}

// ---- the download

struct FakeStreamer {
    answers: Mutex<Vec<(u16, Vec<(String, String)>, Vec<u8>)>>,
    urls: Mutex<Vec<String>>,
}

impl iyakki_proxy::Streamer for FakeStreamer {
    fn open(&self, url: &str, _h: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        self.urls.lock().unwrap().push(url.into());
        let (status, headers, body) = self.answers.lock().unwrap().remove(0);
        Ok(iyakki_proxy::StreamHead { status, headers, body: Box::new(std::io::Cursor::new(body)) })
    }
}

fn hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn the_download_is_checked_against_the_pinned_and_the_published_checksum_and_deleted_when_wrong() {
    let d = tmp("dl");
    let body = vec![7u8; 5000];
    let good = hex(&body);
    let ok = |b: &[u8]| FakeStreamer { answers: Mutex::new(vec![(200, vec![("content-length".into(), b.len().to_string())], b.to_vec())]), urls: Mutex::new(Vec::new()) };
    let mut seen = Vec::new();
    download_to(&ok(&body), &run_url(), &d.join("x.run"), &good, 5000, Some(&good), &mut |a, b| seen.push((a, b))).unwrap();
    assert_eq!(std::fs::read(d.join("x.run")).unwrap(), body);
    assert_eq!(seen.last(), Some(&(5000, 5000)));
    assert!(!d.join("x.part").exists());
    // a different file than the pinned one
    let e = download_to(&ok(&body), &run_url(), &d.join("y.run"), &hex(b"other"), 5000, None, &mut |_, _| {}).unwrap_err();
    assert!(e.contains("not the one pinned") && !d.join("y.run").exists() && !d.join("y.part").exists(), "{e}");
    // the pinned value matches but NVIDIA's published one does not: still refused
    let e = download_to(&ok(&body), &run_url(), &d.join("z.run"), &good, 5000, Some(&hex(b"other")), &mut |_, _| {}).unwrap_err();
    assert!(e.contains("published checksum") && !d.join("z.run").exists(), "{e}");
    // far larger than expected
    let big = FakeStreamer { answers: Mutex::new(vec![(200, vec![("content-length".into(), "999999999".into())], vec![])]), urls: Mutex::new(Vec::new()) };
    assert!(download_to(&big, &run_url(), &d.join("b.run"), &good, 5000, None, &mut |_, _| {}).unwrap_err().contains("refused"));
    // a truncated body
    let cut = FakeStreamer { answers: Mutex::new(vec![(200, vec![("content-length".into(), "6000".into())], body.clone())]), urls: Mutex::new(Vec::new()) };
    assert!(download_to(&cut, &run_url(), &d.join("c.run"), &good, 6000, None, &mut |_, _| {}).unwrap_err().contains("stopped at"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn redirects_stay_on_nvidias_hosts() {
    let ok = FakeStreamer { answers: Mutex::new(vec![(302, vec![("location".into(), "https://us.download.nvidia.com/x.run".into())], vec![]), (200, vec![], vec![1])]), urls: Mutex::new(Vec::new()) };
    open_run(&ok, &run_url()).unwrap();
    assert_eq!(ok.urls.lock().unwrap().as_slice(), [run_url(), "https://us.download.nvidia.com/x.run".to_string()]);
    let evil = FakeStreamer { answers: Mutex::new(vec![(302, vec![("location".into(), "https://evil.example.com/x.run".into())], vec![])]), urls: Mutex::new(Vec::new()) };
    assert!(open_run(&evil, &run_url()).err().unwrap().contains("not on NVIDIA's download hosts"));
    assert!(open_run(&evil, "http://download.nvidia.com/x").is_err(), "plain http is refused");
    assert!(!host_allowed("https://download.nvidia.com.evil.example/x") && !host_allowed("https://evil.example/download.nvidia.com/x"));
    let nf = FakeStreamer { answers: Mutex::new(vec![(404, vec![], vec![])]), urls: Mutex::new(Vec::new()) };
    assert!(open_run(&nf, &run_url()).err().unwrap().contains("HTTP 404"));
}

#[test]
fn the_published_checksum_file_is_read_by_file_name() {
    let t = format!("{}  {}\n", RUN_SHA256, run_file());
    assert_eq!(published_sha(&t, &run_file()).as_deref(), Some(RUN_SHA256));
    assert_eq!(published_sha(&t, "other.run"), None);
    assert_eq!(published_sha("nothex  file", "file"), None);
    assert_eq!(published_sha(&format!("{}  *{}", RUN_SHA256.to_uppercase(), run_file()), &run_file()).as_deref(), Some(RUN_SHA256), "binary-mode star and capitals");
}

#[test]
fn the_licence_notice_says_what_is_downloaded_from_whom_and_how_big() {
    let n = licence_notice();
    for needle in [NVIDIA_VERSION, &run_file(), "download.nvidia.com", &format!("{} MB", RUN_BYTES >> 20), "MIT / GPL-2.0", "proprietary", "NVIDIA's licence", "never part of an image", "Apache-2.0", "CDI", "open specification"] {
        assert!(n.contains(needle), "the notice lacks `{needle}`:\n{n}");
    }
    assert!(crate::quickinstall::Item::GpuNvidia.licence().contains("accepts NVIDIA's licence"));
}

// ---- ELF and staging

/// A minimal 64-bit ELF shared object with a dynamic section holding DT_SONAME.
fn fake_elf(soname: &str) -> Vec<u8> {
    let mut strtab = vec![0u8];
    strtab.extend(soname.as_bytes());
    strtab.push(0);
    let dyn_off = 64 + 2 * 56;
    let str_off = dyn_off + 3 * 16;
    let total = str_off + strtab.len();
    let mut b = vec![0u8; 64];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
    b[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
    b[0x38..0x3a].copy_from_slice(&2u16.to_le_bytes());
    let ph = |ty: u32, off: u64, size: u64| {
        let mut p = vec![0u8; 56];
        p[0..4].copy_from_slice(&ty.to_le_bytes());
        p[8..16].copy_from_slice(&off.to_le_bytes());
        p[16..24].copy_from_slice(&off.to_le_bytes());
        p[32..40].copy_from_slice(&size.to_le_bytes());
        p
    };
    b.extend(ph(1, 0, total as u64));
    b.extend(ph(2, dyn_off as u64, 48));
    for (tag, val) in [(5u64, str_off as u64), (14, 1), (0, 0)] {
        b.extend(tag.to_le_bytes());
        b.extend(val.to_le_bytes());
    }
    b.extend(strtab);
    b
}

#[test]
fn the_soname_is_read_from_the_dynamic_section() {
    assert_eq!(elf_soname(&fake_elf("libcuda.so.1")).as_deref(), Some("libcuda.so.1"));
    assert_eq!(elf_soname(&fake_elf("libnvidia-nvvm.so.4")).as_deref(), Some("libnvidia-nvvm.so.4"));
    assert_eq!(elf_soname(b"not an elf"), None);
    assert_eq!(elf_soname(&[]), None);
    let mut cut = fake_elf("libx.so.1");
    cut.truncate(100);
    assert_eq!(elf_soname(&cut), None, "a truncated file is not an error");
}

/// An extracted vendor package, as `--extract-only` leaves it (the parts that matter), with the `.manifest` the real package has: its
/// lines are copied from the manifest of 595.99.02 (symlinks are entries there, not files in the folder).
pub(crate) fn fake_package(d: &Path, version: &str) {
    fake_package_without_manifest(d, version);
    let m = format!(
        "NVIDIA Accelerated Graphics Driver for Linux-x86_64\n{version}\n \nnvidia nvidia-uvm  nvidia-modeset nvidia-drm nvidia-peermem\n \n \n \n \n\
kernel/nvidia/nv-platform.c 0644 KERNEL_MODULE_SRC INHERIT_PATH_DEPTH:1 MODULE:resman\n\
nvidia-smi 0755 UTILITY_BINARY MODULE:nvml\n\
nvidia-smi.1.gz 0444 MANPAGE man1/ MODULE:nvml\n\
libnvidia-ml.so.{version} 0755 CUDA_LIB NATIVE / MODULE:nvml\n\
libnvidia-ml.so.1 0000 CUDA_SYMLINK NATIVE / libnvidia-ml.so.{version} MODULE:nvml\n\
libnvidia-ml.so 0000 CUDA_SYMLINK NATIVE / libnvidia-ml.so.1 MODULE:nvml\n\
libcuda.so.{version} 0755 CUDA_LIB NATIVE / MODULE:gpgpu\n\
libcuda.so.1 0000 CUDA_SYMLINK NATIVE / libcuda.so.{version} MODULE:gpgpu\n\
libcuda.so 0000 CUDA_SYMLINK NATIVE / libcuda.so.1 MODULE:gpgpu\n\
libnvidia-nvvm.so.{version} 0755 CUDA_LIB NATIVE / MODULE:compiler\n\
libnvidia-nvvm.so.4 0000 CUDA_SYMLINK NATIVE / libnvidia-nvvm.so.{version} MODULE:compiler\n\
libnvidia-nvvm.so 0000 CUDA_SYMLINK NATIVE / libnvidia-nvvm.so.4 MODULE:compiler\n\
libnvidia-nvvm70.so.4 0755 CUDA_LIB NATIVE / MODULE:compiler\n\
libnvidia-ptxjitcompiler.so.{version} 0755 CUDA_LIB NATIVE / MODULE:ptxjit\n\
libnvidia-ptxjitcompiler.so.1 0000 CUDA_SYMLINK NATIVE / libnvidia-ptxjitcompiler.so.{version} MODULE:ptxjit\n\
./32/libcuda.so.{version} 0755 CUDA_LIB COMPAT32 / MODULE:gpgpu\n\
./32/libcuda.so.1 0000 CUDA_SYMLINK COMPAT32 / libcuda.so.{version} MODULE:gpgpu\n\
nvidia_icd.json 0644 VULKAN_ICD_JSON icd.d/ MODULE:opengl\n\
libGLX_nvidia.so.{version} 0755 OPENGL_LIB NATIVE MODULE:opengl\n\
libGLX_nvidia.so.0 0000 OPENGL_SYMLINK NATIVE libGLX_nvidia.so.{version} MODULE:opengl\n\
libGLX_indirect.so.0 0000 OPENGL_SYMLINK NATIVE libGLX_nvidia.so.{version} MODULE:opengl\n\
libnvidia-egl-gbm.so.{version} 0755 OPENGL_LIB NATIVE MODULE:opengl\n\
libnvidia-egl-gbm.so.1 0000 OPENGL_SYMLINK NATIVE libnvidia-egl-gbm.so.{version} MODULE:opengl\n\
libGL.so.1.7.0 0755 GLVND_LIB NATIVE MODULE:glvnd\n\
firmware/gsp_ga10x.bin 0444 FIRMWARE INHERIT_PATH_DEPTH:1 MODULE:gsp\n"
    );
    std::fs::write(d.join(".manifest"), m).unwrap();
    std::fs::write(d.join("libnvidia-nvvm70.so.4"), fake_elf("libnvidia-nvvm70.so.4")).unwrap();
}

/// The same package without a `.manifest` (an older layout): the fallback by file name and ELF soname.
pub(crate) fn fake_package_without_manifest(d: &Path, version: &str) {
    std::fs::create_dir_all(d.join("firmware")).unwrap();
    std::fs::create_dir_all(d.join("kernel-open")).unwrap();
    for (name, so) in [("libcuda", "libcuda.so.1"), ("libnvidia-ml", "libnvidia-ml.so.1"), ("libnvidia-ptxjitcompiler", "libnvidia-ptxjitcompiler.so.1"), ("libnvidia-nvvm", "libnvidia-nvvm.so.4"), ("libGLX_nvidia", "libGLX_nvidia.so.0"), ("libGL", "libGL.so.1"), ("libnvidia-egl-gbm", "libnvidia-egl-gbm.so.1")] {
        std::fs::write(d.join(format!("{name}.so.{version}")), fake_elf(so)).unwrap();
    }
    std::fs::write(d.join(format!("libcuda.so.{version}.bak")), b"junk").unwrap();
    std::fs::write(d.join("nvidia-smi"), b"#!smi").unwrap();
    std::fs::write(d.join("nvidia-persistenced"), b"#!p").unwrap();
    std::fs::write(d.join("nvidia-cuda-mps-control"), b"#!mps").unwrap();
    std::fs::write(d.join("nvidia_icd.json"), "{\"file_format_version\":\"1.0.1\",\"ICD\":{\"library_path\":\"libGLX_nvidia.so.0\",\"api_version\":\"1.4.0\"}}").unwrap();
    std::fs::write(d.join("firmware/gsp_ga10x.bin"), vec![1u8; 2048]).unwrap();
    std::fs::write(d.join("firmware/gsp_tu10x.bin"), vec![2u8; 1024]).unwrap();
    std::fs::write(d.join("firmware/other.bin"), b"x").unwrap();
    std::fs::write(d.join("kernel-open/nvidia.c"), b"/* source */").unwrap();
}

fn names(p: PathBuf) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn the_manifest_decides_which_files_are_libraries_and_which_names_are_links() {
    let m = parse_manifest(&std::fs::read_to_string({
        let d = tmp("manifest");
        fake_package(&d, NVIDIA_VERSION);
        d.join(".manifest")
    }).unwrap());
    // the header lines and the kernel sources are not entries with a library type
    assert!(m.iter().all(|e| !e.name.starts_with("NVIDIA") && e.ty != "KERNEL_MODULE_SRC" || e.name == "kernel/nvidia/nv-platform.c"));
    let cuda1 = m.iter().find(|e| e.name == "libcuda.so.1").unwrap();
    assert_eq!((cuda1.ty.as_str(), cuda1.target.as_deref()), ("CUDA_SYMLINK", Some(format!("libcuda.so.{NVIDIA_VERSION}").as_str())));
    let glx = m.iter().find(|e| e.name == "libGLX_nvidia.so.0").unwrap();
    assert_eq!(glx.target.as_deref(), Some(format!("libGLX_nvidia.so.{NVIDIA_VERSION}").as_str()), "the OpenGL link lines have no install path field");
    let (files, links) = wanted_from_manifest(&m);
    assert!(files.contains(&format!("libcuda.so.{NVIDIA_VERSION}")) && files.contains(&"libnvidia-nvvm70.so.4".to_string()), "a library without the version in its name is a library too: {files:?}");
    assert!(files.contains(&format!("libGLX_nvidia.so.{NVIDIA_VERSION}")));
    assert!(!files.iter().any(|f| f.starts_with("./32/") || f.contains("egl-gbm") || f.starts_with("libGL.so")), "32-bit libraries, EGL glue and the GLVND set are not kept: {files:?}");
    assert!(links.contains(&("libcuda.so".to_string(), "libcuda.so.1".to_string())), "a link to a link whose chain ends in a kept file");
    assert!(links.contains(&("libcuda.so.1".to_string(), format!("libcuda.so.{NVIDIA_VERSION}"))));
    assert!(!links.iter().any(|(n, _)| n.contains("egl-gbm") || n.starts_with("./32")), "{links:?}");
}

#[test]
fn staging_keeps_only_what_a_container_needs_and_makes_the_links_the_vendor_defines() {
    let data = tmp("stage");
    let l = Layout::new(&data);
    assert!(!l.staged());
    let ex = l.extracted();
    fake_package(&ex, NVIDIA_VERSION);
    let s = stage(&ex, &l).unwrap();
    assert_eq!((s.libs, s.bins, s.firmware, s.icd), (6, 2, 2, true), "{s:?}");
    assert_eq!(s.links, 9, "{s:?}");
    let libs = names(l.lib());
    for want in [format!("libcuda.so.{NVIDIA_VERSION}"), "libcuda.so.1".into(), "libcuda.so".into(), "libnvidia-nvvm.so.4".into(), "libnvidia-nvvm70.so.4".into(), "libGLX_nvidia.so.0".into(), "libnvidia-ptxjitcompiler.so.1".into()] {
        assert!(libs.contains(&want), "{want} missing in {libs:?}");
    }
    assert!(!libs.iter().any(|n| n.starts_with("libGL.") || n.contains("egl-gbm") || n.ends_with(".bak")), "{libs:?}");
    assert_eq!(std::fs::read_link(l.lib().join("libcuda.so.1")).unwrap(), PathBuf::from(format!("libcuda.so.{NVIDIA_VERSION}")), "a relative link: it works inside the container's mount too");
    assert_eq!(std::fs::read_link(l.lib().join("libcuda.so")).unwrap(), PathBuf::from("libcuda.so.1"));
    assert_eq!(names(l.bin()), ["nvidia-cuda-mps-control", "nvidia-smi"], "no persistenced daemon");
    assert_eq!(names(l.firmware().join("nvidia").join(NVIDIA_VERSION)), ["gsp_ga10x.bin", "gsp_tu10x.bin"]);
    assert!(l.vulkan().join("nvidia_icd.json").is_file());
    assert!(!l.stage().join("kernel-open").exists(), "no kernel sources in the stage");
    assert!(l.staged());
    // the download and the unpacked tree go away, the stage stays
    std::fs::write(l.run(), b"x").unwrap();
    clean_download(&l);
    assert!(!l.run().exists() && !l.extracted().exists() && l.staged());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn without_a_manifest_the_versioned_files_and_their_sonames_are_used() {
    let data = tmp("stage-nomanifest");
    let l = Layout::new(&data);
    fake_package_without_manifest(&l.extracted(), NVIDIA_VERSION);
    let s = stage(&l.extracted(), &l).unwrap();
    assert_eq!((s.libs, s.links, s.bins, s.firmware), (5, 5, 2, 2), "{s:?}");
    let libs = names(l.lib());
    assert!(libs.contains(&"libcuda.so.1".to_string()) && libs.contains(&"libnvidia-nvvm.so.4".to_string()) && !libs.iter().any(|n| n.contains("egl-gbm")), "{libs:?}");
    let _ = std::fs::remove_dir_all(&data);
}

/// With `NVIDIA_EXTRACTED=<folder>` (the output of `sh NVIDIA-Linux-x86_64-<version>.run --extract-only`) the staging runs on the real
/// vendor package. Skipped otherwise: the file is 400 MB and the tests use no network.
#[test]
fn staging_a_real_vendor_package_when_one_is_given() {
    let Ok(dir) = std::env::var("NVIDIA_EXTRACTED") else { return };
    let data = tmp("stage-real");
    let l = Layout::new(&data);
    let s = stage(Path::new(&dir), &l).unwrap();
    println!("real package staged: {s:?}");
    assert!(s.libs >= 15 && s.links >= 10 && s.bins == 4 && s.firmware == 2 && s.icd, "{s:?}");
    assert!(s.bytes < 700 << 20, "the stage stays well under the package size: {s:?}");
    for want in [format!("libcuda.so.{NVIDIA_VERSION}"), "libcuda.so.1".into(), "libcuda.so".into(), "libnvidia-ml.so.1".into(), "libnvidia-nvvm.so.4".into(), "libnvidia-nvvm70.so.4".into(), "libnvidia-ptxjitcompiler.so.1".into(), format!("libGLX_nvidia.so.{NVIDIA_VERSION}"), "libGLX_nvidia.so.0".into(), "libnvidia-glvkspirv.so.".to_string() + NVIDIA_VERSION, "libnvidia-api.so.1".into()] {
        assert!(l.lib().join(&want).exists() || std::fs::symlink_metadata(l.lib().join(&want)).is_ok(), "{want}");
    }
    // every link resolves to a file in the stage
    for e in std::fs::read_dir(l.lib()).unwrap().flatten() {
        assert!(e.path().exists(), "dangling link {}", e.path().display());
    }
    assert!(!names(l.lib()).iter().any(|n| n.contains("egl") || n.contains("GLES") || n.contains("wayland")));
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_package_of_another_version_is_refused() {
    let data = tmp("stage-bad");
    let l = Layout::new(&data);
    fake_package(&l.extracted(), "550.54.14");
    let e = stage(&l.extracted(), &l).unwrap_err();
    assert!(e.contains("not the pinned version"), "{e}");
    assert!(!l.staged());
    let _ = std::fs::remove_dir_all(&data);
}

// ---- device nodes

#[test]
fn the_device_nodes_follow_what_nvidia_modprobe_makes() {
    let devices = "Character devices:\n  1 mem\n195 nvidia\n195 nvidiactl\n226 drm\n504 nvidia-uvm\n510 nvidia-caps\n\nBlock devices:\n259 blkext\n";
    assert_eq!(device_major(devices, "nvidia-uvm"), Some(504));
    assert_eq!(device_major(devices, "blkext"), None, "block devices do not count");
    let n = nodes_to_make(&[0, 1], devices);
    assert_eq!(n.iter().map(|x| (x.path.as_str(), x.major, x.minor)).collect::<Vec<_>>(), [("/dev/nvidiactl", 195, 255), ("/dev/nvidia0", 195, 0), ("/dev/nvidia1", 195, 1), ("/dev/nvidia-uvm", 504, 0), ("/dev/nvidia-uvm-tools", 504, 1)]);
    assert_eq!(nodes_to_make(&[0], "Character devices:\n195 nvidia\n").len(), 2, "without nvidia-uvm only the control and GPU nodes");
    let info = "Model: \t\t NVIDIA GeForce RTX 4090\nIRQ:   \t\t 169\nGPU UUID: \t GPU-1b8a6e20-5ab2-3c55-b3a8-b1cb84a9ea5b\nVideo BIOS: \t 95.02.18.80.fc\nBus Type: \t PCIe\nDevice Minor: \t 0\n";
    assert_eq!(parse_minor(info), Some(0));
    assert_eq!(parse_uuid(info).as_deref(), Some("GPU-1b8a6e20-5ab2-3c55-b3a8-b1cb84a9ea5b"));
    let r = tmp("proc");
    let g = r.join("proc/driver/nvidia/gpus/0000:01:00.0");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("information"), info).unwrap();
    std::fs::write(r.join("proc/driver/nvidia/version"), format!("NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  {NVIDIA_VERSION}  Release Build  (root@iyakki)  Tue Oct 20 2026\nGCC version:  gcc version 14.3.0\n")).unwrap();
    assert_eq!(driver_gpus(&r), vec![(0, Some("GPU-1b8a6e20-5ab2-3c55-b3a8-b1cb84a9ea5b".to_string()))]);
    assert_eq!(loaded_version(&r).as_deref(), Some(NVIDIA_VERSION));
    let _ = std::fs::remove_dir_all(&r);
}

// ---- the CDI spec

/// A machine as the installer leaves it: staged files, device nodes, a driver that found one GPU.
fn machine(root: &Path, data: &Path) -> Layout {
    let l = Layout::new(data);
    fake_package(&l.extracted(), NVIDIA_VERSION);
    stage(&l.extracted(), &l).unwrap();
    std::fs::create_dir_all(root.join("dev")).unwrap();
    for n in ["nvidiactl", "nvidia0", "nvidia-uvm", "nvidia-uvm-tools"] {
        std::fs::write(root.join("dev").join(n), b"").unwrap();
    }
    l
}

#[test]
fn the_nvidia_cdi_spec_matches_its_golden_file() {
    let root = tmp("cdi-root");
    let data = root.join("var/lib/iyakki");
    let l = machine(&root, &data);
    let gpus = vec![(0u32, Some("GPU-1b8a6e20-5ab2-3c55-b3a8-b1cb84a9ea5b".to_string()))];
    let spec = cdi_spec(crate::gpucdi::NVIDIA_KIND, &root, &l, &gpus).unwrap();
    // the paths in the file are the machine's own; the golden file is written for the standard data folder
    let text = spec.to_json().replace(&data.display().to_string(), "/var/lib/iyakki");
    golden("cdi-nvidia.json", &text);
    // properties the golden file also shows, said in words
    let names: Vec<&str> = spec.devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["0", "GPU-1b8a6e20-5ab2-3c55-b3a8-b1cb84a9ea5b", "all"]);
    assert!(spec.common.env.contains(&"LD_LIBRARY_PATH=/usr/local/nvidia/lib64".to_string()));
    let cp: Vec<&str> = spec.common.mounts.iter().map(|m| m.container_path.as_str()).collect();
    for want in ["/usr/local/nvidia/lib64/libcuda.so.1", &format!("/usr/local/nvidia/lib64/libcuda.so.{NVIDIA_VERSION}"), "/usr/local/nvidia/lib64/libnvidia-ml.so.1", "/usr/local/nvidia/bin/nvidia-smi", "/usr/bin/nvidia-smi", "/etc/vulkan/icd.d/nvidia_icd.json"] {
        assert!(cp.contains(&want), "no mount at {want}: {cp:?}");
    }
    // a soname link and its real file are the same host file (the runtime follows no link inside the stage)
    let m = |c: &str| spec.common.mounts.iter().find(|m| m.container_path == c).unwrap().host_path.clone();
    assert_eq!(m("/usr/local/nvidia/lib64/libcuda.so.1"), m(&format!("/usr/local/nvidia/lib64/libcuda.so.{NVIDIA_VERSION}")));
    assert!(spec.common.mounts.iter().all(|m| m.options.contains(&"ro".to_string())), "everything is read-only");
    assert_eq!(spec.common.device_nodes.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["/dev/nvidiactl", "/dev/nvidia-uvm", "/dev/nvidia-uvm-tools"], "nvidia-modeset is not there, so not listed");
    // the plugin's name gets the same content
    let other = cdi_spec(crate::gpucdi::PLUGIN_KIND, &root, &l, &gpus).unwrap();
    assert_eq!(other.kind, "k8s.device-plugin.nvidia.com/gpu");
    assert_eq!(other.devices, spec.devices);
    assert_eq!(other.file_name(), "k8s.device-plugin.nvidia.com-gpu.json");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_cdi_spec_refuses_a_machine_that_is_not_ready() {
    let root = tmp("cdi-bad");
    let data = root.join("data");
    let l = machine(&root, &data);
    assert!(cdi_spec(crate::gpucdi::NVIDIA_KIND, &root, &l, &[]).unwrap_err().contains("no GPU"));
    std::fs::remove_file(root.join("dev/nvidiactl")).unwrap();
    assert!(cdi_spec(crate::gpucdi::NVIDIA_KIND, &root, &l, &[(0, None)]).unwrap_err().contains("/dev/nvidiactl"));
    let two = {
        std::fs::write(root.join("dev/nvidiactl"), b"").unwrap();
        cdi_spec(crate::gpucdi::NVIDIA_KIND, &root, &l, &[(0, None), (1, None)]).unwrap()
    };
    assert_eq!(two.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["0", "1", "all"], "two GPUs: by index, and all");
    assert_eq!(two.devices[2].edits.device_nodes.len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

// ---- the state machine

#[test]
fn the_install_state_machine_names_the_first_missing_thing() {
    let mut f = Facts::default();
    assert_eq!(next_stage(&f), Stage::NoGpu);
    f.gpu_present = true;
    assert_eq!(next_stage(&f), Stage::UnsupportedGpu);
    f.open_supported = true;
    assert_eq!(next_stage(&f), Stage::NoModule);
    f.module_present = true;
    f.nouveau_loaded = true;
    assert_eq!(next_stage(&f), Stage::NouveauInTheWay, "refused before anything is downloaded");
    assert!(Stage::NouveauInTheWay.refuses() && Stage::NoModule.refuses() && !Stage::NeedsDownload.refuses());
    f.nouveau_loaded = false;
    assert_eq!(next_stage(&f), Stage::NeedsDownload);
    f.vendor_files = true;
    assert_eq!(next_stage(&f), Stage::NeedsModuleLoad);
    f.nvidia_loaded = true;
    assert_eq!(next_stage(&f), Stage::NeedsModuleLoad, "loaded, but not the pinned version");
    f.version_matches = true;
    assert_eq!(next_stage(&f), Stage::NeedsCdi);
    f.cdi_written = true;
    assert_eq!(next_stage(&f), Stage::NeedsContainerdCdi);
    f.cdi_enabled = true;
    assert_eq!(next_stage(&f), Stage::NeedsPlugin);
    f.plugin_ready = true;
    assert_eq!(next_stage(&f), Stage::WaitingForNode);
    f.node_reports = true;
    assert_eq!(next_stage(&f), Stage::Ready);
    for s in [Stage::NoGpu, Stage::UnsupportedGpu, Stage::NoModule, Stage::NouveauInTheWay, Stage::NeedsDownload, Stage::NeedsModuleLoad, Stage::NeedsCdi, Stage::NeedsContainerdCdi, Stage::NeedsPlugin, Stage::WaitingForNode, Stage::Ready] {
        assert!(!s.words().is_empty());
    }
    // nouveau loaded next to nvidia is not "in the way": the NVIDIA driver already runs
    let both = Facts { gpu_present: true, open_supported: true, module_present: true, nouveau_loaded: true, nvidia_loaded: true, vendor_files: true, version_matches: true, ..Default::default() };
    assert_eq!(next_stage(&both), Stage::NeedsCdi);
}

#[test]
fn machine_facts_come_from_sysfs_proc_and_the_data_folder() {
    let root = tmp("facts");
    let data = root.join("var/lib/iyakki");
    let g = |dev: u16| crate::gpu::Gpu { pci: "0000:01:00.0".into(), vendor_id: crate::gpu::NVIDIA, device_id: dev, vendor: crate::gpu::Vendor::Nvidia, name: String::new(), driver: None, card: None, render: None, boot_vga: false, vram: None };
    let f = machine_facts(&root, &data, &[g(0x2684)]);
    assert!(f.gpu_present && f.open_supported && !f.module_present && !f.vendor_files && !f.nvidia_loaded);
    assert!(!machine_facts(&root, &data, &[g(0x1b80)]).open_supported, "Pascal");
    assert!(!machine_facts(&root, &data, &[]).gpu_present);
    // everything in place
    machine(&root, &data);
    std::fs::create_dir_all(root.join("lib/modules/6.12.112/updates")).unwrap();
    std::fs::write(root.join("lib/modules/6.12.112/updates/nvidia.ko"), b"x").unwrap();
    std::fs::create_dir_all(root.join("proc/driver/nvidia")).unwrap();
    std::fs::write(root.join("proc/modules"), "nvidia 1 0 - Live 0x0\n").unwrap();
    std::fs::write(root.join("proc/driver/nvidia/version"), format!("NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  {NVIDIA_VERSION}  Release Build\n")).unwrap();
    std::fs::create_dir_all(root.join("var/run/cdi")).unwrap();
    std::fs::write(root.join("var/run/cdi/nvidia.com-gpu.json"), "{}").unwrap();
    let f = machine_facts(&root, &data, &[g(0x2684)]);
    assert!(f.module_present && f.nvidia_loaded && f.version_matches && f.vendor_files && f.cdi_written && !f.nouveau_loaded, "{f:?}");
    std::fs::write(root.join("proc/driver/nvidia/version"), "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  550.54.14  Release Build\n").unwrap();
    assert!(!machine_facts(&root, &data, &[g(0x2684)]).version_matches, "another version loaded");
    let _ = std::fs::remove_dir_all(&root);
}

// ---- the device plugin

#[test]
fn the_plugin_manifest_uses_cdi_without_hooks_and_the_kubelets_folder() {
    let m = plugin_manifest("/var/lib/kubelet/", "/var/lib/iyakki/gpu/nvidia/595.99.02/stage");
    assert!(m.contains(PLUGIN_IMAGE) && m.contains("kind: DaemonSet") && m.contains("namespace: kube-system"));
    for env in ["DEVICE_LIST_STRATEGY\n              value: cdi-annotations", "DEVICE_ID_STRATEGY\n              value: index", "CDI_DISABLE_HOOKS\n              value: all"] {
        assert!(m.contains(env), "{env}");
    }
    assert!(m.contains("path: /var/lib/kubelet/device-plugins"), "k3s keeps the kubelet in /var/lib/kubelet whatever its data folder is (checked on v1.37.1)");
    assert!(m.contains("mountPath: /var/lib/kubelet/device-plugins"), "inside the pod the plugin sees the usual path");
    assert!(m.contains("path: /var/lib/iyakki/gpu/nvidia/595.99.02/stage") && m.contains("mountPath: /usr/local/nvidia"));
    assert!(m.contains("emptyDir"), "the plugin's own spec goes to a throw-away folder: IYAKKI's spec on the host is the one containerd reads");
    assert!(!m.contains("runtimeClassName") && !m.contains("nvidia-ctk") && !m.contains("nvidia-container-runtime"), "no toolkit, no runtime class");
    assert!(m.contains("app.kubernetes.io/managed-by: iyakki"));
}

// ---- unpacking the vendor file without NVIDIA's own (glibc) helper

/// A makeself-shaped file: a script header whose `skip=` names the line the payload starts at (binary bytes with newlines in it, like the real
/// header's embedded program), then a zstd-compressed tar.
fn fake_run(skip_extra_lines: usize, payload: &[u8]) -> Vec<u8> {
    let mut h = String::from("#! /bin/sh\nCRCsum=1\nMD5=x\n");
    let lines_before_payload = 4 + skip_extra_lines; // header lines up to and including the `skip=` line, plus the embedded program
    h.push_str(&format!("skip={}\n", lines_before_payload + 1));
    let mut out = h.into_bytes();
    // an embedded "program" of binary lines (includes bytes that are not text)
    for i in 0..skip_extra_lines {
        out.extend_from_slice(&[0x7f, b'E', b'L', b'F', i as u8, 0xff, 0x00, b'\n']);
    }
    out.extend_from_slice(payload);
    out
}

#[test]
fn the_vendor_run_file_is_unpacked_here_from_its_zstd_payload() {
    use crate::archive::tests::make_tar;
    let tar = make_tar(&[("nvidia-smi", b'0', "smi", 0o755), ("libcuda.so.595.99.02", b'0', "cuda", 0o755), ("firmware/gsp_ga10x.bin", b'0', "gsp", 0o644)]);
    let z = ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest);
    let d = tmp("nvrun");
    let run = d.join("x.run");
    std::fs::write(&run, fake_run(7, &z)).unwrap();
    let dest = d.join("extracted");
    assert_eq!(extract_makeself_zstd(&run, &dest).unwrap(), 3);
    assert_eq!(std::fs::read(dest.join("nvidia-smi")).unwrap(), b"smi");
    assert_eq!(std::fs::read(dest.join("firmware/gsp_ga10x.bin")).unwrap(), b"gsp");
    // not zstd: another makeself compression is said so (the caller then falls back to `sh <file> --extract-only`)
    std::fs::write(&run, fake_run(2, b"\x1f\x8b\x08 gzip payload")).unwrap();
    assert!(extract_makeself_zstd(&run, &d.join("e2")).unwrap_err().contains("not zstd"));
    // not a makeself file at all
    std::fs::write(&run, b"ELF binary\n").unwrap();
    assert!(extract_makeself_zstd(&run, &d.join("e3")).unwrap_err().contains("skip="));
    // a header that ends before the payload
    std::fs::write(&run, b"#! /bin/sh\nskip=500\nshort\n").unwrap();
    assert!(extract_makeself_zstd(&run, &d.join("e4")).unwrap_err().contains("ends before the payload"));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_device_plugin_looks_for_the_driver_where_the_stage_is_mounted() {
    // the pod stopped with "failed to locate libcuda.so.595.99.02" when its driver root was a folder that is not mounted
    let m = plugin_manifest("/var/lib/iyakki/k3s/agent/kubelet", "/var/lib/iyakki/gpu/nvidia/595.99.02/stage");
    assert!(m.contains(&format!("name: NVIDIA_DRIVER_ROOT\n              value: {CONTAINER_ROOT}\n")), "{m}");
    // the plugin reads its driver root inside the pod from CONTAINER_DRIVER_ROOT (default /driver-root, which does not exist here)
    assert!(m.contains(&format!("name: CONTAINER_DRIVER_ROOT\n              value: {CONTAINER_ROOT}\n")), "{m}");
    assert!(m.contains(&format!("mountPath: {CONTAINER_ROOT}")) && !m.contains("/driver-root"));
    assert!(m.contains(&format!("value: {CONTAINER_ROOT}/lib64")), "the libraries are under lib64 of that root");
}

#[test]
fn the_driver_is_loaded_with_dynamic_power_management_and_the_slot_is_let_to_sleep() {
    assert_eq!(modprobe_argv("nvidia"), ["modprobe", "nvidia", "NVreg_DynamicPowerManagement=0x02"]);
    assert_eq!(modprobe_argv("nvidia-uvm"), ["modprobe", "nvidia-uvm"], "only the main module takes the option");
    let root = tmp("nvpower");
    let dev = root.join("sys/bus/pci/devices");
    for (addr, class, vendor, ctl) in [("0000:01:00.0", "0x030000", "0x10de", "on"), ("0000:01:00.1", "0x040300", "0x10de", "on"), ("0000:04:00.0", "0x028000", "0x14c3", "on"), ("0000:05:00.0", "0x030000", "0x1002", "on")] {
        let d = dev.join(addr);
        std::fs::create_dir_all(d.join("power")).unwrap();
        std::fs::write(d.join("class"), format!("{class}\n")).unwrap();
        std::fs::write(d.join("vendor"), format!("{vendor}\n")).unwrap();
        std::fs::write(d.join("device"), "0x2520\n").unwrap();
        std::fs::write(d.join("power/control"), format!("{ctl}\n")).unwrap();
    }
    let changed = allow_power_saving(&root);
    assert_eq!(changed, ["0000:01:00.0", "0000:01:00.1"], "the card and its audio function; not the Wi-Fi, not the AMD graphics");
    let read = |a: &str| std::fs::read_to_string(dev.join(a).join("power/control")).unwrap();
    assert_eq!(read("0000:01:00.0").trim(), "auto");
    assert_eq!(read("0000:04:00.0").trim(), "on");
    assert_eq!(read("0000:05:00.0").trim(), "on");
    assert!(allow_power_saving(&root).is_empty(), "nothing to change the second time");
    let _ = std::fs::remove_dir_all(&root);
}

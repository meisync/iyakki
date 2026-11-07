//! The PC image (variant "pc", docs/GPU-K3S.md): the macweb recipe for ordinary x86-64 PCs with the whole GPU firmware set and the common PC
//! Wi-Fi, Bluetooth and Ethernet firmware. No build is run here: the keep list, the scripts that wire the variant and the NVIDIA module package
//! are checked as files, the way boot_menu.rs checks the Mac list.

use std::path::{Path, PathBuf};

fn dist() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(dist().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn repo(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Run prune-firmware.sh with the PC keep list on a made-up firmware tree: the GPU, Wi-Fi and Ethernet files stay, Mac-only and server files go.
#[test]
fn the_pc_firmware_list_keeps_gpus_wifi_and_ethernet_and_drops_the_rest() {
    let list = read("board/iyakki/pc-firmware.keep");
    let required: Vec<&str> = list.lines().filter_map(|l| l.strip_prefix("# required:")).flat_map(|l| l.split_whitespace()).collect();
    assert!(required.len() >= 16, "the list names the files it must keep: {}", required.len());
    // the GPUs the point of the image: NVIDIA GSP for Turing, Ampere and Ada, AMD, Intel
    for need in ["nvidia/ga102/gsp/gsp-535.113.01.bin", "nvidia/ad102/gsp/gsp-535.113.01.bin", "nvidia/tu102/gsp/gsp-535.113.01.bin", "amdgpu/gc_11_0_1_pfp.bin", "i915/tgl_dmc_ver2_12.bin", "rtw89/rtw8852a_fw.bin", "rtl_nic/rtl8153b-2.fw"] {
        assert!(required.contains(&need), "{need} must be a required file of the list");
    }
    let dir = std::env::temp_dir().join(format!("iyakki-pcfw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let fw = dir.join("fw");
    // what must go: the 570 GSP files (kernel 6.12 loads 535), router/phone Wi-Fi parts, server NICs, TV tuners, Qualcomm phone parts
    let drop = ["nvidia/ga102/gsp/gsp-570.144.bin", "nvidia/ad102/gsp/booter_load-570.144.bin", "ath12k/QCN9274/hw1.0/amss.bin", "ath11k/IPQ8074/hw2.0/amss.bin", "mediatek/mt8196/scp.img", "mellanox/mlxsw_spectrum-13.2000.1122.mfa2", "cxgb4/t6fw.bin", "dvb-fe-xc5000-1.6.114.fw", "qcom/a530_pm4.fw", "intel/sof/sof-tgl.ri", "amd-ucode/microcode_amd.bin", "amdgpu/isp_4_1_1.bin", "ath10k/WCN3990/hw1.0/qcm2290/firmware-5.bin"];
    for f in required.iter().chain(drop.iter()) {
        let p = fw.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
    }
    // Ada's GSP file is a link into ga102's folder: a kept link keeps its target
    let _ = std::fs::remove_file(fw.join("nvidia/ad102/gsp/gsp-535.113.01.bin"));
    std::fs::create_dir_all(fw.join("nvidia/ga102/gsp")).unwrap();
    std::fs::write(fw.join("nvidia/ga102/gsp/gsp-535.113.01.bin"), b"y").unwrap();
    std::os::unix::fs::symlink("../../ga102/gsp/gsp-535.113.01.bin", fw.join("nvidia/ad102/gsp/gsp-535.113.01.bin")).unwrap();
    let out = std::process::Command::new("sh").arg(dist().join("board/iyakki/prune-firmware.sh")).arg(dist().join("board/iyakki/pc-firmware.keep")).arg(&fw).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for f in &required {
        assert!(fw.join(f).exists(), "PC firmware {f} must stay");
    }
    for f in drop {
        assert!(!fw.join(f).exists(), "{f} must go");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pc_variant_is_wired_into_post_build_repack_and_run_sh() {
    let pb = read("board/iyakki/post-build.sh");
    assert!(pb.contains("pc-*)") && pb.contains("pc-firmware.keep"), "post-build prunes the pc variant with its own list");
    assert!(pb.contains("mac-*|macweb-*)") && pb.contains("mac-firmware.keep"), "the Mac rule is untouched");
    let rp = read("repack.sh");
    assert!(rp.contains("--firmware-from") && rp.contains("copy-firmware.sh") && rp.contains("pc-firmware.keep"));
    assert!(rp.contains(r#"IYAKKI_VARIANT="$VARIANT-x86_64""#), "/etc/iyakki-release names the variant: pc-x86_64");
    let run = repo("run.sh");
    assert!(run.contains("build-pc-layered") && run.contains("repack.sh pc --base target/buildroot-web --kernel-from target/buildroot-mac"));
    // the image goes to its own folder, never over the macweb one
    assert!(rp.contains(r#"target/buildroot-$VARIANT"#) && !run.lines().any(|l| l.contains("build-pc-layered)") && l.contains("buildroot-macweb")));
}

#[test]
fn the_nvidia_modules_are_an_optional_package_and_the_boot_menu_can_turn_nouveau_off() {
    let b = read("build.sh");
    assert!(b.contains("IYAKKI_NVIDIA_OPEN") && b.contains("iyakki_nvidia_open.fragment") && b.contains("rebuild_if_changed nvidia-open"));
    let mk = read("package/nvidia-open/nvidia-open.mk");
    assert!(mk.contains("modules_install") && mk.contains("generic-package") && mk.contains("BR2_PACKAGE_NVIDIA_OPEN"));
    assert!(read("package/nvidia-open/Config.in").contains("depends on BR2_LINUX_KERNEL"));
    let cfg = read("board/iyakki/grub.cfg");
    let e = cfg.split("menuentry \"").find(|e| e.starts_with("IYAKKI - NVIDIA driver (nouveau off)")).expect("an entry that starts without nouveau");
    assert!(e.contains("modprobe.blacklist=nouveau") && e.contains("iyakki_loading") && e.contains("quiet loglevel=3 vt.global_cursor_default=0"), "{e}");
    assert!(!e.contains("nomodeset"), "the other GPU drivers still load");
}

#[test]
fn the_gpu_doc_has_the_numbered_hardware_checklist_and_the_licence_section() {
    let d = repo("docs/GPU-K3S.md");
    for n in 1..=16 {
        assert!(d.lines().any(|l| l.trim_start().starts_with(&format!("{n}. "))), "checklist step {n}");
    }
    for needle in ["## 5. Third-party pieces and licences", "## 9. Troubleshooting", "Secure Boot off", "iyakki hwreport", "nvidia-smi", "tokens/s", "What to send back", "MIT and GPL-2.0", "Apache-2.0", "open specification"] {
        assert!(d.contains(needle), "docs/GPU-K3S.md lacks `{needle}`");
    }
}

/// iwlwifi-keep.sh: for each name the kernel declares, the newest file of that chip not newer than the declared maximum and the one below it.
#[test]
fn the_intel_wifi_firmware_is_chosen_by_what_the_kernel_declares() {
    let dir = std::env::temp_dir().join(format!("iyakki-iwl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let fw = dir.join("fw");
    std::fs::create_dir_all(&fw).unwrap();
    for f in ["iwlwifi-cc-a0-59.ucode", "iwlwifi-cc-a0-72.ucode", "iwlwifi-cc-a0-74.ucode", "iwlwifi-cc-a0-77.ucode", "iwlwifi-cc-a0-89.ucode", "iwlwifi-cc-a0-100.ucode", "iwlwifi-so-a0-gf-a0-89.ucode", "iwlwifi-so-a0-gf-a0.pnvm", "iwlwifi-9260-th-b0-jf-b0-46.ucode", "iwlwifi-9260-th-b0-jf-b0-38.ucode", "iwlwifi-9260-th-b0-jf-b0-34.ucode", "iwlwifi-unrelated-9.ucode"] {
        std::fs::write(fw.join(f), b"x").unwrap();
    }
    let decl = dir.join("declared");
    // the kernel declares cc-a0 up to 77 (the folder also has 89 and 100, which this kernel cannot use), so-a0-gf-a0 up to 89 and its pnvm, 9260 up to 46
    std::fs::write(&decl, "iwlwifi-cc-a0-77.ucode\niwlwifi-so-a0-gf-a0-89.ucode\niwlwifi-so-a0-gf-a0.pnvm\niwlwifi-9260-th-b0-jf-b0-46.ucode\niwlwifi-gone-chip-9.ucode\n").unwrap();
    let run = |declared: Option<&Path>| {
        let mut c = std::process::Command::new("sh");
        c.arg(dist().join("board/iyakki/iwlwifi-keep.sh")).arg(dir.join("nomods")).arg(&fw);
        if let Some(d) = declared {
            c.env("IWLWIFI_DECLARED", d);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect::<Vec<_>>()
    };
    let kept = run(Some(&decl));
    assert_eq!(kept, ["iwlwifi-9260-th-b0-jf-b0-38.ucode", "iwlwifi-9260-th-b0-jf-b0-46.ucode", "iwlwifi-cc-a0-74.ucode", "iwlwifi-cc-a0-77.ucode", "iwlwifi-so-a0-gf-a0-89.ucode", "iwlwifi-so-a0-gf-a0.pnvm"]);
    // no declared names (no modinfo, no modules): everything is kept
    std::fs::write(&decl, "nothing useful\n").unwrap();
    assert_eq!(run(Some(&decl)), ["iwlwifi-*", "intel/iwlwifi/*"]);
    assert!(read("board/iyakki/post-build.sh").contains("iwlwifi-keep.sh"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------ restricted firmware (docs/RESTRICTED-FIRMWARE.md)

fn table_rows() -> Vec<(String, u64, String, String, String)> {
    let mut pack = String::new();
    let mut v = Vec::new();
    for l in read("board/iyakki/restricted-firmware.table").lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.as_slice() {
            ["pack", p] => pack = p.to_string(),
            [k @ ("file" | "doc"), size, sha, path] => v.push((k.to_string(), size.parse().unwrap(), sha.to_string(), path.to_string(), pack.clone())),
            _ => {}
        }
    }
    v
}

fn files_of_table() -> Vec<String> {
    table_rows().into_iter().filter(|r| r.0 == "file").map(|r| r.3).collect()
}

fn sh(args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut c = std::process::Command::new("sh");
    c.args(args).env_remove("IYAKKI_INCLUDE_RESTRICTED_FIRMWARE").env_remove("IYAKKI_RESTRICTED_FIRMWARE_DIR").env_remove("IYAKKI_RESTRICTED_FIRMWARE_TABLE");
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-restricted-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn touch(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// The recipes drop the doubtful files: every file of the pinned table is removed by the Mac and the PC keep lists, and what is clearly redistributable
/// next to them (USB Realtek adapters, Qualcomm Atheros files under LICENSE.QualcommAtheros_ath10k, Intel e100); MediaTek's files are in the table now, so they go too stays.
#[test]
fn the_keep_lists_of_the_published_images_no_longer_carry_the_restricted_files() {
    let rows = files_of_table();
    assert!(rows.len() >= 70 && rows.iter().filter(|f| f.starts_with("qca/")).count() == 20 && rows.contains(&"carl9170-1.fw".to_string()) && rows.iter().any(|f| f.starts_with("tigon/")) && rows.iter().any(|f| f.starts_with("bnx2/")) && rows.iter().any(|f| f.starts_with("rtl_nic/rtl8168")), "{} files", rows.len());
    for (list, decoys) in [("board/iyakki/pc-firmware.keep", vec!["rtl_nic/rtl8153b-2.fw", "rtl_nic/rtl8156b-2.fw", "qca/nvm_usb_00000302.bin", "qca/rampatch_usb_00000302.bin", "ar9170-1.fw", "e100/d101m_ucode.bin"]), ("board/iyakki/mac-firmware.keep", vec!["rtl_nic/rtl8153b-2.fw", "rtl_nic/rtl8156b-2.fw", "ar9170-1.fw"])] {
        let dir = tmpdir("keep");
        let fw = dir.join("fw");
        for f in rows.iter().map(String::as_str).chain(decoys.iter().copied()) {
            touch(&fw.join(f), b"x");
        }
        let out = std::process::Command::new("sh").arg(dist().join("board/iyakki/prune-firmware.sh")).arg(dist().join(list)).arg(&fw).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        for f in &rows {
            assert!(!fw.join(f).exists(), "{f} must not stay in an image built with {list}");
        }
        for f in &decoys {
            assert!(fw.join(f).exists(), "{f} (clearly redistributable) must stay with {list}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    // the hw image has no keep list: the Buildroot selection does not bring the Ethernet blobs in, and post-build.sh strips the rest
    let frag = read("configs/iyakki_firmware.fragment");
    let active: Vec<&str> = frag.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    for opt in ["BROADCOM_TIGON3", "BNX2=", "BNX2X", "RTL_8169"] {
        assert!(!active.iter().any(|l| l.contains(opt)), "{opt} must not be selected for the hw image");
    }
    let pb = read("board/iyakki/post-build.sh");
    let strip = pb.find("restricted-firmware.sh\" strip").expect("post-build strips the restricted files in every variant");
    assert!(pb.contains("restricted-firmware.sh\" add") && strip < pb.find("firmware-licences.sh\"").unwrap(), "stripped before the licence texts are copied");
    // an incremental build notices the switch
    assert!(read("changed.sh").contains("IYAKKI_INCLUDE_RESTRICTED_FIRMWARE") && read("changed.sh").contains("restricted-firmware.table"));
}

#[test]
fn restricted_firmware_is_stripped_from_any_image_tree_and_added_only_when_the_switch_is_set() {
    let script = dist().join("board/iyakki/restricted-firmware.sh");
    let script = script.to_str().unwrap();
    assert!(String::from_utf8_lossy(&sh(&[script, "check"], &[]).stdout).contains("table ok"));
    assert!(String::from_utf8_lossy(&sh(&[script, "excludes"], &[]).stdout).lines().all(|l| l.starts_with('!')));
    // strip: the table's files go, others stay; with the switch nothing is touched
    let dir = tmpdir("strip");
    for f in files_of_table().iter().take(5).map(String::as_str).chain(["rtl_nic/rtl8153b-2.fw", "iwlwifi-x.ucode"]) {
        touch(&dir.join("lib/firmware").join(f), b"x");
    }
    let fw = dir.join("lib/firmware");
    let out = sh(&[script, "strip", fw.to_str().unwrap()], &[("IYAKKI_INCLUDE_RESTRICTED_FIRMWARE", "1")]);
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("nothing is stripped"));
    assert!(files_of_table().iter().take(5).all(|f| fw.join(f).exists()), "with the switch nothing is removed");
    let out = sh(&[script, "strip", fw.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(files_of_table().iter().take(5).all(|f| !fw.join(f).exists()) && fw.join("rtl_nic/rtl8153b-2.fw").exists() && fw.join("iwlwifi-x.ucode").exists());
    let _ = std::fs::remove_dir_all(&dir);

    // add: a small table of made-up files (their real hashes), found in a folder instead of the network
    let work = tmpdir("add");
    let src = work.join("src");
    let (a, b, lic) = (b"firmware a".to_vec(), b"firmware b".to_vec(), b"the licence text".to_vec());
    touch(&src.join("tigon/tg3.bin"), &a);
    touch(&src.join("qca/hmtnv20.bin"), &b);
    touch(&src.join("LICENCE.tigon"), &lic);
    touch(&src.join("LICENSE.qcom"), &lic);
    let sum = |bytes: &[u8]| {
        use std::io::Write;
        let mut c = std::process::Command::new("sha256sum").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        c.stdin.take().unwrap().write_all(bytes).unwrap();
        String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap().split_whitespace().next().unwrap().to_string()
    };
    let mut rows = String::from("commit 1a935bfb330d4a46786eaa903580db9e757b84fa\nbase https://git.kernel.org/x/plain\n");
    rows.push_str(&format!("pack broadcom-net\ndoc {} {} LICENCE.tigon\nfile {} {} tigon/tg3.bin\n", lic.len(), sum(&lic), a.len(), sum(&a)));
    rows.push_str(&format!("pack qualcomm-bt\ndoc {} {} LICENSE.qcom\nfile {} {} qca/hmtnv20.bin\n", lic.len(), sum(&lic), b.len(), sum(&b)));
    // `check` wants 50 rows; the add and fetch commands do not
    let table = work.join("table");
    std::fs::write(&table, rows).unwrap();
    let env_dir = src.to_str().unwrap();
    let tab = table.to_str().unwrap();
    let target = work.join("target");
    touch(&target.join("etc/iyakki-release"), b"VERSION=1\nVARIANT=pc-x86_64\n");
    touch(&target.join("etc/os-release"), b"NAME=Buildroot\n");
    // default OFF: nothing is added
    let out = sh(&[script, "add", target.to_str().unwrap()], &[("IYAKKI_RESTRICTED_FIRMWARE_TABLE", tab), ("IYAKKI_RESTRICTED_FIRMWARE_DIR", env_dir)]);
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("nothing is added"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!target.join("lib/firmware").exists() && !std::fs::read_to_string(target.join("etc/iyakki-release")).unwrap().contains("RESTRICTED"));
    // switch set: files, licence texts, the stamp
    let on = [("IYAKKI_INCLUDE_RESTRICTED_FIRMWARE", "1"), ("IYAKKI_RESTRICTED_FIRMWARE_TABLE", tab), ("IYAKKI_RESTRICTED_FIRMWARE_DIR", env_dir)];
    let out = sh(&[script, "add", target.to_str().unwrap()], &on);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read(target.join("lib/firmware/tigon/tg3.bin")).unwrap(), a);
    assert_eq!(std::fs::read(target.join("lib/firmware/qca/hmtnv20.bin")).unwrap(), b);
    assert_eq!(std::fs::read(target.join("usr/share/doc/linux-firmware-restricted/broadcom-net/LICENCE.tigon")).unwrap(), lic);
    assert!(target.join("usr/share/doc/linux-firmware-restricted/qualcomm-bt/LICENSE.qcom").exists());
    let note = std::fs::read_to_string(target.join("usr/share/doc/linux-firmware-restricted/NOT-FOR-REDISTRIBUTION.txt")).unwrap();
    assert!(note.contains("NOT for redistribution by the IYAKKI project"));
    assert!(std::fs::read_to_string(target.join("etc/iyakki-release")).unwrap().contains("RESTRICTED_FIRMWARE=included"));
    assert!(std::fs::read_to_string(target.join("etc/os-release")).unwrap().contains("not for redistribution by the IYAKKI project"));
    // a file that does not match its pinned hash: the build stops, nothing of it is added
    let target2 = work.join("target2");
    touch(&src.join("qca/hmtnv20.bin"), b"tampered");
    let out = sh(&[script, "add", target2.to_str().unwrap()], &on);
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("qca/hmtnv20.bin") && String::from_utf8_lossy(&out.stderr).contains("nothing was added"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!target2.join("lib/firmware/qca/hmtnv20.bin").exists());
    // fetch: the offline folder for a stick, verified
    touch(&src.join("qca/hmtnv20.bin"), &b);
    let stick = work.join("stick");
    let out = sh(&[script, "fetch", stick.to_str().unwrap(), "broadcom-net"], &[("IYAKKI_RESTRICTED_FIRMWARE_TABLE", tab), ("IYAKKI_RESTRICTED_FIRMWARE_DIR", env_dir)]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stick.join("tigon/tg3.bin").exists() && !stick.join("qca/hmtnv20.bin").exists(), "only the named pack");
    assert!(!sh(&[script, "fetch", stick.to_str().unwrap(), "no-such-pack"], &[("IYAKKI_RESTRICTED_FIRMWARE_TABLE", tab), ("IYAKKI_RESTRICTED_FIRMWARE_DIR", env_dir)]).status.success());
    let _ = std::fs::remove_dir_all(&work);
}

/// The default is OFF everywhere the recipes are driven from: run.sh and the build scripts never set the switch.
#[test]
fn the_build_switch_is_documented_and_never_set_by_the_scripts() {
    for f in ["run.sh", "dist/buildroot/build.sh", "dist/buildroot/repack.sh", "dist/release/release.sh"] {
        let t = repo(f);
        assert!(!t.lines().map(str::trim_start).any(|l| (l.starts_with("IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=") || l.starts_with("export IYAKKI_INCLUDE_RESTRICTED_FIRMWARE") || l.contains(": ${IYAKKI_INCLUDE_RESTRICTED_FIRMWARE:=")) && !l.starts_with('#')), "{f} must not turn the switch on");
    }
    let doc = repo("docs/RESTRICTED-FIRMWARE.md");
    for w in ["IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1", "not for redistribution", "restricted-firmware.sh fetch", "pin-firmware.sh", "Reboot to finish", "I have read the licence and accept"] {
        assert!(doc.contains(w), "docs/RESTRICTED-FIRMWARE.md must mention {w}");
    }
    assert!(repo("run.sh").contains("IYAKKI_INCLUDE_RESTRICTED_FIRMWARE"), "run.sh help names the switch");
}

#[test]
fn every_image_says_whether_it_is_releasable_in_its_release_file() {
    let pb = read("board/iyakki/post-build.sh");
    // a non-release image is stamped dev, and bundled when vendor files are inside; the release profile stamps release / none
    assert!(pb.contains("echo \"IMAGE_PROFILE=dev\"") && pb.contains("echo \"FIRMWARE=bundled\""));
    let rp = read("board/iyakki/release-profile.sh");
    assert!(rp.contains("echo \"IMAGE_PROFILE=release\"") && rp.contains("echo \"FIRMWARE=none\""));
}

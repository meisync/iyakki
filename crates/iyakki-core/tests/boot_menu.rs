//! The x86 boot menu (GRUB) and the image steps that carry it: kernel command lines, the safe-graphics fallback, the graphical
//! menu reaching both the ISO and the install image, and the Mac variant's kernel patch.

use std::path::{Path, PathBuf};

fn board() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(board().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// (title, kernel command line without the kernel path) of every `menuentry`.
fn entries(cfg: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut title = None;
    for l in cfg.lines().map(str::trim) {
        if let Some(rest) = l.strip_prefix("menuentry \"") {
            title = rest.split('"').next().map(str::to_string);
        } else if let Some(rest) = l.strip_prefix("linux ") {
            let args = rest.split_once(' ').map_or("", |(_, a)| a).to_string();
            out.push((title.take().expect("linux line outside a menuentry"), args));
        }
    }
    out
}

#[test]
fn every_x86_boot_entry_has_the_quiet_flicker_free_command_line() {
    let cfg = read("board/iyakki/grub.cfg");
    let e = entries(&cfg);
    assert!(e.len() >= 7, "{e:?}");
    for (t, args) in &e {
        let a: Vec<&str> = args.split_whitespace().collect();
        assert!(a.contains(&"console=tty0"), "{t}");
        assert!(!args.contains("i915.fastboot"), "{t}: Linux 6.12 has no i915.fastboot (it only logs 'unknown parameter')");
        if a.contains(&"quiet") {
            assert!(a.contains(&"loglevel=3") && a.contains(&"vt.global_cursor_default=0"), "{t}: quiet entries hide text and the cursor");
        }
    }
    let normal = &e.iter().find(|(t, _)| t == "IYAKKI").expect("the normal entry").1;
    assert!(normal.ends_with("quiet loglevel=3 vt.global_cursor_default=0"), "{normal}");
    let safe = &e.iter().find(|(t, _)| t.contains("safe graphics")).expect("a safe graphics entry").1;
    assert!(safe.contains("nomodeset") && safe.contains("modprobe.blacklist=i915,"), "{safe}");
    assert!(e.iter().any(|(t, a)| t == "IYAKKI - remote diagnostics (SSH, keys only)" && a.contains("iyakki.ssh=1")));
    assert!(e.iter().any(|(t, a)| t == "IYAKKI - show boot messages" && !a.contains("quiet")), "one entry shows the kernel's text");
    // the graphical menu comes before the first entry and keeps the screen mode for the kernel
    let first = cfg.find("menuentry").unwrap();
    for want in ["set gfxpayload=keep", "insmod gfxterm", "themes/iyakki/theme.txt", "iyakki-bg.png"] {
        assert!(cfg[..first].contains(want), "{want}");
    }
}

#[test]
fn the_new_menu_reaches_both_the_iso_and_the_install_image() {
    let s = read("board/iyakki/post-image.sh");
    let gen = s.find("\"$BOARD/grub.cfg\" > \"$TREE/boot/grub/grub.cfg\"").expect("post-image writes the board's grub.cfg into the ISO tree");
    assert!(s[..gen].contains("s%__KERNEL_PATH__%/boot/bzImage%") && s[..gen].contains("s%__INITRD_PATH__%/boot/initrd%"));
    assert!(gen < s.find("xorriso").unwrap(), "before the ISO is made");
    let copy = s.find("cp \"$TREE/boot/grub/grub.cfg\" \"$ROOT/boot/grub/grub.cfg\"").expect("the install template gets the same file");
    assert!(gen < copy);
    assert!(s.contains("cp -r \"$TREE/boot/grub/fonts\" \"$TREE/boot/grub/themes\" \"$ROOT/boot/grub/\""), "and the theme and font");
    assert!(s.contains("lacks '$want'") && s.contains("set gfxpayload=keep") && s.contains("IYAKKI - remote diagnostics"), "a stale menu stops the build");
    // GRUB itself is rebuilt when its module list changes (Buildroot would keep an old build without the video modules)
    let b = read("build.sh");
    assert!(b.contains("rebuild_if_changed grub2") && b.contains("rebuild_if_changed linux"));
}

#[test]
fn the_mac_variant_carries_the_cdclk_patch_for_the_defconfig_kernel() {
    let def = read("configs/iyakki_x86_64_defconfig");
    let ver = def.lines().find_map(|l| l.strip_prefix("BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE=")).unwrap().trim_matches('"').to_string();
    let mac = read("configs/iyakki_mac.fragment");
    assert!(mac.contains("BR2_GLOBAL_PATCH_DIR=\"$(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/patches $(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/patches-sb\""));
    assert!(def.contains("BR2_GLOBAL_PATCH_DIR=\"$(BR2_EXTERNAL_IYAKKI_PATH)/board/iyakki/patches-sb\""), "the default images carry only the Secure Boot lockdown patch");
    assert!(!def.contains("board/iyakki/patches\""), "the default images do not carry the Mac patches");
    let dir = board().join(format!("board/iyakki/patches/linux/{ver}"));
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap_or_else(|_| panic!("no patches for kernel {ver} in {}", dir.display())).flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    // 0001 is IYAKKI's own (Mac mini 2018); 0002-0004 are small, pinned t2linux / upstream backports for T2 MacBooks
    assert_eq!(names, [
        "0001-drm-i915-skl-keep-the-firmware-cdclk-while-pipes-are-on.patch",
        "0002-HID-apple-touch-bar-backlight-and-keyboard-mode-backport.patch",
        "0003-Input-bcm5974-add-support-for-the-T2-Macs.patch",
        "0004-drm-amd-pm-fix-boot-of-the-Radeon-Pro-5300M-in-MacBookPro16-1.patch",
    ]);
    let boundary = std::fs::read_to_string(board().join("../../BOUNDARY.md")).unwrap();
    for n in &names {
        assert!(boundary.contains(n.as_str()), "BOUNDARY.md rule 3 lists {n}");
    }
    for n in &names[1..] {
        let p = std::fs::read_to_string(dir.join(n)).unwrap();
        assert!(p.contains("Source") && p.contains("GPL-2.0") && p.contains("github.com/"), "{n} records where it comes from and its licence");
        assert!(p.contains("commit ") || p.contains("/tree/v6.18"), "{n} is pinned");
    }
    let mac_frag = read("board/iyakki/linux-mac.fragment");
    for opt in ["CONFIG_HID_APPLETB_KBD=m", "CONFIG_HID_APPLETB_BL=m", "CONFIG_MOUSE_BCM5974=m", "CONFIG_BT_HCIBCM4377=m"] {
        assert!(mac_frag.contains(opt), "the Mac kernel fragment enables {opt}");
    }
    let p = std::fs::read_to_string(dir.join(&names[0])).unwrap();
    assert!(p.contains("+++ b/drivers/gpu/drm/i915/display/intel_cdclk.c"));
    assert_eq!(p.matches("+++ ").count(), 1, "one file");
    let added = p.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
    let removed = p.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
    assert!(added <= 20 && removed == 0, "small and only adds a check: +{added} -{removed}");
    assert!(p.contains("cdclk_state->active_pipes") && p.contains("cdclk = cdclk_state->logical.cdclk;"));
}

/// GRUB's core image has only the modules it was built with: a command of grub.cfg whose module is missing prints
/// "can't find command" at boot (seen on a Mac: three `echo` errors and a longer wait). Every command the menu uses must have
/// its module in both lists (BIOS and EFI).
#[test]
fn the_grub_modules_cover_every_command_the_menu_uses() {
    let cfg = read("board/iyakki/grub.cfg");
    let def = read("configs/iyakki_x86_64_defconfig");
    let list = |key: &str| -> Vec<String> {
        let line = def.lines().find(|l| l.starts_with(key)).unwrap_or_else(|| panic!("{key} missing"));
        line.split('"').nth(1).unwrap().split_whitespace().map(str::to_string).collect()
    };
    let (pc, efi) = (list("BR2_TARGET_GRUB2_BUILTIN_MODULES_PC="), list("BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI="));
    // (the command as a word at the start of a line or after a space, the module that provides it)
    for (cmd, module) in [("echo", "echo"), ("loadfont", "font"), ("background_image", "gfxterm_background"), ("terminal_output", "gfxterm"), ("search", "search")] {
        let used = cfg.lines().map(str::trim).any(|l| l == cmd || l.starts_with(&format!("{cmd} ")) || l.contains(&format!("; {cmd} ")));
        if used {
            for (name, mods) in [("PC", &pc), ("EFI", &efi)] {
                assert!(mods.iter().any(|m| m == module), "grub.cfg uses `{cmd}` but the {name} module list has no `{module}`");
            }
        }
    }
}

#[test]
fn every_boot_entry_says_it_is_loading() {
    let cfg = read("board/iyakki/grub.cfg");
    assert!(cfg.contains("function iyakki_loading") && cfg.contains("Loading IYAKKI from USB"));
    let entries = cfg.matches("menuentry \"").count();
    assert_eq!(cfg.matches("\tiyakki_loading\n\tlinux ").count(), entries, "each entry prints the message before GRUB reads the kernel");
}

/// Run board/iyakki/prune-firmware.sh with the Mac keep list on a made-up firmware tree: the files Macs need stay (also the
/// target of a kept link), the rest goes.
#[test]
fn the_mac_firmware_list_keeps_what_macs_need_and_drops_the_rest() {
    let list = read("board/iyakki/mac-firmware.keep");
    let required: Vec<&str> = list.lines().filter_map(|l| l.strip_prefix("# required:")).flat_map(|l| l.split_whitespace()).collect();
    assert!(required.len() >= 15, "the list names the files it must keep");
    let dir = std::env::temp_dir().join(format!("iyakki-fwprune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let fw = dir.join("fw");
    let drop = ["iwlwifi-9260-th-b0-jf-b0-46.ucode", "intel/iwlwifi/iwlwifi-so-a0-gf-a0-89.ucode", "ath12k/WCN7850/hw2.0/amss.bin", "rtw89/rtw8852c_fw.bin",
        "amdgpu/gc_11_0_0_pfp.bin", "amdgpu/yellow_carp_dmcub.bin", "i915/tgl_dmc_ver2_12.bin", "bnx2x/bnx2x-e2-7.13.21.0.fw", "nvidia/gm200/acr/bl.bin"];
    for f in required.iter().chain(drop.iter()) {
        let p = fw.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
    }
    // a kept link keeps its target even when no pattern names the target
    std::fs::write(fw.join("target-only.fw"), b"y").unwrap();
    std::os::unix::fs::symlink("target-only.fw", fw.join("mt7601u.bin.link")).unwrap();
    let keep_copy = dir.join("keep");
    std::fs::write(&keep_copy, format!("{list}\nmt7601u.bin.link\n")).unwrap();
    let out = std::process::Command::new("sh").arg(board().join("board/iyakki/prune-firmware.sh")).arg(&keep_copy).arg(&fw).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for f in &required {
        assert!(fw.join(f).exists(), "Mac firmware {f} must stay");
    }
    assert!(fw.join("target-only.fw").exists(), "the target of a kept link stays");
    for f in drop {
        assert!(!fw.join(f).exists(), "{f} must go");
    }
    let _ = std::fs::remove_dir_all(&dir);
    // wired into the Mac variants only
    let pb = read("board/iyakki/post-build.sh");
    assert!(pb.contains("mac-*|macweb-*)") && pb.contains("prune-firmware.sh"));
}

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
    assert!(e.len() >= 9, "{e:?}");
    for (t, args) in &e {
        let a: Vec<&str> = args.split_whitespace().collect();
        assert!(a.contains(&"console=tty0"), "{t}");
        assert!(!args.contains("i915.fastboot"), "{t}: Linux 6.12 has no i915.fastboot (it only logs 'unknown parameter')");
        if a.contains(&"quiet") {
            assert!(a.contains(&"loglevel=3") && a.contains(&"vt.global_cursor_default=0"), "{t}: quiet entries hide text and the cursor");
        }
    }
    let normal = &e.iter().find(|(t, _)| t == "IYAKKI").expect("the normal entry").1;
    assert!(normal.contains("quiet loglevel=3 vt.global_cursor_default=0"), "{normal}");
    let safe = &e.iter().find(|(t, _)| t.contains("safe graphics")).expect("a safe graphics entry").1;
    assert!(safe.contains("nomodeset") && safe.contains("modprobe.blacklist=i915,"), "{safe}");
    assert!(e.iter().any(|(t, a)| t == "IYAKKI - remote diagnostics (SSH, keys only)" && a.contains("iyakki.ssh=1")));
    assert!(e.iter().any(|(t, a)| t == "IYAKKI - show boot messages" && !a.contains("quiet")), "one entry shows the kernel's text");
    // install mode: boots the live system and runs the installer's pages (PID 1 reads iyakki.install=1)
    let inst = e.iter().find(|(t, _)| t == "IYAKKI - Install to a disk").expect("the install entry");
    assert!(inst.1.split_whitespace().any(|a| a == "iyakki.install=1"), "{}", inst.1);
    assert!(e.iter().filter(|(_, a)| a.contains("iyakki.install=1")).count() == 1, "only the install entry starts the installer");
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
    for (cmd, module) in [("echo", "echo"), ("loadfont", "font"), ("background_image", "gfxterm_background"), ("terminal_output", "gfxterm"), ("search", "search"), ("chainloader", "chain")] {
        let used = cfg.lines().map(str::trim).any(|l| l == cmd || l.starts_with(&format!("{cmd} ")) || l.contains(&format!("; {cmd} ")));
        if used {
            // the new boot loader's entry is UEFI only (hidden on BIOS), so `chain` is needed in the EFI build only
            for (name, mods) in [("PC", &pc), ("EFI", &efi)] {
                if cmd == "chainloader" && name == "PC" {
                    continue;
                }
                assert!(mods.iter().any(|m| m == module), "grub.cfg uses `{cmd}` but the {name} module list has no `{module}`");
            }
        }
    }
}

#[test]
fn every_boot_entry_says_it_is_loading() {
    let cfg = read("board/iyakki/grub.cfg");
    assert!(cfg.contains("function iyakki_loading") && cfg.contains("IYAKKI is starting") && cfg.contains("[1/3] Reading the kernel"));
    // the chain-loading entry of the new boot loader starts no kernel through GRUB, so it has no loading text
    let entries = cfg.matches("menuentry \"").count() - cfg.matches("chainloader ").count();
    let loading = cfg.lines().map(str::trim).collect::<Vec<_>>().windows(2).filter(|w| w[0] == "iyakki_loading" && w[1].starts_with("linux ")).count();
    assert_eq!(loading, entries, "each entry prints the message before GRUB reads the kernel");
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

/// `iyakki dev update` keeps the version it replaces on the DATA partition (ext4, label IYAKKI-DATA) and adds a menu entry that
/// loads it from there: GRUB must be able to find the volume by its label and read ext4 in both the BIOS and the EFI build, and
/// the entry is copied from the normal one, which must therefore exist with a `linux` and an `initrd` line.
#[test]
fn the_previous_version_entry_of_a_system_update_needs_only_what_the_grub_build_has() {
    let def = read("configs/iyakki_x86_64_defconfig");
    let list = |key: &str| -> Vec<String> { def.lines().find(|l| l.starts_with(key)).unwrap().split('"').nth(1).unwrap().split_whitespace().map(str::to_string).collect() };
    for (name, mods) in [("PC", list("BR2_TARGET_GRUB2_BUILTIN_MODULES_PC=")), ("EFI", list("BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI="))] {
        for m in ["search", "search_label", "ext2", "linux", "echo", "part_gpt", "part_msdos"] {
            assert!(mods.iter().any(|x| x == m), "the {name} GRUB build lacks `{m}`, which the previous-version entry of a system update needs");
        }
    }
    let cfg = read("board/iyakki/grub.cfg");
    let e = entries(&cfg);
    let normal = e.iter().find(|(t, _)| t == "IYAKKI").expect("the normal entry");
    assert!(normal.1.contains("quiet") && cfg.contains("linux __KERNEL_PATH__ console=tty0") && cfg.contains("initrd __INITRD_PATH__"), "{}", normal.1);
    // the image says which IYAKKI its boot files are, for `iyakki dev update-pack` and the next update
    assert!(read("board/iyakki/post-image.sh").contains("boot/iyakki-release"));
}

/// The compatible USB mode (no UAS, docs/STORAGE-LOSS.md): an optional entry, and a variable every entry reads that is empty unless
/// Settings wrote the file on the boot partition, so nothing changes by default.
#[test]
fn the_compatible_usb_mode_entry_and_file_are_in_the_menu_and_change_nothing_by_default() {
    let cfg = read("board/iyakki/grub.cfg");
    let e = entries(&cfg);
    let compat = e.iter().find(|(t, _)| t == "IYAKKI - compatible USB mode (no UAS)").expect("the optional entry");
    assert!(compat.1.contains("usb-storage.quirks=04e8:4001:u") && compat.1.contains("quiet"), "{}", compat.1);
    // only that entry names a quirk itself; the default entry does not
    assert_eq!(e.iter().filter(|(_, a)| a.contains("usb-storage.quirks=")).count(), 1, "only the compatible entry names a quirk");
    let first = cfg.find("menuentry").unwrap();
    let head = &cfg[..first];
    assert!(head.contains("set iyakki_usbq=\"\"") && head.contains("source $prefix/iyakki-usb-compat.cfg"), "the variable is empty unless the file exists");
    assert!(head.find("set iyakki_usbq=\"\"").unwrap() < head.find("source $prefix/iyakki-usb-compat.cfg").unwrap());
    // every other entry reads the variable, right after the kernel path (the rest of each command line is unchanged)
    for (t, a) in &e {
        if !t.contains("compatible USB mode (no UAS)") && !t.contains("trouble 5") {
            assert!(a.starts_with("$iyakki_usbq console=tty0"), "{t}: {a}");
        }
    }
    // the file Settings writes is the one GRUB sources
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-storage/src/loss.rs")).unwrap();
    assert!(src.contains("pub const COMPAT_FILE: &str = \"iyakki-usb-compat.cfg\"") && src.contains("set iyakki_usbq="));
}

/// The fast start (lean boot, docs/BOOT-SPEED.md): the one first-screen entry, "IYAKKI", reads the small RAM disk when `boot/initrd-lean` is on the
/// volume and the full one otherwise; the first stage's failure marker makes every entry read the full RAM disk again (when the stick has it). "full start" under
/// More options always reads the full one.
#[test]
fn the_fast_start_entries_come_first_when_installed_and_the_full_boot_is_the_fallback() {
    let cfg = read("board/iyakki/grub.cfg");
    assert!(cfg.contains("set default=\"0\""), "the first entry is the default");
    let e = entries(&cfg);
    assert_eq!(e[0].0, "IYAKKI", "the first and default entry is the plain IYAKKI");
    assert!(e[0].1.starts_with("$iyakki_usbq console=tty0") && e[0].1.contains("quiet") && !e[0].1.contains("iyakki.install"));
    // the block chooses the RAM disk once; every entry but the full start reads it
    let guard = cfg.find("if [ -f __INITRD_PATH__-lean ]; then").expect("the small RAM disk is used only when it is on the volume");
    let end = cfg[guard..].find("\nfi\n").expect("closed") + guard;
    let block = &cfg[guard..end];
    assert!(block.contains("set iyakki_initrd=__INITRD_PATH__-lean"), "{block}");
    assert!(cfg.contains("set iyakki_initrd=__INITRD_PATH__\n"), "the full RAM disk is the default");
    assert_eq!(block.matches("menuentry \"").count(), 0);
    let full_start = cfg.find("menuentry \"IYAKKI - full start").expect("the full start entry");
    let sub = cfg.find("submenu \"More options...\" {").unwrap();
    assert!(sub < full_start, "the full start is under More options");
    let (first_initrd, entry_end) = (cfg.find("initrd $iyakki_initrd").unwrap(), cfg[full_start..].find("\n\t}\n").unwrap() + full_start);
    assert!(first_initrd < full_start && cfg[full_start..entry_end].contains("initrd __INITRD_PATH__\n"), "the full start reads the full RAM disk");
    assert_eq!(cfg.matches("initrd __INITRD_PATH__\n").count(), 1, "only the full start names the full RAM disk directly");
    // the failure marker of the first stage (crates/iyakki-stage1) puts the full RAM disk back
    assert!(block.contains("[ -f $prefix/iyakki-lean-failed ]") && block.contains("if [ -f __INITRD_PATH__ ]; then"), "a failed fast start reads the full RAM disk only when the volume has it");
    let stage1 = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-stage1/src/main.rs")).unwrap();
    assert!(stage1.contains("/s/boot/boot/grub/iyakki-lean-failed"), "the first stage writes the file GRUB looks for");
    // the choice sits between the markers the install scripts use, in front of the first entry
    let (b1, b2, n) = (cfg.find("# BEGIN lean-boot").unwrap(), cfg.find("# END lean-boot").unwrap(), cfg.find("menuentry \"IYAKKI\" {").unwrap());
    assert!(b1 < guard && guard < b2 && b2 < n);
    assert!(cfg[..guard].contains("insmod test"), "`[ -f` needs GRUB's test module, loaded before");
    // the paths are filled in the same way as the others (post-image.sh replaces __INITRD_PATH__, the suffix stays)
    let filled = cfg.replace("__KERNEL_PATH__", "/boot/bzImage").replace("__INITRD_PATH__", "/boot/initrd");
    assert!(filled.contains("set iyakki_initrd=/boot/initrd-lean") && filled.contains("[ -f /boot/initrd-lean ]"));
    // the installer script and the first stage agree on the names
    let sh = read("lean-install.sh");
    assert!(sh.contains("initrd-lean") && sh.contains("IYAKKI-BOOT") && sh.contains("IYAKKI-DATA") && sh.contains("iyakki/root"));
}

/// The first screen stays short: the rarely needed starts are in one submenu.
#[test]
fn the_first_menu_screen_is_short_and_the_rare_starts_are_in_one_submenu() {
    let cfg = read("board/iyakki/grub.cfg");
    let sub = cfg.find("submenu \"More options...\" {").expect("the submenu");
    assert_eq!(cfg.matches("submenu \"").count(), 1);
    let top: Vec<&str> = cfg[..sub].lines().filter_map(|l| l.strip_prefix("menuentry \"")).filter_map(|r| r.split('"').next()).collect();
    assert_eq!(top, ["IYAKKI"], "one entry on the first screen; the fast start is what it starts when it is on the stick");
    let rest = &cfg[sub..];
    for t in ["IYAKKI - Install to a disk", "IYAKKI - safe graphics", "IYAKKI - full start", "IYAKKI - remote diagnostics (SSH, keys only)", "IYAKKI - compatible USB mode (no UAS)", "IYAKKI - show boot messages", "IYAKKI - hardware check", "IYAKKI (verbose, boot self-test, power off)", "IYAKKI - NVIDIA driver", "IYAKKI - also in a browser", "IYAKKI - touch screen"] {
        assert!(rest.contains(&format!("menuentry \"{t}")), "{t} belongs in the submenu");
        assert!(!cfg[..sub].contains(&format!("menuentry \"{t}")), "{t} is not on the first screen");
    }
    // a bigger menu font is applied by the image step when it can build one; the theme keeps the working default otherwise
    let post = read("board/iyakki/post-image.sh");
    assert!(post.contains("grub-mkfont") && post.contains("iyakki-menu.pf2"));
    assert!(cfg.contains("loadfont $prefix/fonts/iyakki-menu.pf2") && cfg.contains("loadfont $prefix/fonts/iyakki-term.pf2"));
    // GRUB finds fonts by the name inside the .pf2; "Unifont Regular 16" is not a match for "GNU Unifont Regular 16" and GRUB then uses
    // the font loaded last (big letters on a narrow grid: the gap-between-letters bug), so the theme must use the full name
    let theme = read("board/iyakki/grub/theme.txt");
    assert!(!theme.contains("\"Unifont Regular 16\"") && theme.contains("terminal-font: \"GNU Unifont Regular 16\""));
    assert!(post.contains("DejaVu Sans Mono Regular 24") && post.contains("--no-hinting"));
}

#[test]
fn the_lean_install_script_refuses_wrong_drives_and_copies_safely() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-leantest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::process::Command::new("bash").arg(board().join("lean-install-test.sh")).env("TMPDIR", &dir).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("all lean-install tests passed"));
}

/// Crash behaviour of every entry: a kernel panic restarts the machine after 20 seconds (a hung blank screen after a panic helps nobody on a
/// desktop that boots from a stick); no `oops=panic` (an oops in one driver, say sound, should not take the session down), no softlockup or
/// hung-task panics (they would turn a slow disk into a reboot). docs/TROUBLESHOOTING.md explains the choice.
#[test]
fn every_entry_restarts_after_a_kernel_panic_and_nothing_stricter() {
    let cfg = read("board/iyakki/grub.cfg");
    let e = entries(&cfg);
    assert!(e.len() >= 12);
    for (t, a) in &e {
        let w: Vec<&str> = a.split_whitespace().collect();
        if t.contains("trouble 5") {
            // the one entry that stays on the screen after a panic, so the message can be read
            assert!(w.contains(&"panic=0"), "{t}: {a}");
            continue;
        }
        assert!(w.contains(&"panic=20"), "{t}: {a}");
        assert!(!a.contains("oops=panic") && !a.contains("softlockup_panic") && !a.contains("hung_task_panic"), "{t}");
    }
}

/// The extra boot loader (iyakki-boot): a GRUB test entry that chain-loads it on UEFI only, shown only when the image carries it, and the
/// image step that lays it out with the same kernel, RAM disk and command lines as the plain GRUB entries. GRUB stays the default.
#[test]
fn the_new_boot_loader_is_an_extra_test_entry_and_grub_stays_the_default() {
    let cfg = read("board/iyakki/grub.cfg");
    assert!(cfg.contains("set default=\"0\""), "GRUB's own first entry stays the default");
    let sub = cfg.find("submenu \"More options...\"").unwrap();
    let at = cfg.find("menuentry \"IYAKKI - new boot loader (test)\"").expect("the entry");
    assert!(at > sub, "inside the test submenu, not on the first screen");
    let block = &cfg[sub..at];
    assert!(block.contains("$grub_platform\" = efi") && block.contains("-f /EFI/iyakki-boot/loader.efi"), "UEFI only, and only when the loader is on the volume");
    assert!(cfg[at..].lines().nth(1).unwrap().trim() == "chainloader /EFI/iyakki-boot/loader.efi");
    let stage = read("boot-loader-stage.sh");
    let post = read("board/iyakki/post-image.sh");
    let lean = read("lean-image.sh");
    assert!(post.contains("boot-loader-stage.sh") && lean.contains("boot-loader-stage.sh"), "both the install template and the lean image get it");
    // the loader's entries mirror the GRUB ones: see every_grub_entry_has_a_loader_twin_or_is_listed_as_grub_only_with_a_reason
    assert!(stage.contains("kernel /boot/bzImage") && stage.contains("EFI/iyakki-boot") && stage.contains("sign.sh"));
}

/// GRUB entries that the boot loader's menu does not carry, with the reason. Empty today: every start has a twin.
const GRUB_ONLY: &[(&str, &str)] = &[];

/// Rows of the ENTRIES table of `boot-loader-stage.sh`: (id, level, title, kernel command line).
fn loader_table() -> Vec<(String, String, String, String)> {
    let script = read("boot-loader-stage.sh");
    let body = script.split("ENTRIES='\n").nth(1).expect("the ENTRIES table").split("\n'\n").next().unwrap();
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.splitn(4, '|').collect();
            assert_eq!(f.len(), 4, "table row {l:?}");
            (f[0].into(), f[1].into(), f[2].into(), f[3].into())
        })
        .collect()
}

#[test]
fn every_grub_entry_has_a_loader_twin_or_is_listed_as_grub_only_with_a_reason() {
    let grub = entries(&read("board/iyakki/grub.cfg"));
    let table = loader_table();
    for (title, args) in &grub {
        let a = args.replace("$iyakki_usbq ", "");
        let twin = table.iter().find(|t| t.3 == a);
        let only = GRUB_ONLY.iter().find(|(t, _)| t == title);
        assert!(twin.is_some() || only.is_some_and(|(_, why)| !why.is_empty()), "GRUB entry {title:?} has no twin in the ENTRIES table of boot-loader-stage.sh and is not in GRUB_ONLY: {a}");
        assert!(!(twin.is_some() && only.is_some()), "{title:?} is both twinned and GRUB-only");
    }
    for (t, _) in GRUB_ONLY {
        assert!(grub.iter().any(|(g, _)| g == t), "GRUB_ONLY names {t:?}, which is not in grub.cfg any more");
    }
    // and the other way: nothing in the loader's table that GRUB does not start
    for (id, _, title, cmd) in &table {
        assert!(grub.iter().any(|(_, a)| a.replace("$iyakki_usbq ", "") == *cmd), "loader entry {id} ({title}) has no GRUB twin: {cmd}");
    }
}

#[test]
fn the_loader_table_makes_a_config_the_loader_accepts() {
    let table = loader_table();
    let mut ids = std::collections::HashSet::new();
    for (id, level, title, cmd) in &table {
        assert!(ids.insert(id.clone()), "{id} twice");
        assert!(!id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\'), "{id}");
        assert!(matches!(level.as_str(), "top" | "more"), "{id}: level {level}");
        assert!(!title.is_empty() && title.len() <= 64, "{id}: title");
        assert!(cmd.bytes().all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\') && cmd.contains("console=tty0"), "{id}: command line");
    }
    // owner 2026-11-05: the first level is IYAKKI and "Install to a disk" (which the loader shows only on install media: it requires the marker file)
    let top: Vec<&str> = table.iter().filter(|t| t.1 == "top").map(|t| t.0.as_str()).collect();
    assert_eq!(top, vec!["iyakki", "install"], "the first level: IYAKKI, then Install to a disk");
    assert_eq!(table[0].0, "iyakki", "the default start is the first row");
    // run the real script with a stand-in loader file and read what it wrote (python3 is already needed by the image steps)
    let tmp = std::env::temp_dir().join(format!("iyakki-loader-stage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("root")).unwrap();
    // the stage script refuses a loader that was not built with the config path of the image: the stand-in carries the path like the real one
    std::fs::write(tmp.join("fake.efi"), b"MZ /EFI/iyakki-boot/boot.cfg").unwrap();
    for (initrd, full) in [("/boot/initrd", false), ("/boot/initrd-lean", true)] {
        let out = std::process::Command::new("sh")
            .arg(board().join("boot-loader-stage.sh"))
            .arg(tmp.join("root"))
            .arg(initrd)
            .env("IYAKKI_BOOT_EFI", tmp.join("fake.efi"))
            .output()
            .expect("sh");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let cfg = std::fs::read_to_string(tmp.join("root/EFI/iyakki-boot/boot.cfg")).unwrap();
        assert!(cfg.len() < 32 * 1024, "{} bytes: over the loader's file limit", cfg.len());
        let n = cfg.lines().filter(|l| l.starts_with("entry ")).count();
        assert!(n <= 48 && n >= table.len() + 3, "{n} entries");
        for (id, level, ..) in &table {
            assert!(cfg.contains(&format!("\nentry {id}\n")), "{id} missing from boot.cfg");
            let block = cfg.split(&format!("\nentry {id}\n")).nth(1).unwrap().split("\nentry ").next().unwrap();
            assert_eq!(block.contains("\n  more\n"), level == "more", "{id}: level");
            assert!(block.contains(&format!("initrd {initrd}\n")), "{id}: RAM disk");
        }
        assert_eq!(cfg.contains("\nentry full\n"), full, "the full-start entry is only for a lean image");
        assert_eq!(cfg.matches("\n  default\n").count(), 1);
        for (id, file) in [("previous", "/EFI/iyakki-boot/loader.prev.efi"), ("recovery", "/EFI/BOOT/grub-recovery.efi")] {
            let block = cfg.split(&format!("\nentry {id}\n")).nth(1).expect(id);
            assert!(block.contains(&format!("requires {file}\n")), "{id} is shown only when its file exists");
        }
        assert!(cfg.contains("kind firmware"), "UEFI firmware setup entry");
        assert!(cfg.contains("splash /EFI/iyakki-boot/splash.bmp") && cfg.contains("\nscan yes\n"));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `IYAKKI_LOADER_DEFAULT=1` (docs/BOOT-LOADER.md): GRUB-default builds keep the "new boot loader (test)" entry, loader-default builds drop it
/// and lay the loader out as shim's second stage / BOOTX64.EFI with GRUB as grub-recovery.efi, consistent with the loader's Recovery row.
#[test]
fn the_loader_default_option_drops_the_test_entry_and_lays_out_the_recovery_names_and_grub_default_keeps_the_entry() {
    let post = read("board/iyakki/post-image.sh");
    let lean = read("lean-image.sh");
    for (name, text) in [("post-image.sh", &post), ("lean-image.sh", &lean)] {
        assert!(text.contains("IYAKKI_LOADER_DEFAULT:-0") && text.contains("loader-default.sh"), "{name}: the option (default 0)");
    }
    // GRUB-default (the option off): the entry is in the board's grub.cfg, between the markers the script cuts out
    let cfg = read("board/iyakki/grub.cfg");
    assert!(cfg.contains("# BEGIN loader-test-entry") && cfg.contains("# END loader-test-entry"));
    assert!(cfg.find("# BEGIN loader-test-entry").unwrap() < cfg.find("new boot loader (test)").unwrap() && cfg.find("new boot loader (test)").unwrap() < cfg.find("# END loader-test-entry").unwrap());
    let tmp = std::env::temp_dir().join(format!("iyakki-loader-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let sh = |args: &[&str], envs: &[(&str, &Path)]| {
        let mut c = std::process::Command::new("sh");
        c.arg(board().join(args[0])).args(&args[1..]).env_remove("IYAKKI_SB_KEY").env_remove("IYAKKI_SB_CERT");
        for (k, v) in envs {
            c.env(k, v);
        }
        c.output().expect("sh")
    };
    // loader-default: the entry is gone, every other entry and the rest of the file are untouched
    let f = tmp.join("grub.cfg");
    std::fs::write(&f, &cfg).unwrap();
    let out = sh(&["loader-default.sh", "grubcfg", f.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let dropped = std::fs::read_to_string(&f).unwrap();
    assert!(!dropped.contains("new boot loader (test)") && !dropped.contains("chainloader /EFI/iyakki-boot/loader.efi") && !dropped.contains("loader-test-entry"));
    assert_eq!(entries(&dropped), entries(&cfg), "all the starts stay");
    assert_eq!(dropped.matches("menuentry \"").count() + 1, cfg.matches("menuentry \"").count(), "exactly the one test entry went");
    // the layout, both paths. A stand-in loader staged by the real boot-loader-stage.sh (so boot.cfg has the real Recovery row).
    std::fs::write(tmp.join("loader.efi"), b"MZ-LOADER /EFI/iyakki-boot/boot.cfg").unwrap();
    for (path, grub_file, second) in [("shim", "grubx64.efi", "grubx64.efi"), ("plain", "bootx64.efi", "BOOTX64.EFI")] {
        let root = tmp.join(path);
        std::fs::create_dir_all(root.join("EFI/BOOT")).unwrap();
        std::fs::write(root.join("EFI/BOOT").join(grub_file), b"MZ-GRUB").unwrap();
        if path == "shim" {
            std::fs::write(root.join("EFI/BOOT/BOOTX64.EFI"), b"MZ-SHIM").unwrap();
        }
        let out = sh(&["boot-loader-stage.sh", root.to_str().unwrap(), "/boot/initrd"], &[("IYAKKI_BOOT_EFI", &tmp.join("loader.efi"))]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let out = sh(&["loader-default.sh", "esp", root.to_str().unwrap()], &[]);
        assert!(out.status.success(), "{path}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("UNSIGNED"), "{path}: no key configured: the message says the loader is unsigned");
        let b = root.join("EFI/BOOT");
        assert_eq!(std::fs::read(b.join(second)).unwrap(), b"MZ-LOADER /EFI/iyakki-boot/boot.cfg", "{path}: the loader is started by name");
        assert_eq!(std::fs::read(b.join("grub-recovery.efi")).unwrap(), b"MZ-GRUB", "{path}: GRUB is kept as the Recovery file");
        assert_eq!(std::fs::read(root.join("EFI/iyakki-boot/loader.efi")).unwrap(), b"MZ-LOADER /EFI/iyakki-boot/boot.cfg", "{path}: the updater's copy is the same");
        if path == "shim" {
            assert_eq!(std::fs::read(b.join("BOOTX64.EFI")).unwrap(), b"MZ-SHIM", "shim is untouched");
        } else {
            assert!(!b.join("bootx64.efi").exists(), "plain: only one BOOTX64.EFI");
        }
        let cfg = std::fs::read_to_string(root.join("EFI/iyakki-boot/boot.cfg")).unwrap();
        assert!(cfg.contains("entry recovery\n") && cfg.contains("kernel /EFI/BOOT/grub-recovery.efi\n"), "the Recovery row names the file the script made");
        // a second run changes nothing (lean-image.sh runs it after the template did)
        let again = sh(&["loader-default.sh", "esp", root.to_str().unwrap()], &[]);
        assert!(again.status.success());
        assert_eq!(std::fs::read(b.join("grub-recovery.efi")).unwrap(), b"MZ-GRUB");
    }
    // no loader staged: the script says so and leaves the layout alone (GRUB stays the default)
    let empty = tmp.join("empty");
    std::fs::create_dir_all(empty.join("EFI/BOOT")).unwrap();
    std::fs::write(empty.join("EFI/BOOT/bootx64.efi"), b"MZ-GRUB").unwrap();
    let out = sh(&["loader-default.sh", "esp", empty.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(3));
    assert!(empty.join("EFI/BOOT/bootx64.efi").exists() && !empty.join("EFI/BOOT/grub-recovery.efi").exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

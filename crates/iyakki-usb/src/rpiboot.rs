//! The Raspberry Pi 5 boot volume (F-ARM-10): the small text files the Pi firmware reads, the partition table of the SD card
//! image, the list of files the boot volume must hold, and the plan for a trial boot of an update (`tryboot`).
//!
//! A Pi 5 has no UEFI and no GRUB here: the board's EEPROM reads `config.txt` from the first FAT partition and starts the kernel
//! with the RAM disk named there. Stage 1 (`iyakki-stage1`) then does the work GRUB's menu did (docs/development/RASPBERRY-PI-PLAN.md
//! section 1.3). Everything in this file is pure text and byte arithmetic, tested without a board; `iyakki-usb rpi files` writes the
//! texts for the image scripts and `iyakki-usb` writes the finished image to the card like any install template (the disk signature
//! below is the template's, so the writer stretches the data partition to fill the card and verifies what it wrote).
//!
//! What is NOT here, on purpose: any Raspberry Pi firmware blob (a Pi 5 needs none on the card; the EEPROM is never written by IYAKKI),
//! and any claim that it boots: nothing here was started on a real board (the owner's first-boot checklist is
//! `docs/BOARD-RPI5-FIRST-BOOT.md`).

/// The boards this module can lay out. The Pi 3 target ("IYAKKI lite") is a design in the plan only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Board {
    Pi5,
}

impl Board {
    /// The name written to the volume's `board` file and read back by stage 1 (`iyakki_stage1::board_check`).
    pub fn id(self) -> &'static str {
        match self {
            Board::Pi5 => "rpi5",
        }
    }
    pub fn parse(s: &str) -> Result<Board, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pi5" | "rpi5" | "raspberrypi5" => Ok(Board::Pi5),
            other => Err(format!("unknown board '{other}'; the only board laid out so far is pi5 (the Pi 3 target is a design, see docs/development/RASPBERRY-PI-PLAN.md)")),
        }
    }
    /// The device tree the firmware loads.
    pub fn dtb(self) -> &'static str {
        match self {
            Board::Pi5 => "bcm2712-rpi-5-b.dtb",
        }
    }
    /// The KMS display overlay `config.txt` and the tryboot file name. The Pi 5 firmware has no `overlay_map.dtb`, so the plain
    /// `vc4-kms-v3d` name is not found there and the `-pi5` file is the one it loads.
    pub fn display_overlay(self) -> &'static str {
        match self {
            Board::Pi5 => "vc4-kms-v3d-pi5",
        }
    }
    /// Kernel command line words the board needs in every mode. The Pi's default device tree turns the memory cgroup off, which stops k3s.
    pub fn cmdline_extra(self) -> &'static str {
        match self {
            Board::Pi5 => " cgroup_enable=memory cgroup_memory=1",
        }
    }
    /// The kernel file. 4 KiB pages (`kernel8.img` style), not Raspberry Pi OS's 16 KiB `kernel_2712.img`: k3s, container tooling and
    /// some allocators break on 16 KiB pages (plan section 2). The file name is explicit in `config.txt`, so the firmware's own
    /// choice by board never picks another kernel.
    pub fn kernel(self) -> &'static str {
        match self {
            Board::Pi5 => "kernel8.img",
        }
    }
    /// The serial console of the 3-pin debug header of a Pi 5 (not the GPIO header): `ttyAMA10` at 115200 baud (plan section 1.3).
    pub fn serial_console(self) -> &'static str {
        match self {
            Board::Pi5 => "ttyAMA10,115200",
        }
    }
}

/// Which RAM disk the next start reads: the fast one (stage 1, the root comes from the volume) or the full one (the whole system
/// in memory, the fallback after a failed fast start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initrd {
    Lean,
    Full,
}

impl Initrd {
    pub fn file(self) -> &'static str {
        match self {
            Initrd::Lean => "initrd-lean",
            Initrd::Full => "initrd",
        }
    }
}

/// The file `config.txt` ends by including; the only file that changes when the start switches between the lean and the full RAM disk.
pub const BOOT_TXT: &str = "iyakki-boot.txt";
/// The one-line file stage 1 reads to refuse a volume made for another board.
pub const BOARD_FILE: &str = "board";
/// The file the firmware reads instead of `config.txt` for ONE start after `reboot "0 tryboot"` (an update's trial boot).
pub const TRYBOOT_TXT: &str = "tryboot.txt";

/// `iyakki-boot.txt`: one line, the RAM disk. `followkernel` makes the firmware put it right after the kernel in memory.
pub fn iyakki_boot_txt(initrd: Initrd) -> String {
    format!("# IYAKKI: which RAM disk the next start reads (written by the first stage after a failed fast start and by the running system when the fast tree is ready)\ninitramfs {} followkernel\n", initrd.file())
}

/// The `config.txt` of a board. Small on purpose: the firmware's defaults stay, this names our kernel, the device tree, the KMS display
/// overlay and the include.
pub fn config_txt(board: Board) -> String {
    let mut t = String::new();
    t.push_str("# IYAKKI boot configuration for the Raspberry Pi 5. Written by iyakki-usb; an update rewrites it. Do not edit by hand: put your own\n");
    t.push_str("# lines in config-user.txt (included last).\n");
    t.push_str("[all]\n");
    t.push_str("arm_64bit=1\n");
    t.push_str(&format!("kernel={}\n", board.kernel()));
    t.push_str(&format!("device_tree={}\n", board.dtb()));
    // the display: the kernel's vc4 driver drives HDMI (the Pi 5 has no overlay map, so the `-pi5` file name is the one it finds)
    t.push_str(&format!("dtoverlay={}\n", board.display_overlay()));
    t.push_str("max_framebuffers=2\n");
    // the debug UART on the 3-pin header, so a serial adapter shows the firmware and the kernel from the first line
    t.push_str("enable_uart=1\n");
    t.push_str("uart_2ndstage=1\n");
    t.push_str("disable_splash=1\n");
    t.push_str(&format!("include {BOOT_TXT}\n"));
    t.push_str("include config-user.txt\n");
    t
}

/// The modes of the boot menu GRUB gives on a PC. A Pi has no menu at power-on: the running system (or a person with the card in a
/// computer) copies one of the `cmdline-<name>.txt` files over `cmdline.txt` and restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    HwCheck,
    SafeGraphics,
    SelfTest,
    Ssh,
    Web,
    /// The root is taken from the volume's own seed only, ignoring updates on the data volume (`iyakki.root=boot`): the rollback of last resort.
    SeedOnly,
}

impl Mode {
    pub const ALL: [Mode; 7] = [Mode::Normal, Mode::HwCheck, Mode::SafeGraphics, Mode::SelfTest, Mode::Ssh, Mode::Web, Mode::SeedOnly];
    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::HwCheck => "hwcheck",
            Mode::SafeGraphics => "safegraphics",
            Mode::SelfTest => "selftest",
            Mode::Ssh => "ssh",
            Mode::Web => "web",
            Mode::SeedOnly => "seed",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }
    /// The file name of this mode's command line on the volume. Every mode has a `cmdline-<name>.txt`, the normal one too, so a person who
    /// copied another one over `cmdline.txt` can copy `cmdline-normal.txt` back.
    pub fn file(self) -> String {
        format!("cmdline-{}.txt", self.name())
    }
    fn extra(self) -> &'static str {
        match self {
            Mode::Normal => "",
            Mode::HwCheck => " iyakki.hwcheck=1",
            // the safe-graphics entry of the PC menu blacklists the GPU drivers; on a Pi that leaves the firmware's own framebuffer
            Mode::SafeGraphics => " modprobe.blacklist=vc4,v3d",
            Mode::SelfTest => " iyakki.selftest=1",
            Mode::Ssh => " iyakki.ssh=1",
            Mode::Web => " iyakki.web=1",
            Mode::SeedOnly => " iyakki.root=boot",
        }
    }
}

/// One kernel command line (one line, no trailing space). `console=tty1` first, the serial console last: the LAST `console=` becomes
/// /dev/console, so the boot messages reach a serial adapter and the screen.
pub fn cmdline_txt(board: Board, mode: Mode) -> String {
    format!("console=tty1 console={} rootwait panic=10{} iyakki.board={}{}\n", board.serial_console(), board.cmdline_extra(), board.id(), mode.extra())
}

/// The file the firmware reads: the normal command line unless a person or the running system copied another mode's file over it.
pub const CMDLINE_TXT: &str = "cmdline.txt";

/// Every command line file the volume carries: `cmdline.txt` (the normal line) and one `cmdline-<mode>.txt` for every mode.
pub fn cmdline_files(board: Board) -> Vec<(String, String)> {
    let mut v = vec![(CMDLINE_TXT.to_string(), cmdline_txt(board, Mode::Normal))];
    v.extend(Mode::ALL.into_iter().map(|m| (m.file(), cmdline_txt(board, m))));
    v
}

/// The text of the `board` file.
pub fn board_file(board: Board) -> String {
    format!("{}\n", board.id())
}

/// The names of the files an update stages for a trial boot, the `tryboot.txt` that points at them, and the two directions of the
/// switch: promote (the trial boot reached the agent: the staged files become the live ones, the live ones become the rollback
/// copy) and roll back (undo a promotion). Pure data; the running system executes the renames in order and syncs between them.
#[derive(Debug, PartialEq, Eq)]
pub struct Tryboot {
    pub stage: Vec<(&'static str, &'static str)>,
    pub tryboot_txt: String,
    pub promote: Vec<(String, String)>,
    pub rollback: Vec<(String, String)>,
    /// The argument of `reboot` that makes the firmware read `tryboot.txt` once.
    pub reboot_arg: &'static str,
}

/// The files of a staged update: (live name, staged name).
const STAGED: [(&str, &str); 2] = [("kernel8.img", "kernel8-next.img"), ("initrd-lean", "initrd-lean-next")];

pub fn tryboot(board: Board) -> Tryboot {
    let mut t = String::new();
    t.push_str("# IYAKKI trial boot of an update: read ONCE by the firmware after `reboot \"0 tryboot\"`, never by a normal start.\n");
    t.push_str("[all]\narm_64bit=1\n");
    t.push_str(&format!("kernel={}\n", STAGED[0].1));
    t.push_str(&format!("device_tree={}\n", board.dtb()));
    t.push_str(&format!("dtoverlay={}\nmax_framebuffers=2\nenable_uart=1\nuart_2ndstage=1\ndisable_splash=1\n", board.display_overlay()));
    t.push_str(&format!("initramfs {} followkernel\n", STAGED[1].1));
    t.push_str("include config-user.txt\n");
    let mut promote = Vec::new();
    let mut rollback = Vec::new();
    for (live, next) in STAGED {
        // the live file is kept as `<name>.prev` first (rename, atomic on FAT), then the staged one takes its name
        promote.push((live.to_string(), format!("{live}.prev")));
        promote.push((next.to_string(), live.to_string()));
        rollback.push((live.to_string(), next.to_string()));
        rollback.push((format!("{live}.prev"), live.to_string()));
    }
    Tryboot { stage: STAGED.to_vec(), tryboot_txt: t, promote, rollback, reboot_arg: "0 tryboot" }
}

/// What a finished boot volume must hold, by role. The image script fills the paths from the build; `missing` tells which are absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeFile {
    pub path: String,
    /// `true`: the start cannot work without it.
    pub required: bool,
    pub role: &'static str,
}

pub fn volume_files(board: Board) -> Vec<VolumeFile> {
    let mut v = vec![
        VolumeFile { path: "config.txt".into(), required: true, role: "firmware configuration (generated)" },
        VolumeFile { path: board.kernel().into(), required: true, role: "kernel built from the Raspberry Pi tree at the pinned tag" },
        VolumeFile { path: board.dtb().into(), required: true, role: "device tree from the same kernel build" },
        VolumeFile { path: "overlays/vc4-kms-v3d-pi5.dtbo".into(), required: false, role: "display overlay from the same kernel build (headless start works without it)" },
        VolumeFile { path: BOOT_TXT.into(), required: true, role: "which RAM disk (generated)" },
        VolumeFile { path: Initrd::Lean.file().into(), required: true, role: "first-stage RAM disk (stage 1 as /init)" },
        VolumeFile { path: Initrd::Full.file().into(), required: false, role: "the full RAM disk, the fallback after a failed fast start" },
        VolumeFile { path: "boot/root.sfs".into(), required: true, role: "the seed root (squashfs); stage 1 prefers an update on the data volume" },
        VolumeFile { path: BOARD_FILE.into(), required: true, role: "which board this volume is for (generated)" },
    ];
    v.push(VolumeFile { path: CMDLINE_TXT.into(), required: true, role: "kernel command line (generated)" });
    for m in Mode::ALL {
        v.push(VolumeFile { path: m.file(), required: false, role: "a start mode's command line, copied over cmdline.txt to choose it" });
    }
    v
}

/// The required files that `present` (paths on the volume, case does not matter: FAT) lacks.
pub fn missing<'a>(board: Board, present: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let have: Vec<String> = present.into_iter().map(|p| p.trim_start_matches('/').to_ascii_lowercase()).collect();
    volume_files(board).into_iter().filter(|f| f.required && !have.contains(&f.path.to_ascii_lowercase())).map(|f| f.path).collect()
}

/// Files that must NEVER be on an IYAKKI Pi boot volume: the vendor's boot blobs (they come only through the firmware-by-button pack,
/// and a Pi 5 needs none) and EEPROM images (IYAKKI never writes the EEPROM). Returns the offending paths.
pub fn forbidden<'a>(present: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    present
        .into_iter()
        .filter(|p| {
            let n = p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase();
            n == "bootcode.bin" || n.starts_with("start") && n.ends_with(".elf") || n.starts_with("fixup") && n.ends_with(".dat") || n == "pieeprom.bin" || n == "recovery.bin" || n.ends_with(".sig") && n.contains("eeprom") || n.starts_with("pieeprom")
        })
        .map(str::to_string)
        .collect()
}

// ---- the SD card image's partition table -------------------------------------------------------------------------------------

/// The first partition starts here (sectors): 4 MiB, aligned for SD-card erase blocks, with room before it for nothing else (a Pi reads no boot code).
pub const BOOT_START_SECTORS: u64 = 8192;
/// Sectors of the data partition inside the template: a 1 MiB marker `IYAKKI-FORMAT-ME-v1` that IYAKKI formats on first boot (the same marker the x86 template carries).
pub const DATA_MARKER_SECTORS: u64 = 2048;
/// The install template's disk signature (`00 53 46 41`), which is how `iyakki-usb` knows to stretch partition 2 to the card.
pub const TEMPLATE_SIGNATURE: [u8; 4] = [0x00, 0x53, 0x46, 0x41];

/// Size of the FAT32 boot volume in MiB for `payload_bytes` of files: payload plus 25 percent and 32 MiB, at least 256 MiB (plan section 1.4),
/// rounded up to a whole MiB. FAT32 needs at least 65525 clusters, which 256 MiB with 4 KiB clusters gives (65536).
pub fn boot_fat_mib(payload_bytes: u64) -> u64 {
    let want = payload_bytes + payload_bytes / 4 + 32 * 1024 * 1024;
    (want.div_ceil(1024 * 1024)).max(256)
}

/// Sector 0 of the SD card image: an MBR with the template signature, partition 1 FAT32 (LBA, type 0x0C, bootable) of `boot_mib`, and
/// partition 2 (type 0x83) of [`DATA_MARKER_SECTORS`]. MBR, not GPT: one layout for every Pi and for the existing 4Kn logic (plan section 1.4).
pub fn mbr_sector(boot_mib: u64) -> [u8; 512] {
    let mut s = [0u8; 512];
    s[440..444].copy_from_slice(&TEMPLATE_SIGNATURE);
    let boot_sectors = boot_mib * 2048;
    let entry = |s: &mut [u8; 512], i: usize, status: u8, ty: u8, start: u64, len: u64| {
        let e = 446 + 16 * i;
        s[e] = status;
        s[e + 1..e + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]); // CHS is meaningless: "use LBA"
        s[e + 4] = ty;
        s[e + 5..e + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
        s[e + 8..e + 12].copy_from_slice(&(start as u32).to_le_bytes());
        s[e + 12..e + 16].copy_from_slice(&(len as u32).to_le_bytes());
    };
    entry(&mut s, 0, 0x80, 0x0C, BOOT_START_SECTORS, boot_sectors);
    entry(&mut s, 1, 0x00, 0x83, BOOT_START_SECTORS + boot_sectors, DATA_MARKER_SECTORS);
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

/// Where partition 1 and 2 start (sectors) and how long they are, read back from an MBR sector: `[(start, len, type); 2]`.
pub fn read_mbr(sector: &[u8]) -> Result<[(u64, u64, u8); 2], String> {
    if sector.len() < 512 || sector[510] != 0x55 || sector[511] != 0xAA {
        return Err("not a boot sector".into());
    }
    let one = |i: usize| {
        let e = 446 + 16 * i;
        (u32::from_le_bytes([sector[e + 8], sector[e + 9], sector[e + 10], sector[e + 11]]) as u64, u32::from_le_bytes([sector[e + 12], sector[e + 13], sector[e + 14], sector[e + 15]]) as u64, sector[e + 4])
    };
    Ok([one(0), one(1)])
}

/// The size of the whole template image in bytes: the table, the gap, the boot volume and the marker.
pub fn image_bytes(boot_mib: u64) -> u64 {
    (BOOT_START_SECTORS + boot_mib * 2048 + DATA_MARKER_SECTORS) * 512
}

/// The text of the first-boot data marker's first bytes (1 MiB partition starting with it; the rest zero).
pub const DATA_MARKER: &[u8] = b"IYAKKI-FORMAT-ME-v1\0";

// ---- the command `iyakki-usb rpi ...` ----------------------------------------------------------------------------------------

pub const RPI_HELP: &str = "iyakki-usb rpi: the Raspberry Pi boot volume\n\n  rpi files --board pi5 --out DIR     write config.txt, cmdline*.txt, iyakki-boot.txt, board, tryboot.txt into DIR\n  rpi mbr --boot-mib N --out FILE     write the 512-byte table of the SD card image (template signature, FAT32 boot volume, marked data partition)\n  rpi size --payload-bytes N          print the boot volume size in MiB for that much payload\n  rpi check --list FILE               check the file list of a finished boot volume: every required file, no vendor blob\n  rpi firmware list|fetch ...         the vendor firmware packs (downloaded only at your click, licence shown first)\n\nWriting the finished image to a card is the normal path: iyakki-usb --image iyakki-rpi5.img --erase sdX\n";

/// Runs `iyakki-usb rpi ...` for the sub-commands that need no network. Returns the text to print.
pub fn run_cli(args: &[String], write: &mut dyn FnMut(&std::path::Path, &[u8]) -> Result<(), String>, read: &dyn Fn(&std::path::Path) -> Result<String, String>) -> Result<String, String> {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
    match args.first().map(String::as_str) {
        Some("files") => {
            let board = Board::parse(flag("--board").unwrap_or("pi5"))?;
            let out = std::path::Path::new(flag("--out").ok_or("--out DIR is needed")?);
            let mut names = Vec::new();
            let mut put = |name: &str, text: &str| -> Result<(), String> {
                write(&out.join(name), text.as_bytes())?;
                names.push(name.to_string());
                Ok(())
            };
            put("config.txt", &config_txt(board))?;
            for (n, t) in cmdline_files(board) {
                put(&n, &t)?;
            }
            put(BOOT_TXT, &iyakki_boot_txt(Initrd::Lean))?;
            put(BOARD_FILE, &board_file(board))?;
            put(TRYBOOT_TXT, &tryboot(board).tryboot_txt)?;
            Ok(format!("wrote {} files for {}: {}\n", names.len(), board.id(), names.join(", ")))
        }
        Some("mbr") => {
            let mib: u64 = flag("--boot-mib").and_then(|v| v.parse().ok()).ok_or("--boot-mib N is needed")?;
            if !(256..=4096).contains(&mib) {
                return Err("--boot-mib must be between 256 and 4096".into());
            }
            let out = std::path::Path::new(flag("--out").ok_or("--out FILE is needed")?);
            write(out, &mbr_sector(mib))?;
            Ok(format!("table for a {} MiB boot volume; image size {} bytes; data marker partition starts at sector {}\n", mib, image_bytes(mib), BOOT_START_SECTORS + mib * 2048))
        }
        Some("size") => {
            let n: u64 = flag("--payload-bytes").and_then(|v| v.parse().ok()).ok_or("--payload-bytes N is needed")?;
            Ok(format!("{}\n", boot_fat_mib(n)))
        }
        Some("check") => {
            // the list of files on a finished boot volume (`mdir -/ -b`, one path per line): every required file there, no vendor blob
            let board = Board::parse(flag("--board").unwrap_or("pi5"))?;
            let list = read(std::path::Path::new(flag("--list").ok_or("--list FILE is needed (one path per line)")?))?;
            let paths: Vec<&str> = list.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            let (gone, bad) = (missing(board, paths.iter().copied()), forbidden(paths.iter().copied()));
            if !gone.is_empty() || !bad.is_empty() {
                return Err(format!("the boot volume is not right: missing [{}]; must not be there [{}]", gone.join(", "), bad.join(", ")));
            }
            Ok(format!("the boot volume holds every required file for {} and no vendor boot blob ({} paths)\n", board.id(), paths.len()))
        }
        _ => Ok(RPI_HELP.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_names_our_kernel_the_device_tree_and_ends_by_including_the_ram_disk_file() {
        let c = config_txt(Board::Pi5);
        assert!(c.contains("kernel=kernel8.img\n") && c.contains("device_tree=bcm2712-rpi-5-b.dtb\n") && c.contains("arm_64bit=1\n"));
        let lines: Vec<&str> = c.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
        assert_eq!(lines[lines.len() - 2], "include iyakki-boot.txt", "the RAM disk include comes before the person's own file");
        assert_eq!(lines[lines.len() - 1], "include config-user.txt");
        assert!(!c.contains("initramfs"), "the RAM disk is chosen in iyakki-boot.txt only");
        assert!(c.contains("enable_uart=1") && c.contains("dtoverlay=vc4-kms-v3d-pi5\n"));
    }

    #[test]
    fn the_pi5_config_names_the_pi5_display_overlay_and_not_the_plain_one() {
        for c in [config_txt(Board::Pi5), tryboot(Board::Pi5).tryboot_txt] {
            assert!(c.lines().any(|l| l == "dtoverlay=vc4-kms-v3d-pi5"), "the -pi5 overlay is named");
            assert!(!c.lines().any(|l| l == "dtoverlay=vc4-kms-v3d"), "the plain name is not found by the Pi 5 firmware");
        }
    }

    #[test]
    fn every_pi5_mode_turns_the_memory_cgroup_on_and_keeps_the_serial_console_last() {
        for m in Mode::ALL {
            let c = cmdline_txt(Board::Pi5, m);
            assert!(c.contains(" cgroup_enable=memory") && c.contains(" cgroup_memory=1"), "{} lacks the cgroup words: {c}", m.name());
            assert!(c.trim_end().rfind("console=ttyAMA10,115200") > c.find("console=tty1"), "the serial console stays last");
        }
    }

    #[test]
    fn the_ram_disk_file_names_the_lean_or_the_full_disk_after_the_kernel() {
        assert!(iyakki_boot_txt(Initrd::Lean).ends_with("initramfs initrd-lean followkernel\n"));
        assert!(iyakki_boot_txt(Initrd::Full).ends_with("initramfs initrd followkernel\n"));
        assert_eq!(iyakki_boot_txt(Initrd::Lean).lines().filter(|l| !l.starts_with('#')).count(), 1, "exactly one directive");
    }

    #[test]
    fn every_mode_is_one_command_line_with_the_serial_console_last_and_the_board_named() {
        for m in Mode::ALL {
            let c = cmdline_txt(Board::Pi5, m);
            assert_eq!(c.matches('\n').count(), 1, "a command line file is one line");
            assert!(c.trim_end().rfind("console=ttyAMA10,115200") > c.find("console=tty1"), "the serial console is the last console=");
            assert!(c.contains("iyakki.board=rpi5"));
        }
        assert_eq!(Mode::Normal.file(), "cmdline-normal.txt");
        assert_eq!(Mode::HwCheck.file(), "cmdline-hwcheck.txt");
        assert!(cmdline_txt(Board::Pi5, Mode::SeedOnly).contains("iyakki.root=boot"));
        assert!(cmdline_txt(Board::Pi5, Mode::SafeGraphics).contains("modprobe.blacklist=vc4,v3d"));
        assert_eq!(Mode::parse("web"), Some(Mode::Web));
        assert_eq!(Mode::parse("nope"), None);
        let all = cmdline_files(Board::Pi5);
        assert_eq!(all.len(), 8, "cmdline.txt and one file per mode");
        assert_eq!(all[0], (CMDLINE_TXT.to_string(), cmdline_txt(Board::Pi5, Mode::Normal)));
        assert_eq!(all[0].1, all[1].1, "cmdline.txt and cmdline-normal.txt are the same line");
        let mut names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 8, "no two modes share a file");
    }

    #[test]
    fn the_command_line_options_are_the_ones_stage_one_understands() {
        // the same parser the first stage uses: nothing is lost on the way from the file to the options
        let o = iyakki_stage1::options(&cmdline_txt(Board::Pi5, Mode::SeedOnly));
        assert!(o.boot_only && !o.previous && !o.shell);
        let o = iyakki_stage1::options(&cmdline_txt(Board::Pi5, Mode::Normal));
        assert!(!o.boot_only);
        // the text the first stage writes after a failed fast start is the text this module writes when the volume is made
        assert_eq!(iyakki_boot_txt(Initrd::Lean), iyakki_stage1::pi_boot_txt(true));
        assert_eq!(iyakki_boot_txt(Initrd::Full), iyakki_stage1::pi_boot_txt(false));
        assert_eq!(BOOT_TXT, iyakki_stage1::PI_BOOT_TXT);
        assert_eq!(iyakki_stage1::pi_boot_txt_is_lean(&iyakki_boot_txt(Initrd::Lean)), Some(true));
    }

    #[test]
    fn the_board_is_parsed_and_unknown_boards_are_refused_with_a_reason() {
        assert_eq!(Board::parse("pi5"), Ok(Board::Pi5));
        assert_eq!(Board::parse(" RPI5\n"), Ok(Board::Pi5));
        assert!(Board::parse("pi3").unwrap_err().contains("design"));
        assert_eq!(board_file(Board::Pi5), "rpi5\n");
        assert_eq!(board_file(Board::Pi5).trim(), iyakki_stage1::PI5_BOARD);
    }

    #[test]
    fn a_trial_boot_reads_the_staged_files_and_promotion_and_rollback_undo_each_other() {
        let t = tryboot(Board::Pi5);
        assert!(t.tryboot_txt.contains("kernel=kernel8-next.img") && t.tryboot_txt.contains("initramfs initrd-lean-next followkernel"));
        assert!(!t.tryboot_txt.contains("include iyakki-boot.txt"), "a trial boot does not follow the include that a failed start rewrites");
        assert_eq!(t.reboot_arg, "0 tryboot");
        // replay both plans on a model of the volume
        let mut vol: std::collections::BTreeMap<String, &str> = [("kernel8.img", "old-k"), ("kernel8-next.img", "new-k"), ("initrd-lean", "old-i"), ("initrd-lean-next", "new-i")].into_iter().map(|(a, b)| (a.to_string(), b)).collect();
        let apply = |vol: &mut std::collections::BTreeMap<String, &str>, steps: &[(String, String)]| {
            for (from, to) in steps {
                let v = vol.remove(from).unwrap_or_else(|| panic!("{from} is missing when the step runs"));
                assert!(!vol.contains_key(to) || to.ends_with(".prev"), "a rename never overwrites a live file: {to}");
                vol.insert(to.clone(), v);
            }
        };
        let before = vol.clone();
        apply(&mut vol, &t.promote);
        assert_eq!(vol["kernel8.img"], "new-k");
        assert_eq!(vol["kernel8.img.prev"], "old-k", "the old kernel is kept for the rollback");
        assert_eq!(vol["initrd-lean"], "new-i");
        apply(&mut vol, &t.rollback);
        assert_eq!(vol["kernel8.img"], "old-k");
        assert_eq!(vol["initrd-lean"], "old-i");
        assert_eq!(vol["kernel8-next.img"], "new-k");
        assert_eq!(vol, before, "promote then roll back restores the volume exactly");
    }

    #[test]
    fn a_volume_missing_a_required_file_is_named_and_the_vendor_blobs_are_forbidden() {
        let all: Vec<String> = volume_files(Board::Pi5).into_iter().map(|f| f.path).collect();
        assert!(missing(Board::Pi5, all.iter().map(String::as_str)).is_empty());
        let without: Vec<&str> = all.iter().map(String::as_str).filter(|p| *p != "boot/root.sfs" && *p != "initrd").collect();
        assert_eq!(missing(Board::Pi5, without), ["boot/root.sfs"], "the full RAM disk is optional, the seed is not");
        assert_eq!(missing(Board::Pi5, ["/CONFIG.TXT"]).len(), 7, "case and a leading slash do not matter on FAT");
        let bad = forbidden(["config.txt", "bootcode.bin", "start4.elf", "fixup4.dat", "boot/Start.ELF", "pieeprom.bin", "recovery.bin", "kernel8.img", "overlays/vc4-kms-v3d-pi5.dtbo"]);
        assert_eq!(bad, ["bootcode.bin", "start4.elf", "fixup4.dat", "boot/Start.ELF", "pieeprom.bin", "recovery.bin"]);
        assert!(forbidden(all.iter().map(String::as_str)).is_empty(), "the volume this module describes holds no vendor blob");
    }

    #[test]
    fn the_sd_card_table_is_the_install_template_the_writer_already_stretches() {
        let boot_mib = boot_fat_mib(150 * 1024 * 1024);
        let s = mbr_sector(boot_mib);
        assert!(crate::writer::is_install_template(&s), "the signature marks it as a template, so the writer grows the data partition");
        let [(s1, l1, t1), (s2, l2, t2)] = read_mbr(&s).unwrap();
        assert_eq!((s1, t1, s[446]), (8192, 0x0C, 0x80), "FAT32 LBA, bootable, 4 MiB in");
        assert_eq!(l1, boot_mib * 2048);
        assert_eq!((s2, l2, t2), (s1 + l1, 2048, 0x83), "the data partition follows the boot volume with no gap");
        assert_eq!(s1 % 2048, 0);
        assert_eq!(s2 % 2048, 0, "both partitions are 1 MiB aligned");
        assert_eq!(image_bytes(boot_mib), (s2 + l2) * 512);
        // the existing writer accepts it: partition 2 grows to fill a 64 GB card
        let mut sector = s;
        let lay = crate::writer::patch_install_mbr(&mut sector, 64_000_000_000, [1, 2, 3, 4]).unwrap();
        assert_eq!(lay.data_start, s2);
        assert!(lay.data_sectors > 100_000_000 && lay.data_sectors % 2048 == 0);
        assert_eq!(read_mbr(&sector).unwrap()[0], (s1, l1, 0x0C), "partition 1 is untouched");
        assert!(read_mbr(&[0u8; 512]).is_err());
    }

    #[test]
    fn the_boot_volume_is_never_below_256_mib_and_grows_with_the_payload() {
        assert_eq!(boot_fat_mib(0), 256);
        assert_eq!(boot_fat_mib(100 * 1024 * 1024), 256);
        assert!(boot_fat_mib(400 * 1024 * 1024) >= 400 * 5 / 4 + 32);
        assert_eq!(boot_fat_mib(400 * 1024 * 1024), 400 + 100 + 32);
        // FAT32 needs 65525 clusters or more: with 4 KiB clusters 256 MiB is 65536
        assert!(256 * 1024 / 4 >= 65525);
    }

    #[test]
    fn the_command_writes_the_generated_files_and_the_table_and_refuses_bad_input() {
        use std::cell::RefCell;
        let written: RefCell<Vec<(String, Vec<u8>)>> = RefCell::new(Vec::new());
        let mut w = |p: &std::path::Path, b: &[u8]| -> Result<(), String> {
            written.borrow_mut().push((p.display().to_string(), b.to_vec()));
            Ok(())
        };
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let no_read = |_: &std::path::Path| -> Result<String, String> { Err("no file".into()) };
        let msg = run_cli(&a(&["files", "--board", "pi5", "--out", "/o"]), &mut w, &no_read).unwrap();
        assert!(msg.starts_with("wrote 12 files for rpi5"));
        let names: Vec<String> = written.borrow().iter().map(|(n, _)| n.clone()).collect();
        assert!(names.contains(&"/o/config.txt".to_string()) && names.contains(&"/o/cmdline-web.txt".to_string()) && names.contains(&"/o/tryboot.txt".to_string()) && names.contains(&"/o/board".to_string()));
        assert!(run_cli(&a(&["files", "--board", "pi3", "--out", "/o"]), &mut w, &no_read).is_err());
        assert!(run_cli(&a(&["files"]), &mut w, &no_read).is_err(), "no --out");
        written.borrow_mut().clear();
        let msg = run_cli(&a(&["mbr", "--boot-mib", "256", "--out", "/o/mbr.bin"]), &mut w, &no_read).unwrap();
        assert!(msg.contains("image size"));
        assert_eq!(written.borrow()[0].1.len(), 512);
        assert!(run_cli(&a(&["mbr", "--boot-mib", "10", "--out", "/o/m"]), &mut w, &no_read).is_err());
        assert_eq!(run_cli(&a(&["size", "--payload-bytes", "0"]), &mut w, &no_read).unwrap(), "256\n");
        assert!(run_cli(&a(&[]), &mut w, &no_read).unwrap().contains("iyakki-usb rpi"));
        // `check`: the listing of a finished volume
        let all: Vec<String> = volume_files(Board::Pi5).into_iter().map(|f| f.path).collect();
        let good = all.join("\n");
        let read_good = |_: &std::path::Path| -> Result<String, String> { Ok(good.clone()) };
        let ok = run_cli(&a(&["check", "--list", "/l"]), &mut w, &read_good).unwrap();
        assert!(ok.contains("every required file") && ok.contains("no vendor boot blob"));
        let with_blob = format!("{good}\nbootcode.bin");
        let read_blob = |_: &std::path::Path| -> Result<String, String> { Ok(with_blob.clone()) };
        assert!(run_cli(&a(&["check", "--list", "/l"]), &mut w, &read_blob).unwrap_err().contains("bootcode.bin"));
        let read_short = |_: &std::path::Path| -> Result<String, String> { Ok("config.txt\n".to_string()) };
        assert!(run_cli(&a(&["check", "--list", "/l"]), &mut w, &read_short).unwrap_err().contains("kernel8.img"));
        assert!(run_cli(&a(&["check"]), &mut w, &no_read).is_err(), "no --list");
    }

    /// The generator of the loader repository (`iyakki-boot/crates/board-cfg`, one board table for every board) holds golden copies of the
    /// tested image's boot files. The files written here must be those bytes, so the day the image scripts switch to the generator nothing
    /// on the card changes. (Skipped when the submodule is not checked out.)
    #[test]
    fn the_baseline_files_are_the_golden_files_of_the_board_table_generator() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../iyakki-boot/crates/board-cfg/tests/golden/baseline-rpi5");
        if !dir.is_dir() {
            eprintln!("skipped: the iyakki-boot submodule is not checked out");
            return;
        }
        let golden = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_else(|e| panic!("{n}: {e}"));
        assert_eq!(config_txt(Board::Pi5), golden("config.txt"));
        assert_eq!(iyakki_boot_txt(Initrd::Lean), golden("iyakki-boot.txt"));
        assert_eq!(iyakki_boot_txt(Initrd::Full), golden("iyakki-boot-full.txt"));
        assert_eq!(tryboot(Board::Pi5).tryboot_txt, golden("tryboot.txt"));
        assert_eq!(board_file(Board::Pi5), golden("board"));
        let files = cmdline_files(Board::Pi5);
        assert_eq!(files.len(), 8);
        for (name, text) in files {
            assert_eq!(text, golden(&name), "{name}");
        }
    }
}

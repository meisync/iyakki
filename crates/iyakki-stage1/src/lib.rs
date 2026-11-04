//! Pure decisions of the first-stage init (docs/BOOT-SPEED.md). The system calls are in `main.rs`; everything that can be wrong
//! without a kernel (reading a label, parsing the pointer file, ordering the places the root may come from) is here and tested.

/// Label of the boot volume and of the data volume (same names as `iyakki-storage` and the installer).
pub const BOOT_LABEL: &str = "IYAKKI-BOOT";
pub const DATA_LABEL: &str = "IYAKKI-DATA";
/// The seed root on the FAT boot volume (`boot/root.sfs`), written by the image build and by `iyakki-usb`.
pub const SEED_IMAGE: &str = "boot/root.sfs";
/// Roots that were put on the data volume by an update: `iyakki/root/<id>/root.sfs` or `iyakki/root/<id>/tree/`, chosen by the
/// pointer files `iyakki/root/current` and `iyakki/root/previous` (one line: the id; an optional second line `size=BYTES`).
pub const ROOTS_DIR: &str = "iyakki/root";

/// Label of a FAT boot sector (first 512 bytes of the partition): the label of the boot sector itself, FAT32 (offset 71, type text at 82) or
/// FAT12/16 (offset 43, type text at 54), trimmed of the padding spaces. Formatters differ: some leave the type text empty, some write the
/// label only in the root directory (see `fat_label_in_root`), some use FAT16 for a small volume.
pub fn fat32_label(sector: &[u8]) -> Option<String> {
    if sector.len() < 512 || sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }
    let bps = u16::from_le_bytes([sector[11], sector[12]]);
    if !matches!(bps, 512 | 1024 | 2048 | 4096) || sector[13] == 0 || !sector[13].is_power_of_two() {
        return None;
    }
    let fatsz16 = u16::from_le_bytes([sector[22], sector[23]]);
    let (lo, ty) = if fatsz16 == 0 { (71, &sector[82..90]) } else { (43, &sector[54..62]) };
    let is_fat32 = fatsz16 == 0 && u32::from_le_bytes([sector[36], sector[37], sector[38], sector[39]]) > 0;
    if !is_fat32 && fatsz16 == 0 {
        return None;
    }
    if !(ty.starts_with(b"FAT") || ty.iter().all(|&c| c == b' ' || c == 0)) {
        return None;
    }
    Some(String::from_utf8_lossy(&sector[lo..lo + 11]).trim_end().to_string())
}

/// Where a FAT32 volume's root directory starts and how long to read: (byte offset in the partition, bytes), from its boot sector.
pub fn fat32_root_dir(sector: &[u8]) -> Option<(u64, usize)> {
    fat32_label(sector)?;
    let bps = u16::from_le_bytes([sector[11], sector[12]]) as u64;
    let spc = sector[13] as u64;
    let rsvd = u16::from_le_bytes([sector[14], sector[15]]) as u64;
    let nfats = sector[16] as u64;
    let fatsz = u32::from_le_bytes([sector[36], sector[37], sector[38], sector[39]]) as u64;
    let root = u32::from_le_bytes([sector[44], sector[45], sector[46], sector[47]]) as u64;
    if fatsz == 0 || root < 2 {
        return None;
    }
    Some(((rsvd + nfats * fatsz) * bps + (root - 2) * spc * bps, (spc * bps).min(32768) as usize))
}

/// The volume-label entry (attribute 0x08, not a long-name entry) of a FAT root directory block, trimmed.
pub fn fat_label_in_root(dir: &[u8]) -> Option<String> {
    for e in dir.chunks_exact(32) {
        if e[0] == 0 {
            break;
        }
        if e[0] != 0xE5 && e[11] & 0x3F == 0x08 {
            return Some(String::from_utf8_lossy(&e[..11]).trim_end().to_string());
        }
    }
    None
}

/// Is this label the boot volume's (FAT labels are upper case by convention but some tools keep what they were given).
pub fn is_boot_label(l: &str) -> bool {
    l.trim().eq_ignore_ascii_case(BOOT_LABEL)
}

/// Label of an ext2/3/4 filesystem (`sb` is the 1024 bytes that start at byte offset 1024 of the partition).
pub fn ext4_label(sb: &[u8]) -> Option<String> {
    if sb.len() < 136 || sb[56] != 0x53 || sb[57] != 0xEF {
        return None;
    }
    let raw = &sb[120..136];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Some(String::from_utf8_lossy(&raw[..end]).to_string())
}

/// What a pointer file names.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Pointer {
    pub id: String,
    pub size: Option<u64>,
}

/// An id is a plain folder name: letters, digits, `.`, `_`, `-`, 1 to 64 characters, never `..`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id != "." && !id.contains("..") && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Pointer {
    pub fn parse(text: &str) -> Option<Pointer> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let id = lines.next()?;
        if !valid_id(id) {
            return None;
        }
        let size = lines.find_map(|l| l.strip_prefix("size=")).and_then(|v| v.trim().parse().ok());
        Some(Pointer { id: id.to_string(), size })
    }
}

/// Where a root comes from.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Volume {
    Data,
    Boot,
}

/// A way of presenting a root: a squashfs image attached to a loop device, or a folder with the extracted files.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    Image,
    Tree,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Candidate {
    pub volume: Volume,
    pub kind: Kind,
    /// Path inside the volume, no leading slash.
    pub path: String,
    /// The size the pointer promised (an image of another size is a half-written update and is skipped).
    pub size: Option<u64>,
}

/// The roots to try, best first: the data volume's `current`, then its `previous` (rollback), then the seed on the boot volume.
/// `kind_of` says what `iyakki/root/<id>/` holds (an image file, a tree folder, or nothing usable).
pub fn candidates(current: Option<&Pointer>, previous: Option<&Pointer>, kind_of: &dyn Fn(&str) -> Option<Kind>, boot_seed: bool) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for p in [current, previous].into_iter().flatten() {
        if seen.contains(&p.id.as_str()) {
            continue;
        }
        seen.push(&p.id);
        let base = format!("{ROOTS_DIR}/{}", p.id);
        match kind_of(&base) {
            Some(Kind::Tree) => out.push(Candidate { volume: Volume::Data, kind: Kind::Tree, path: format!("{base}/tree"), size: None }),
            Some(Kind::Image) => out.push(Candidate { volume: Volume::Data, kind: Kind::Image, path: format!("{base}/root.sfs"), size: p.size }),
            None => {}
        }
    }
    if boot_seed {
        out.push(Candidate { volume: Volume::Boot, kind: Kind::Image, path: SEED_IMAGE.to_string(), size: None });
    }
    out
}

/// How long the first stage waits for the stick (a USB SSD appears 0.5 to 3 s after the kernel starts; behind a Mac's Thunderbolt/T2 chain possibly much later, so 60 s), and for the
/// second volume once the first one is there (both partitions of a disk are registered together; the extra seconds are for a slow scan).
pub const WAIT_FOR_DEVICES_MS: u64 = 60_000;
pub const WAIT_FOR_SECOND_MS: u64 = 3_000;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Wait {
    /// Both volumes are there, or one is and the other had its chance: go on.
    Go,
    /// Look again shortly.
    Keep,
    /// Nothing appeared in time: say so clearly and do not hang.
    GiveUp,
}

/// `since_first_ms`: time since the first of the two volumes was seen (None while none is), `since_start_ms`: time since the search began.
pub fn wait_decision(boot: bool, data: bool, since_first_ms: Option<u64>, since_start_ms: u64) -> Wait {
    if boot && data {
        return Wait::Go;
    }
    if (boot || data) && since_first_ms.is_some_and(|t| t >= WAIT_FOR_SECOND_MS) {
        return Wait::Go;
    }
    if since_start_ms > WAIT_FOR_DEVICES_MS {
        return if boot || data { Wait::Go } else { Wait::GiveUp };
    }
    Wait::Keep
}

/// Keep a text log bounded: `old` (what is in the file) plus `new`, at most `max` bytes, cut at a line start, newest kept.
pub fn bounded_log(old: &str, new: &str, max: usize) -> String {
    let all = format!("{old}{new}");
    if all.len() <= max {
        return all;
    }
    let mut cut = all.len() - max;
    while cut < all.len() && !all.is_char_boundary(cut) {
        cut += 1;
    }
    // start at the next line so no half line is kept
    let rest = &all[cut..];
    match rest.find('\n') {
        Some(i) => rest[i + 1..].to_string(),
        None => rest.to_string(),
    }
}

/// One `/dev/kmsg` record (`pri,seq,usec,flags;text`) as a log line `[ s.uuuuuu] text`; continuation lines (start with a space) are dropped.
pub fn kmsg_line(rec: &str) -> Option<String> {
    let first = rec.lines().next()?;
    let (head, text) = first.split_once(';')?;
    let usec: u64 = head.split(',').nth(2)?.parse().ok()?;
    Some(format!("[{:>5}.{:06}] {}", usec / 1_000_000, usec % 1_000_000, text))
}

/// Human size of a block device given its 512-byte sector count (`/sys/class/block/X/size`).
pub fn human_size(sectors: u64) -> String {
    let b = sectors * 512;
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{:.0} MB", b as f64 / (1u64 << 20) as f64) }
}

/// Mount options of the overlay that carries the writes (upper layer on tmpfs, which can be swapped out to zram/disk).
pub fn overlay_options(lower: &str, upper: &str, work: &str) -> String {
    format!("lowerdir={lower},upperdir={upper},workdir={work}")
}

// ------------------------------------------------------------------------------------------------ patch layers (docs/development/REMOTE-PATCH.md)

/// Hot patches (`iyakki dev patch`) live on the data volume: `iyakki/patches/active` lists the active ids (one per line, the one that wins first),
/// `iyakki/patches/<id>/layer/` is the tree that goes over the root as an extra overlay lower layer, `iyakki/patches/<id>/layer.txt` describes it
/// (see [`parse_layer_info`]). Nothing here is ever required for a boot: a patch that cannot be used is skipped, and after [`PATCH_GUARD_MAX`]
/// starts that did not reach the running system (`iyakki/patches/boot-count`, removed by the system once it is up) none is used.
pub const PATCHES_DIR: &str = "iyakki/patches";
pub const PATCH_GUARD_MAX: u32 = 3;
pub const PATCH_MAX_LAYERS: usize = 16;
pub const PATCH_MAX_ENTRIES: usize = 2000;

/// The ids named by the `active` file: valid ids only, each once, at most [`PATCH_MAX_LAYERS`].
pub fn parse_active(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if valid_id(l) && !out.iter().any(|x| x == l) && out.len() < PATCH_MAX_LAYERS {
            out.push(l.to_string());
        }
    }
    out
}

/// What a patch layer must contain, as its `layer.txt` says.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct LayerInfo {
    pub id: String,
    /// The root id the patch was made for (`*`: any root).
    pub base: String,
    /// (kind, size, path in the layer): kind `file` (exact size), `link` or `del` (a whiteout) must exist.
    pub entries: Vec<(String, u64, String)>,
}

fn layer_path_ok(p: &str) -> bool {
    p.starts_with('/') && p.len() <= 400 && !p.contains(['\n', ',', ':']) && p.split('/').skip(1).all(|c| !c.is_empty() && c != "." && c != "..")
}

/// `iyakki-patch-layer 1`, `id=ID`, `base=ROOT-ID|*`, then lines `file SIZE /path`, `link 0 /path`, `del 0 /path`. Anything else is not a layer.
pub fn parse_layer_info(text: &str) -> Option<LayerInfo> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next()? != "iyakki-patch-layer 1" {
        return None;
    }
    let (mut id, mut base, mut entries) = (None, None, Vec::new());
    for l in lines {
        if let Some(v) = l.strip_prefix("id=") {
            id = Some(v.to_string()).filter(|v| valid_id(v));
        } else if let Some(v) = l.strip_prefix("base=") {
            base = Some(v.to_string()).filter(|v| v == "*" || valid_id(v));
        } else {
            let mut it = l.splitn(3, ' ');
            let (kind, size, path) = (it.next()?, it.next()?.parse::<u64>().ok()?, it.next()?);
            if !matches!(kind, "file" | "link" | "del") || !layer_path_ok(path) || entries.len() >= PATCH_MAX_ENTRIES {
                return None;
            }
            entries.push((kind.to_string(), size, path.to_string()));
        }
    }
    Some(LayerInfo { id: id?, base: base?, entries })
}

/// The id of the root a candidate path names (`iyakki/root/<id>/tree` gives `<id>`); None for the seed on the boot volume.
pub fn root_id_of(path: &str) -> Option<&str> {
    path.strip_prefix(ROOTS_DIR)?.strip_prefix('/')?.split('/').next().filter(|id| valid_id(id))
}

/// Which patch layers to put over the root, and why the others are not.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct PatchPlan {
    /// Paths on the data volume (no leading slash), highest priority first.
    pub layers: Vec<String>,
    /// (id, reason) of every patch left out, for the log.
    pub skipped: Vec<(String, String)>,
    /// Too many starts in a row did not reach the running system: no patch is used.
    pub guard: bool,
}

/// Decide the layers. `attempts`: starts that began with patches and were not marked good; `root_id`: the id of the chosen root;
/// `read(path)`: a text file of the data volume; `stat(path)`: size of an entry there without following a link (None: missing).
pub fn plan_patches(active: &str, attempts: u32, root_id: Option<&str>, read: &dyn Fn(&str) -> Option<String>, stat: &dyn Fn(&str) -> Option<u64>) -> PatchPlan {
    let ids = parse_active(active);
    let mut plan = PatchPlan::default();
    if ids.is_empty() {
        return plan;
    }
    if attempts >= PATCH_GUARD_MAX {
        plan.guard = true;
        plan.skipped = ids.into_iter().map(|i| (i, format!("{attempts} starts in a row did not reach the system")) ).collect();
        return plan;
    }
    for id in ids {
        let base = format!("{PATCHES_DIR}/{id}");
        let skip = |why: &str| (id.clone(), why.to_string());
        let Some(info) = read(&format!("{base}/layer.txt")).and_then(|t| parse_layer_info(&t)) else {
            plan.skipped.push(skip("layer.txt is missing or not valid"));
            continue;
        };
        if info.id != id {
            plan.skipped.push(skip("layer.txt names another patch"));
            continue;
        }
        if info.base != "*" && Some(info.base.as_str()) != root_id {
            plan.skipped.push(skip(&format!("made for root {}, not this one (obsolete)", info.base)));
            continue;
        }
        let bad = info.entries.iter().find(|(k, size, p)| match stat(&format!("{base}/layer{p}")) {
            None => true,
            Some(n) => k == "file" && n != *size,
        });
        if let Some((_, _, p)) = bad {
            plan.skipped.push(skip(&format!("{p} is missing or has the wrong size")));
            continue;
        }
        plan.layers.push(format!("{base}/layer"));
    }
    plan
}

/// Overlay options with patch layers (full paths) over the root, which stays the lowest layer.
pub fn overlay_options_patched(patches: &[String], lower: &str, upper: &str, work: &str) -> String {
    let mut l = patches.join(":");
    if !l.is_empty() {
        l.push(':');
    }
    l.push_str(lower);
    overlay_options(&l, upper, work)
}

/// The start counter file's text as a number (anything unreadable counts as 0).
pub fn parse_count(text: &str) -> u32 {
    text.trim().parse().unwrap_or(0)
}

/// The kernel command line asks for a pause with a shell instead of the switch (`iyakki.stage1=shell`), or for the root
/// to be taken from one place only (`iyakki.root=boot` ignores updates on the data volume: the rollback of last resort).
#[derive(Debug, PartialEq, Eq, Default)]
pub struct Options {
    pub shell: bool,
    pub boot_only: bool,
    /// `iyakki.root=previous`: the root the last update replaced (the boot menu's "previous version" entry), never the current one.
    pub previous: bool,
    /// `iyakki.data=/dev/sdb2`: the data partition the person chose; without it the system takes the data volume of its own disk.
    pub data: Option<String>,
}

pub fn options(cmdline: &str) -> Options {
    let mut o = Options::default();
    for t in cmdline.split_whitespace() {
        match t {
            "iyakki.stage1=shell" => o.shell = true,
            "iyakki.root=boot" => o.boot_only = true,
            "iyakki.root=previous" => o.previous = true,
            _ => {
                if let Some(d) = t.strip_prefix("iyakki.data=").filter(|d| d.starts_with("/dev/")) {
                    o.data = Some(d.to_string());
                }
            }
        }
    }
    o
}

/// The colour of row `y` of `h` rows of the boot picture: the vertical gradient of GRUB's background (`iyakki-bg.png`: 7,14,28 at the top
/// to 3,5,12 at the bottom), so the screen goes from GRUB's picture to the boot animation without a black gap.
pub fn bg_colour(y: u32, h: u32) -> [u8; 3] {
    let t = if h <= 1 { 0.0 } else { y.min(h - 1) as f32 / (h - 1) as f32 };
    let mix = |a: f32, b: f32| (a + (b - a) * t).round() as u8;
    [mix(7.0, 3.0), mix(14.0, 5.0), mix(28.0, 12.0)]
}

/// One pixel of 32 or 16 bits as the framebuffer wants it, from the (offset, length) of its red, green and blue fields. `None` for a
/// layout that is not one of those (the screen is left alone).
pub fn pack_pixel(rgb: [u8; 3], bpp: u32, red: (u32, u32), green: (u32, u32), blue: (u32, u32)) -> Option<u32> {
    if !(bpp == 32 || bpp == 16) || [red, green, blue].iter().any(|(o, l)| *l == 0 || *l > 8 || o + l > bpp) {
        return None;
    }
    let f = |v: u8, (o, l): (u32, u32)| ((v as u32) >> (8 - l)) << o;
    Some(f(rgb[0], red) | f(rgb[1], green) | f(rgb[2], blue))
}

/// The disk a `/dev/...` partition or disk node belongs to: `/dev/sda2` -> `sda`, `/dev/nvme0n1p3` -> `nvme0n1`, `/dev/mmcblk0p2` -> `mmcblk0`.
pub fn disk_of(dev: &str) -> String {
    let n = dev.strip_prefix("/dev/").unwrap_or(dev);
    let digits_off = n.trim_end_matches(|c: char| c.is_ascii_digit());
    if (n.starts_with("nvme") || n.starts_with("mmcblk")) && digits_off.ends_with('p') && digits_off.len() > 1 {
        return digits_off[..digits_off.len() - 1].to_string();
    }
    if n.starts_with("nvme") || n.starts_with("mmcblk") {
        return n.to_string(); // the whole disk
    }
    digits_off.to_string()
}

/// Which of the data volumes found is this system's: the one the person chose, else the one on the disk the boot volume is on. With a
/// boot volume found and no data volume on its disk the answer is `None`: it does not take another disk's. With no boot volume (a
/// data-only drive) the first one.
pub fn pick_data(boot: Option<&str>, candidates: &[String], choice: Option<&str>) -> Option<String> {
    if let Some(c) = choice {
        return candidates.iter().find(|d| d.as_str() == c).cloned();
    }
    match boot {
        Some(b) => candidates.iter().find(|d| disk_of(d) == disk_of(b)).cloned(),
        None => candidates.first().cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_screen_is_grubs_gradient_in_the_framebuffers_own_pixel_format() {
        assert_eq!(bg_colour(0, 1080), [7, 14, 28]);
        assert_eq!(bg_colour(1079, 1080), [3, 5, 12]);
        assert_eq!(bg_colour(5000, 1080), [3, 5, 12], "past the end stays at the last row");
        assert_eq!(bg_colour(0, 0), [7, 14, 28]);
        // XRGB8888 (red at 16, green at 8, blue at 0) and RGB565
        assert_eq!(pack_pixel([7, 14, 28], 32, (16, 8), (8, 8), (0, 8)), Some(0x070e1c));
        assert_eq!(pack_pixel([255, 255, 255], 16, (11, 5), (5, 6), (0, 5)), Some(0xffff));
        assert_eq!(pack_pixel([1, 2, 3], 24, (16, 8), (8, 8), (0, 8)), None, "24 bits is left alone");
        assert_eq!(pack_pixel([1, 2, 3], 32, (16, 0), (8, 8), (0, 8)), None);
    }

    #[test]
    fn a_system_takes_its_own_data_volume_not_the_first_one_found() {
        assert_eq!(disk_of("/dev/sda2"), "sda");
        assert_eq!(disk_of("/dev/nvme0n1p3"), "nvme0n1");
        assert_eq!(disk_of("/dev/mmcblk0p2"), "mmcblk0");
        assert_eq!(disk_of("/dev/vdb12"), "vdb");
        let both = vec!["/dev/nvme0n1p2".to_string(), "/dev/sda3".to_string()];
        assert_eq!(pick_data(Some("/dev/sda2"), &both, None).as_deref(), Some("/dev/sda3"), "the stick's own, though nvme sorts first");
        assert_eq!(pick_data(Some("/dev/nvme0n1p1"), &both, None).as_deref(), Some("/dev/nvme0n1p2"));
        assert_eq!(pick_data(Some("/dev/sdb1"), &both, None), None, "no volume on its own disk: none of the others");
        assert_eq!(pick_data(Some("/dev/sda2"), &both, Some("/dev/nvme0n1p2")).as_deref(), Some("/dev/nvme0n1p2"), "the person's choice wins");
        assert_eq!(pick_data(Some("/dev/sda2"), &both, Some("/dev/sdz9")), None);
        assert_eq!(pick_data(None, &both, None).as_deref(), Some("/dev/nvme0n1p2"));
    }

    #[test]
    fn labels_are_read_from_the_first_sectors() {
        let mut s = vec![0u8; 512];
        s[82..90].copy_from_slice(b"FAT32   ");
        s[71..82].copy_from_slice(b"IYAKKI-BOOT");
        s[11..13].copy_from_slice(&512u16.to_le_bytes());
        s[13] = 8;
        s[36] = 1;
        s[510] = 0x55;
        s[511] = 0xAA;
        assert_eq!(fat32_label(&s).as_deref(), Some(BOOT_LABEL));
        s[511] = 0;
        assert_eq!(fat32_label(&s), None);
        let mut e = vec![0u8; 1024];
        e[56] = 0x53;
        e[57] = 0xEF;
        e[120..131].copy_from_slice(b"IYAKKI-DATA");
        assert_eq!(ext4_label(&e).as_deref(), Some(DATA_LABEL));
        e[57] = 0;
        assert_eq!(ext4_label(&e), None);
        assert_eq!(ext4_label(&[0; 10]), None);
    }

    #[test]
    fn fat_labels_are_found_whatever_the_formatter_did() {
        let mut s = vec![0u8; 512];
        s[510] = 0x55;
        s[511] = 0xAA;
        s[11..13].copy_from_slice(&512u16.to_le_bytes());
        s[13] = 8;
        s[36] = 1; // FAT32: sectors per FAT in the 32-bit field
        s[71..82].copy_from_slice(b"IYAKKI-BOOT");
        // type text empty (some formatters): still FAT32
        assert_eq!(fat32_label(&s).as_deref(), Some("IYAKKI-BOOT"));
        s[82..90].copy_from_slice(b"FAT32   ");
        assert_eq!(fat32_label(&s).as_deref(), Some("IYAKKI-BOOT"));
        s[82..90].copy_from_slice(b"NTFS    ");
        assert_eq!(fat32_label(&s), None);
        // lower case and padding
        s[82..90].copy_from_slice(b"FAT32   ");
        s[71..82].copy_from_slice(b"iyakki-boot");
        assert!(is_boot_label(&fat32_label(&s).unwrap()));
        // FAT16 (small volume): label at 43, type text at 54, sectors per FAT in the 16-bit field
        let mut f = vec![0u8; 512];
        f[510] = 0x55;
        f[511] = 0xAA;
        f[11..13].copy_from_slice(&512u16.to_le_bytes());
        f[13] = 4;
        f[22] = 100;
        f[43..54].copy_from_slice(b"IYAKKI-BOOT");
        f[54..62].copy_from_slice(b"FAT16   ");
        assert_eq!(fat32_label(&f).as_deref(), Some("IYAKKI-BOOT"));
        // a label that exists only as a root directory entry (the boot sector says NO NAME)
        s[71..82].copy_from_slice(b"NO NAME    ");
        assert_eq!(fat32_label(&s).as_deref(), Some("NO NAME"));
        s[14] = 32;
        s[16] = 2;
        s[44] = 2;
        assert_eq!(fat32_root_dir(&s), Some(((32 + 2) * 512, 4096)));
        let mut dir = vec![0u8; 128];
        dir[..11].copy_from_slice(b"BOOT       ");
        dir[11] = 0x10;
        dir[32] = 0x41;
        dir[32 + 11] = 0x0F; // a long-name entry is not the label
        dir[64..75].copy_from_slice(b"IYAKKI-BOOT");
        dir[64 + 11] = 0x08;
        assert_eq!(fat_label_in_root(&dir).as_deref(), Some("IYAKKI-BOOT"));
        assert_eq!(fat_label_in_root(&[0u8; 64]), None);
        assert_eq!(fat32_label(&[0u8; 512]), None);
    }

    #[test]
    fn pointer_files_are_parsed_strictly() {
        assert_eq!(Pointer::parse("0.1.0-alpha.2\nsize=123\n"), Some(Pointer { id: "0.1.0-alpha.2".into(), size: Some(123) }));
        assert_eq!(Pointer::parse("a1\n"), Some(Pointer { id: "a1".into(), size: None }));
        for bad in ["", "\n", "../etc", "a/b", "a b", "..", ".", &"x".repeat(65)] {
            assert_eq!(Pointer::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Pointer::parse("a1\nsize=oops").unwrap().size, None);
    }

    #[test]
    fn roots_are_tried_current_then_previous_then_the_seed() {
        let cur = Pointer { id: "n2".into(), size: Some(10) };
        let prev = Pointer { id: "n1".into(), size: None };
        let kinds = |p: &str| match p {
            "iyakki/root/n2" => Some(Kind::Image),
            "iyakki/root/n1" => Some(Kind::Tree),
            _ => None,
        };
        let c = candidates(Some(&cur), Some(&prev), &kinds, true);
        assert_eq!(c.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), ["iyakki/root/n2/root.sfs", "iyakki/root/n1/tree", "boot/root.sfs"]);
        assert_eq!(c[0].size, Some(10));
        assert_eq!(c[2].volume, Volume::Boot);
        // a pointer to something that is not there is skipped; the same id twice is tried once; no seed, no candidate
        assert_eq!(candidates(Some(&Pointer { id: "gone".into(), size: None }), None, &kinds, false), vec![]);
        assert_eq!(candidates(Some(&cur), Some(&cur), &kinds, false).len(), 1);
        assert_eq!(candidates(None, None, &kinds, true).len(), 1);
    }

    #[test]
    fn the_wait_for_a_late_usb_disk_is_bounded_and_ends_clearly() {
        // nothing yet: keep looking (a USB SSD shows up 0.5 to 3 s in) until 60 s, then give up (the message and restart)
        assert_eq!(wait_decision(false, false, None, 0), Wait::Keep);
        assert_eq!(wait_decision(false, false, None, 3_000), Wait::Keep);
        assert_eq!(wait_decision(false, false, None, 40_000), Wait::Keep);
        assert_eq!(wait_decision(false, false, None, 60_000), Wait::Keep);
        assert_eq!(wait_decision(false, false, None, 60_001), Wait::GiveUp);
        // both there: go at once
        assert_eq!(wait_decision(true, true, Some(0), 1_400), Wait::Go);
        // only one: the other gets 3 s from the moment the first appeared, then the first is used alone (the root decides what is usable)
        assert_eq!(wait_decision(true, false, Some(0), 1_400), Wait::Keep);
        assert_eq!(wait_decision(true, false, Some(2_999), 4_400), Wait::Keep);
        assert_eq!(wait_decision(false, true, Some(3_000), 4_400), Wait::Go);
        // a stick that appears at the very end is still used
        assert_eq!(wait_decision(true, false, Some(0), 60_500), Wait::Go);
    }

    #[test]
    fn the_log_on_the_stick_stays_bounded_and_keeps_whole_lines() {
        assert_eq!(bounded_log("a\n", "b\n", 100), "a\nb\n");
        let old = "l1\nl2\nl3\n".repeat(10);
        let out = bounded_log(&old, "new line\n", 40);
        assert!(out.len() <= 40 && out.ends_with("new line\n") && out.starts_with('l') && out.lines().all(|l| l == "l1" || l == "l2" || l == "l3" || l == "new line"));
        assert_eq!(kmsg_line("6,339,5140900,-;stage1: started\n SUBSYSTEM=x\n").as_deref(), Some("[    5.140900] stage1: started"));
        assert_eq!(kmsg_line("garbage"), None);
        assert_eq!(human_size(4096), "2 MB");
        assert_eq!(human_size(1 << 31), "1024.0 GB");
    }

    #[test]
    fn overlay_and_cmdline() {
        assert_eq!(overlay_options("/l", "/u", "/w"), "lowerdir=/l,upperdir=/u,workdir=/w");
        assert_eq!(options("quiet iyakki.stage1=shell iyakki.root=boot"), Options { shell: true, boot_only: true, previous: false, data: None });
        assert_eq!(options("quiet iyakki.root=previous"), Options { previous: true, ..Options::default() });
        assert_eq!(options("iyakki.data=/dev/sdb2").data.as_deref(), Some("/dev/sdb2"));
        assert_eq!(options("iyakki.data=ram").data, None, "ram is not a partition");
        assert_eq!(options("quiet"), Options::default());
    }

    fn layer_txt(id: &str, base: &str, entries: &[&str]) -> String {
        format!("iyakki-patch-layer 1\nid={id}\nbase={base}\n{}\n", entries.join("\n"))
    }

    /// A data volume as a map: file texts and entry sizes.
    fn vol(files: &[(&str, String)], sizes: &[(&str, u64)]) -> (std::collections::HashMap<String, String>, std::collections::HashMap<String, u64>) {
        (files.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), sizes.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    }

    #[test]
    fn no_patches_dir_means_no_layers_and_the_old_overlay() {
        let (f, s) = vol(&[], &[]);
        let p = plan_patches("", 0, Some("n1"), &|x| f.get(x).cloned(), &|x| s.get(x).copied());
        assert_eq!(p, PatchPlan::default());
        assert_eq!(overlay_options_patched(&[], "/lower", "/u", "/w"), overlay_options("/lower", "/u", "/w"));
        // an active file naming nothing usable
        assert!(parse_active("../x\n a b\n").is_empty());
    }

    #[test]
    fn a_valid_patch_layer_goes_over_the_root_newest_first() {
        let (f, s) = vol(
            &[
                ("iyakki/patches/p2/layer.txt", layer_txt("p2", "*", &["file 12 /usr/bin/kubectl", "del 0 /etc/old.conf"])),
                ("iyakki/patches/p1/layer.txt", layer_txt("p1", "n1", &["file 5 /etc/asound.conf", "link 0 /usr/bin/x"])),
            ],
            &[("iyakki/patches/p2/layer/usr/bin/kubectl", 12), ("iyakki/patches/p2/layer/etc/old.conf", 0), ("iyakki/patches/p1/layer/etc/asound.conf", 5), ("iyakki/patches/p1/layer/usr/bin/x", 9)],
        );
        let p = plan_patches("p2\np1\np2\n", 0, Some("n1"), &|x| f.get(x).cloned(), &|x| s.get(x).copied());
        assert_eq!(p.layers, ["iyakki/patches/p2/layer", "iyakki/patches/p1/layer"]);
        assert!(p.skipped.is_empty() && !p.guard);
        let full: Vec<String> = p.layers.iter().map(|l| format!("/s/data/{l}")).collect();
        assert_eq!(overlay_options_patched(&full, "/lower", "/u", "/w"), "lowerdir=/s/data/iyakki/patches/p2/layer:/s/data/iyakki/patches/p1/layer:/lower,upperdir=/u,workdir=/w");
        // a patch made for another root (a newer tree replaced it) is skipped, one for any root is kept
        let p = plan_patches("p2\np1", 0, Some("n2"), &|x| f.get(x).cloned(), &|x| s.get(x).copied());
        assert_eq!(p.layers, ["iyakki/patches/p2/layer"]);
        assert!(p.skipped[0].1.contains("obsolete"));
        // the seed root has no id: only patches for any root
        assert_eq!(plan_patches("p2\np1", 0, None, &|x| f.get(x).cloned(), &|x| s.get(x).copied()).layers.len(), 1);
        assert_eq!(root_id_of("iyakki/root/n1/tree"), Some("n1"));
        assert_eq!(root_id_of("boot/root.sfs"), None);
    }

    #[test]
    fn a_corrupt_or_incomplete_patch_is_skipped_not_fatal() {
        let (f, s) = vol(
            &[
                ("iyakki/patches/good/layer.txt", layer_txt("good", "*", &["file 3 /etc/a"])),
                ("iyakki/patches/bad/layer.txt", "garbage {".to_string()),
                ("iyakki/patches/short/layer.txt", layer_txt("short", "*", &["file 100 /etc/b"])),
                ("iyakki/patches/other/layer.txt", layer_txt("not-other", "*", &[])),
                ("iyakki/patches/evil/layer.txt", layer_txt("evil", "*", &["file 1 /../../etc/shadow"])),
            ],
            &[("iyakki/patches/good/layer/etc/a", 3), ("iyakki/patches/short/layer/etc/b", 99)],
        );
        let p = plan_patches("bad\nshort\ngone\nother\nevil\ngood", 0, Some("n1"), &|x| f.get(x).cloned(), &|x| s.get(x).copied());
        assert_eq!(p.layers, ["iyakki/patches/good/layer"]);
        assert_eq!(p.skipped.iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(), ["bad", "short", "gone", "other", "evil"]);
        assert!(p.skipped[1].1.contains("wrong size"));
        assert_eq!(parse_layer_info("iyakki-patch-layer 2\nid=a\nbase=*\n"), None);
        assert_eq!(parse_layer_info("iyakki-patch-layer 1\nid=a\n"), None);
        assert_eq!(parse_layer_info("iyakki-patch-layer 1\nid=a\nbase=*\nfile x /a\n"), None);
        assert_eq!(parse_layer_info("iyakki-patch-layer 1\nid=a\nbase=*\nfile 1 /a,b\n"), None);
    }

    #[test]
    fn repeated_failed_starts_switch_the_patches_off() {
        let (f, s) = vol(&[("iyakki/patches/p/layer.txt", layer_txt("p", "*", &["file 1 /etc/a"]))], &[("iyakki/patches/p/layer/etc/a", 1)]);
        let go = |n| plan_patches("p", n, Some("n1"), &|x| f.get(x).cloned(), &|x| s.get(x).copied());
        assert_eq!(go(0).layers.len(), 1);
        assert_eq!(go(PATCH_GUARD_MAX - 1).layers.len(), 1);
        let g = go(PATCH_GUARD_MAX);
        assert!(g.guard && g.layers.is_empty() && g.skipped.len() == 1);
        assert!(go(99).guard);
        assert_eq!((parse_count("2\n"), parse_count("x"), parse_count("")), (2, 0, 0));
    }
}

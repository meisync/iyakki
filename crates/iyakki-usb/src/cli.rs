//! Command-line options, finding the image, and deciding which drives to offer.

use std::path::{Path, PathBuf};

use crate::drives::{verdict, Drive};
use crate::ui::Item;

pub const HELP: &str = "\
iyakki-usb - write an IYAKKI image to a USB stick

USAGE:
  sudo iyakki-usb [IMAGE]            choose the drive with the arrow keys, confirm, write, verify
  iyakki-usb --list [IMAGE]          show the drives and whether each may be written (no root needed)

IMAGE is the .iso (PC) or .img (VM) file. Without it, an image in the current folder is used.

OPTIONS:
  --install          install IYAKKI on the stick: a real partitioned system with a FAT boot partition and a data
                     partition that fills the rest of the stick, bootable on legacy BIOS and UEFI PCs
  --table <t>        with --install: the partition table, auto (default), gpt or mbr.
                       gpt   BIOS boot partition + EFI System Partition (FAT32) + data partition filling the rest, primary
                             and backup tables; boots on legacy BIOS and UEFI; any size (the data partition may pass
                             2 TiB); 512 and 4096-byte sectors (4096: UEFI only)
                       mbr   the classic table, bootable on any old PC; the data partition ends at 2 TiB; 512-byte sectors
                       auto  gpt when the drive is over 2 TiB, has 4096-byte sectors, or this computer was started through
                             UEFI; mbr otherwise. (A stick for an old BIOS-only PC made on a UEFI computer: add --table mbr
                             if its firmware does not take GPT; most do.)
  --iso              copy the live ISO instead (read-only, loses changes at power-off unless a data disk exists)
                     (with both images available and neither flag, you are asked)
  --erase <drive>    no menu: write to this drive (for example /dev/sdb or disk4). Naming it is the confirmation,
                     so double-check it. The warning is still printed.
  --all              also offer internal disks (never the one holding the running system)
  --manifest <file>  check the image against this release.json (its sha256) before writing; a release.json or
                     <image>.sha256 next to the image is used automatically (check the signature with: iyakki release verify)
  --no-verify        skip reading the stick back to check it (not recommended)
  --no-zero-tail     do not wipe the last MiB of the stick
  --sector-size <n>  testing, with --target-file: pretend the file is a drive with n-byte logical sectors (512 or 4096)
  -h, --help         this text
  -V, --version      version

SAFETY: the system disk, any disk with the running system mounted, write-protected disks, swap and the disk the
image file lives on are always refused. Only the drive you pick is touched.
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub image: Option<PathBuf>,
    pub list: bool,
    pub all: bool,
    pub erase: Option<String>,
    pub no_verify: bool,
    pub no_zero_tail: bool,
    /// Write the install layout (partitioned, boots from the stick and keeps data) / copy the live ISO.
    pub install: bool,
    pub iso: bool,
    /// Testing: pretend this regular file is a drive (never offered otherwise).
    pub target_file: Option<PathBuf>,
    /// `--install`: `auto`, `gpt` or `mbr`.
    pub table: Option<String>,
    /// Testing: the logical sector size a `--target-file` pretends to have.
    pub sector_size: Option<u32>,
    /// A release manifest (`release.json`) to check the image's SHA-256 against before writing.
    pub manifest: Option<PathBuf>,
    pub help: bool,
    pub version: bool,
}

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => o.help = true,
            "-V" | "--version" => o.version = true,
            "--list" => o.list = true,
            "--all" => o.all = true,
            "--no-verify" => o.no_verify = true,
            "--no-zero-tail" => o.no_zero_tail = true,
            "--install" => o.install = true,
            "--iso" => o.iso = true,
            "--erase" => o.erase = Some(it.next().ok_or("--erase needs a drive, for example --erase /dev/sdb")?.clone()),
            "--manifest" => o.manifest = Some(PathBuf::from(it.next().ok_or("--manifest needs the path of release.json")?)),
            "--table" => {
                let t = it.next().ok_or("--table needs auto, gpt or mbr")?;
                crate::gpt_install::TableChoice::parse(t)?;
                o.table = Some(t.clone());
            }
            "--sector-size" => {
                let n: u32 = it.next().and_then(|v| v.parse().ok()).filter(|n| matches!(n, 512 | 4096)).ok_or("--sector-size needs 512 or 4096")?;
                o.sector_size = Some(n);
            }
            "--target-file" => o.target_file = Some(PathBuf::from(it.next().ok_or("--target-file needs a path")?)),
            s if s.starts_with('-') => return Err(format!("unknown option {s} (see --help)")),
            s => {
                if o.image.replace(PathBuf::from(s)).is_some() {
                    return Err("only one image file can be given".into());
                }
            }
        }
    }
    Ok(o)
}

/// What to write.
#[derive(Debug, PartialEq, Eq)]
pub enum Pick {
    Image(PathBuf),
    /// Both an install template and a live ISO are available: the user chooses.
    AskMode { install: PathBuf, iso: PathBuf },
}

fn candidates(cwd: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut templates, mut isos) = (Vec::new(), Vec::new());
    for dir in [cwd.to_path_buf(), cwd.join("target/buildroot-libre/images")] {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !e.path().is_file() {
                continue;
            }
            if name.starts_with("iyakki") && name.contains("install") && name.ends_with(".img") {
                templates.push(e.path());
            } else if name.ends_with(".iso") || name.ends_with(".iso9660") || (name.starts_with("iyakki") && name.ends_with(".img")) {
                isos.push(e.path());
            }
        }
    }
    templates.sort();
    isos.sort();
    // the build also leaves the plain CD image (`rootfs.iso9660`, no boot sector for USB); the hybrid `iyakki-*` wins
    if isos.iter().any(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("iyakki"))) {
        isos.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("iyakki")));
    }
    (templates, isos)
}

fn one(mut v: Vec<PathBuf>, what: &str) -> Result<Option<PathBuf>, String> {
    match v.len() {
        0 => Ok(None),
        1 => Ok(v.pop()),
        _ => Err(format!("several {what} found ({}); name the one to write", v.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>().join(", "))),
    }
}

/// The image to write: the one named, or what is found near the current folder (`--install` / `--iso` choose
/// between an install template and a live ISO; with both present and no flag, the user is asked).
pub fn pick_image(explicit: Option<&Path>, cwd: &Path, install: bool, iso: bool) -> Result<Pick, String> {
    if install && iso {
        return Err("choose either --install or --iso, not both".into());
    }
    if let Some(p) = explicit {
        return if p.is_file() { Ok(Pick::Image(p.to_path_buf())) } else { Err(format!("{} does not exist or is not a file", p.display())) };
    }
    let (templates, isos) = candidates(cwd);
    // only look at the kind that was asked for, so an ambiguity in the other kind does not matter
    if install {
        return one(templates, "install images")?.map(Pick::Image).ok_or_else(|| "no install image (iyakki-install-*.img) found. Build one, or name it".to_string());
    }
    if iso {
        return one(isos, "images")?.map(Pick::Image).ok_or_else(|| "no ISO image found. Build one, or name it".to_string());
    }
    match (one(templates, "install images")?, one(isos, "images")?) {
        (Some(t), Some(i)) => Ok(Pick::AskMode { install: t, iso: i }),
        (Some(x), None) | (None, Some(x)) => Ok(Pick::Image(x)),
        (None, None) => Err("no image found here. Name it: iyakki-usb path/to/image".into()),
    }
}

/// The drives to show: USB/removable ones always, internal ones only with `--all`.
pub fn items_for(drives: &[Drive], image: &Path, all: bool) -> Vec<Item> {
    drives.iter().filter(|d| all || d.usb || d.removable).map(|d| Item { drive: d.clone(), verdict: verdict(d, image) }).collect()
}

/// Match `--erase` against a drive by short name or path (`sdb`, `/dev/sdb`, `disk4`, `/dev/rdisk4`).
pub fn find_drive<'a>(drives: &'a [Item], wanted: &str) -> Option<&'a Item> {
    let w = wanted.trim();
    drives.iter().find(|i| i.drive.name == w || i.drive.path == w || w.strip_prefix("/dev/").is_some_and(|n| n == i.drive.name) || w.strip_prefix("/dev/r").is_some_and(|n| n == i.drive.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drives::Verdict;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn options_are_parsed_and_mistakes_explained() {
        let o = parse_args(&s(&["iyakki.iso", "--all", "--no-verify"])).unwrap();
        assert_eq!((o.image.as_deref(), o.all, o.no_verify, o.list), (Some(Path::new("iyakki.iso")), true, true, false));
        assert_eq!(parse_args(&s(&["--erase", "/dev/sdb"])).unwrap().erase.as_deref(), Some("/dev/sdb"));
        assert!(parse_args(&s(&["--list"])).unwrap().list);
        assert!(parse_args(&s(&["-h"])).unwrap().help && parse_args(&s(&["--version"])).unwrap().version);
        assert!(parse_args(&s(&["--erase"])).unwrap_err().contains("needs a drive"));
        assert!(parse_args(&s(&["--frobnicate"])).unwrap_err().contains("unknown option"));
        assert!(parse_args(&s(&["a.iso", "b.iso"])).unwrap_err().contains("only one image"));
        assert_eq!(parse_args(&[]).unwrap(), Options::default());
        assert!(HELP.contains("--erase") && HELP.contains("SAFETY") && HELP.contains("--install"));
        let o = parse_args(&s(&["--install"])).unwrap();
        assert!(o.install && !o.iso);
    }

    #[test]
    fn the_image_is_found_or_the_choice_is_explained() {
        let d = std::env::temp_dir().join(format!("iyakki-usb-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("target/buildroot-libre/images")).unwrap();
        let w = |p: &str| std::fs::write(d.join(p), b"x").unwrap();
        let pick = |e: Option<&Path>, i: bool, s: bool| pick_image(e, &d, i, s);
        assert!(pick(None, false, false).unwrap_err().contains("no image found"));
        w("target/buildroot-libre/images/rootfs.iso9660");
        assert!(matches!(pick(None, false, false), Ok(Pick::Image(p)) if p.ends_with("rootfs.iso9660")), "the build output folder is searched");
        w("target/buildroot-libre/images/iyakki-x86_64.iso");
        assert!(matches!(pick(None, false, false), Ok(Pick::Image(p)) if p.ends_with("iyakki-x86_64.iso")), "the hybrid iyakki*.iso beats the plain CD image");
        w("target/buildroot-libre/images/iyakki-install-x86_64.img");
        // both an install template and an ISO: ask, unless a flag decides
        assert!(matches!(pick(None, false, false), Ok(Pick::AskMode { install, iso }) if install.ends_with("iyakki-install-x86_64.img") && iso.ends_with("iyakki-x86_64.iso")));
        assert!(matches!(pick(None, true, false), Ok(Pick::Image(p)) if p.ends_with("iyakki-install-x86_64.img")));
        assert!(matches!(pick(None, false, true), Ok(Pick::Image(p)) if p.ends_with("iyakki-x86_64.iso")));
        assert!(pick(None, true, true).unwrap_err().contains("not both"));
        w("iyakki-other.iso");
        assert!(pick(None, false, false).unwrap_err().contains("several images"));
        assert_eq!(pick(Some(&d.join("iyakki-other.iso")), false, false).unwrap(), Pick::Image(d.join("iyakki-other.iso")), "a named file wins");
        assert!(pick(Some(&d.join("missing.iso")), false, false).unwrap_err().contains("does not exist"));
        assert!(pick(Some(&d), false, false).is_err(), "a folder is not an image");
        std::fs::remove_file(d.join("target/buildroot-libre/images/iyakki-install-x86_64.img")).unwrap();
        assert!(pick(None, true, false).unwrap_err().contains("no install image"));
    }

    fn drive(name: &str, usb: bool, mounted: Option<&str>) -> Drive {
        Drive { path: format!("/dev/{name}"), name: name.into(), model: "m".into(), size_bytes: 8_000_000_000, removable: usb, usb, read_only: false, mounts: mounted.map(|m| vec![(format!("/dev/{name}2"), m.to_string())]).unwrap_or_default(), swap: false }
    }

    #[test]
    fn internal_disks_are_hidden_unless_asked_for_and_the_system_disk_is_always_refused() {
        let drives = vec![drive("nvme0n1", false, Some("/")), drive("sdb", true, None), drive("sdc", false, None)];
        let img = Path::new("/tmp/a.iso");
        let normal = items_for(&drives, img, false);
        assert_eq!(normal.iter().map(|i| i.drive.name.as_str()).collect::<Vec<_>>(), ["sdb"]);
        let all = items_for(&drives, img, true);
        assert_eq!(all.len(), 3);
        assert!(matches!(&all[0].verdict, Verdict::Refused(_)), "the system disk is refused even with --all");
        assert_eq!(all[2].verdict, Verdict::Internal);
    }

    #[test]
    fn a_drive_can_be_named_by_short_name_or_path() {
        let img = Path::new("/tmp/a.iso");
        let items = items_for(&[drive("sdb", true, None), drive("disk4", true, None)], img, false);
        for w in ["sdb", "/dev/sdb", " sdb "] {
            assert_eq!(find_drive(&items, w).unwrap().drive.name, "sdb", "{w}");
        }
        assert_eq!(find_drive(&items, "/dev/rdisk4").unwrap().drive.name, "disk4");
        assert!(find_drive(&items, "sda").is_none() && find_drive(&items, "").is_none() && find_drive(&items, "/dev/sdb1").is_none());
    }
}

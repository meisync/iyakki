use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use iyakki_usb::cli::{find_drive, items_for, parse_args, pick_image, Pick, HELP};
use iyakki_usb::drives::{list_drives, reread_partitions, Drive, Verdict};
use iyakki_usb::term::{width, Raw};
use iyakki_usb::install::{self, TableKind};
use iyakki_usb::ui::{self, confirmed, install_layout_lines, mode_menu_key, render_menu, render_mode_menu, render_progress, render_warning, Item, Key, Menu, MenuOutcome, BOLD, GREEN, RED, RESET, YELLOW};
use iyakki_usb::writer::{self, ImageKind, Phase, Progress};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{RED}iyakki-usb: {e}{RESET}");
            ExitCode::FAILURE
        }
    }
}

/// A progress printer that redraws one line at most ten times a second.
struct Printer {
    last: Instant,
    width: usize,
}

impl Printer {
    fn new() -> Printer {
        Printer { last: Instant::now() - Duration::from_secs(1), width: width() }
    }
    fn show(&mut self, p: Progress) {
        let finished = p.done >= p.total;
        if !finished && self.last.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last = Instant::now();
        print!("\r{}\x1b[K", render_progress(p.phase, p.done, p.total, p.elapsed, self.width));
        let _ = std::io::stdout().flush();
    }
    fn done(&self) {
        println!();
    }
}

fn pick_interactively(items: Vec<Item>, image_name: &str, image_size: u64) -> Result<Option<Item>, String> {
    let raw = Raw::enter().ok_or("the menu needs a terminal. Use --erase <drive> to skip it.")?;
    let mut menu = Menu::new(items);
    let mut note: Option<String> = None;
    loop {
        print!("{}", render_menu(&menu, image_name, image_size, note.as_deref()));
        let _ = std::io::stdout().flush();
        for k in ui::parse_keys(&raw.read_bytes()) {
            match menu.handle(k) {
                MenuOutcome::Continue => note = None,
                MenuOutcome::Blocked(why) => note = Some(format!("That drive cannot be used: {why}.")),
                MenuOutcome::Quit => {
                    drop(raw);
                    println!();
                    return Ok(None);
                }
                MenuOutcome::Chosen(i) => {
                    let chosen = menu.items[i].clone();
                    drop(raw);
                    return Ok(Some(chosen));
                }
            }
            if matches!(k, Key::Quit) {
                break;
            }
        }
    }
}

/// "Install" or "copy the ISO": `Some(0)` install, `Some(1)` ISO, `None` quit.
fn choose_mode() -> Result<Option<usize>, String> {
    let raw = Raw::enter().ok_or("the menu needs a terminal. Use --install or --iso to skip it.")?;
    let mut sel = 0usize;
    loop {
        print!("{}", render_mode_menu(sel));
        let _ = std::io::stdout().flush();
        for k in ui::parse_keys(&raw.read_bytes()) {
            if let Some(r) = mode_menu_key(&mut sel, k) {
                return Ok(r);
            }
        }
    }
}

fn random_signature() -> [u8; 4] {
    let mut b = [0u8; 4];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).is_ok();
    if !ok || b == [0; 4] {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
        b = t.to_le_bytes();
    }
    b
}

/// Free the drive: unmount everything on it (Linux: umount; macOS: diskutil).
fn unmount_all(d: &Drive) -> Result<(), String> {
    for (_, mp) in d.mounts.iter().rev() {
        #[cfg(target_os = "macos")]
        {
            let _ = mp;
            return iyakki_usb::macos::unmount_disk(&d.name);
        }
        #[cfg(not(target_os = "macos"))]
        iyakki_storage::SysMounter.unmount(std::path::Path::new(mp)).map_err(|e| format!("could not unmount {mp}: {e} (close anything using it and try again)"))?;
    }
    Ok(())
}

use iyakki_storage::Mounter as _;

fn run() -> Result<ExitCode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse_args(&args)?;
    if o.help {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    if o.version {
        println!("iyakki-usb {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let image = match pick_image(o.image.as_deref(), &cwd, o.install, o.iso)? {
        Pick::Image(p) => p,
        Pick::AskMode { install, iso } => {
            if o.list {
                install
            } else {
                match choose_mode()? {
                    Some(0) => install,
                    Some(_) => iso,
                    None => {
                        println!("Cancelled. Nothing was written.");
                        return Ok(ExitCode::from(2));
                    }
                }
            }
        }
    };
    let image = std::fs::canonicalize(&image).unwrap_or(image);
    let info = writer::inspect_image(&image)?;
    let image_name = image.file_name().map_or_else(|| image.display().to_string(), |n| n.to_string_lossy().into_owned());
    if info.kind == ImageKind::Unknown {
        return Err(format!("{image_name} does not look like a bootable image (no ISO or disk header). Nothing was written."));
    }
    // the drives (a regular file stands in for one when testing)
    let items: Vec<Item> = match &o.target_file {
        Some(f) => {
            let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0).max(info.size + 1024 * 1024);
            vec![Item { drive: Drive { path: f.display().to_string(), name: "testfile".into(), model: "test file".into(), size_bytes: size, removable: true, usb: true, read_only: false, mounts: vec![], swap: false }, verdict: Verdict::Ok }]
        }
        None => items_for(&list_drives()?, &image, o.all),
    };
    if o.list {
        println!("Image: {image_name} ({}, {})", ui::human_size(info.size), match info.kind { ImageKind::Iso9660 => "ISO", ImageKind::InstallTemplate => "install template", _ => "disk image" });
        if items.is_empty() {
            println!("No USB drives found.");
        }
        for i in &items {
            let status = match &i.verdict {
                Verdict::Ok => format!("{GREEN}ok{RESET}"),
                Verdict::Internal => format!("{YELLOW}internal disk (needs --all){RESET}"),
                Verdict::Refused(r) => format!("{RED}refused: {r}{RESET}"),
            };
            println!("{:<12} {:>9}  {:<26} {status}", i.drive.path, ui::human_size(i.drive.size_bytes), i.drive.model);
        }
        return Ok(ExitCode::SUCCESS);
    }
    // writing a real drive needs administrator rights
    // SAFETY: geteuid takes no arguments.
    if o.target_file.is_none() && unsafe { libc::geteuid() } != 0 {
        return Err("writing to a drive needs administrator rights: run it again with sudo".into());
    }
    // integrity of the download, when a checksum file sits next to the image
    // (<image>.sha256, or release.json from the release folder / --manifest FILE)
    let sidecar = writer::read_sidecar(&image).map(|s| (s, format!("{image_name}.sha256")));
    let from_manifest = || writer::read_manifest_sum(&image, o.manifest.as_deref()).map(|s| (s, "release.json".to_string()));
    if o.manifest.is_some() && sidecar.is_none() && from_manifest().is_none() {
        return Err(format!("{image_name} is not listed in the manifest you gave (--manifest). Nothing was written."));
    }
    if let Some((expected, source)) = sidecar.or_else(from_manifest) {
        println!("Checking {image_name} against {source} …");
        let mut p = Printer::new();
        let got = writer::file_sha256(&image, &mut |pr| p.show(pr))?;
        p.done();
        if got != expected {
            return Err(format!("the image does not match its checksum ({source}; the download is damaged or was altered). Nothing was written.\n  expected {expected}\n  found    {got}"));
        }
        println!("{GREEN}Checksum OK.{RESET}\n");
    }
    let chosen: Item = match &o.erase {
        Some(wanted) => {
            let i = find_drive(&items, wanted).ok_or_else(|| format!("{wanted} is not a drive this tool offers (try --list{})", if o.all { "" } else { ", or --all for internal disks" }))?;
            i.clone()
        }
        None => match pick_interactively(items, &image_name, info.size)? {
            Some(i) => i,
            None => {
                println!("Cancelled. Nothing was written.");
                return Ok(ExitCode::from(2));
            }
        },
    };
    match &chosen.verdict {
        Verdict::Refused(r) => return Err(format!("{} cannot be used: {r}. Nothing was written.", chosen.drive.path)),
        Verdict::Internal if !o.all => return Err(format!("{} is an internal disk. Add --all if you really mean it. Nothing was written.", chosen.drive.path)),
        _ => {}
    }
    let d = &chosen.drive;
    if d.size_bytes < info.size {
        return Err(format!("{} holds {} but the image is {}: use a bigger stick.", d.path, ui::human_size(d.size_bytes), ui::human_size(info.size)));
    }
    // the warning, and the confirmation: typing the name (or having named it with --erase)
    let installing = info.kind == ImageKind::InstallTemplate;
    if o.table.is_some() && !installing {
        return Err("--table applies to --install (the install image); nothing was written".into());
    }
    let sector = o.sector_size.filter(|_| o.target_file.is_some()).unwrap_or_else(|| iyakki_usb::drives::logical_sector(d));
    let table_choice = install::TableChoice::parse(o.table.as_deref().unwrap_or("auto"))?;
    let (kind, why) = if installing { install::resolve_table(table_choice, d.size_bytes, sector, install::booted_uefi()).map_err(|e| format!("{}: {e}. Nothing was written.", d.path))? } else { (TableKind::Mbr, String::new()) };
    let source = if installing && kind == TableKind::Gpt { Some(install::read_source(&image)?) } else { None };
    let gpt_plan = match &source {
        Some(src) => Some(install::plan_stick_gpt(d, sector, src, &why).map_err(|e| format!("{e}. Nothing was written."))?),
        None => None,
    };
    let layout = installing.then(|| match &gpt_plan {
        Some(p) => {
            let mut v = vec![format!("{} partition table ({why}), {sector}-byte sectors", kind.label())];
            v.extend(p.layout_lines());
            v.push(if sector == 512 { "Boots on legacy BIOS PCs (GRUB in the BIOS boot partition) and on UEFI PCs.".to_string() } else { "Boots on UEFI PCs (4096-byte sectors: no legacy BIOS boot).".to_string() });
            v
        }
        None => {
            let mut v = vec![format!("MBR partition table ({why})")];
            v.extend(install_layout_lines(d.size_bytes, 256 * 1024 * 1024));
            v
        }
    });
    print!("{}", render_warning(d, &chosen.verdict, &image_name, info.size, layout.as_deref()));
    let _ = std::io::stdout().flush();
    if o.erase.is_some() {
        println!("\n  (confirmed on the command line with --erase {})", d.path);
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        if !confirmed(&line, d) {
            println!("\nCancelled. Nothing was written.");
            return Ok(ExitCode::from(2));
        }
    }
    println!();
    unmount_all(d)?;
    let target = PathBuf::from(&d.path);
    let mut p = Printer::new();
    // installing: the data partition is stretched to fill the stick while the boot sector goes by
    if let Some(plan) = &gpt_plan {
        let report = install::execute(plan, false, "", &mut |pr| p.show(pr)).map_err(|e| format!("{e}\nThe drive may be half-written; do not boot it."))?;
        p.done();
        reread_partitions(&d.path);
        println!("\n{BOLD}{GREEN}Done.{RESET} {} written to {} and verified (both copies of the GPT checked). It is safe to remove the stick.", ui::human_size(report.written), d.path);
        println!("IYAKKI is installed. The first boot formats the data partition (a few seconds); after that your accounts, settings and files are kept on the stick.");
        println!("Boot it: pick the stick in your computer's boot menu (usually F12, F10 or Esc at power-on). Secure Boot must be off for now.");
        return Ok(ExitCode::SUCCESS);
    }
    let (size, sig) = (d.size_bytes, random_signature());
    let patch = move |off: u64, buf: &mut [u8]| -> Result<(), String> {
        if off == 0 { writer::patch_install_mbr(&mut buf[..512], size, sig).map(|_| ()) } else { Ok(()) }
    };
    let patcher: Option<writer::Patch> = installing.then_some(&patch);
    let report = writer::write_image(&image, &target, Some(d.size_bytes), patcher, &mut |pr| p.show(pr)).map_err(|e| format!("{e}\nThe drive may be half-written; do not boot it."))?;
    p.done();
    if o.target_file.is_none() && !o.no_zero_tail {
        writer::zero_tail(&target, d.size_bytes, 1024 * 1024).map_err(|e| format!("could not wipe the end of the drive: {e}"))?;
    }
    if o.no_verify {
        println!("{YELLOW}Skipped verification (--no-verify).{RESET}");
    } else {
        let mut p = Printer::new();
        writer::verify(&target, report.bytes, &report.sha256, &mut |pr: Progress| p.show(Progress { phase: Phase::Verifying, ..pr })).map_err(|e| format!("{e}\nDo not boot this drive."))?;
        p.done();
    }
    reread_partitions(&d.path);
    println!("\n{BOLD}{GREEN}Done.{RESET} {} written to {}{} It is safe to remove the stick.", ui::human_size(report.bytes), d.path, if o.no_verify { "." } else { " and verified." });
    if installing {
        println!("IYAKKI is installed. The first boot formats the data partition (a few seconds); after that your accounts, settings and files are kept on the stick.");
    }
    println!("Boot it: pick the stick in your computer's boot menu (usually F12, F10 or Esc at power-on). Secure Boot must be off for now.");
    Ok(ExitCode::SUCCESS)
}

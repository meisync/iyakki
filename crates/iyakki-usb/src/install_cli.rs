//! `iyakki install`: the text-mode installer ("install to this computer"). The shell's SYSTEM > Install to disk screen
//! uses the same library calls ([`crate::install`]).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::drives::{self, Verdict};
use crate::install::{self, Choices, DiskInfo, TableChoice};
use crate::ui::human_size;
use crate::writer::{Phase, Progress};

pub const HELP: &str = "\
iyakki install - install IYAKKI on this computer's disk (run from the IYAKKI USB stick)

USAGE:
  iyakki install                    list the disks, ask which one, ask a few questions, confirm, install
  iyakki install --list             list the disks and which may be used
  iyakki install --dry-run ...      show the plan and write NOTHING

OPTIONS:
  --disk <name>        the disk to erase, for example sda, nvme0n1, mmcblk0 (asked when missing)
  --confirm <name>     with --yes: repeat the disk name to confirm without the prompts
  --yes                do not ask (needs --disk and --confirm)
  --zram <v>           auto (default), off, 4G, 50%, 4G,zstd
  --swapfile <v>       off (default) or a size such as 2G, kept on the data partition
  --hostname <name>    the machine's name
  --table <t>          partition table: auto (default), gpt or mbr.
                         gpt: BIOS boot partition + EFI System Partition + data; boots on legacy BIOS and UEFI; any disk
                              size; 512 and 4096-byte sectors (4096: UEFI only)
                         mbr: the classic table; data partition up to 2 TiB; 512-byte sectors; boots on any old PC
                         auto: gpt for disks over 2 TiB, disks with 4096-byte sectors and PCs started through UEFI,
                               mbr otherwise
  --source <disk>      the IYAKKI boot disk to copy from (default: found automatically)
  --template <file>    copy from an install template file (iyakki-install-x86_64.img) instead
  --free-space         install next to another system: same as --plan --mode free-space
  --plan ...           the partition planner: free space, chosen partitions, dual boot with the existing EFI System
                       Partition, a plan to confirm, --plan-file, --dry-run (iyakki install --plan --help)
  --dry-run            print the plan, write nothing
  -h, --help           this text

The whole disk is erased. The disk you booted from, disks with anything mounted, swap disks, write-protected disks,
disks under 4 GB and disks with sector sizes other than 512 and 4096 bytes are refused. The plan shows the table the
disk has now and the layout it gets. Details: docs/ZRAM-AND-INSTALL.md.
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Opts {
    pub list: bool,
    pub dry_run: bool,
    pub yes: bool,
    pub help: bool,
    pub free_space: bool,
    pub disk: Option<String>,
    pub confirm: Option<String>,
    pub zram: Option<String>,
    pub swapfile: Option<String>,
    pub hostname: Option<String>,
    pub table: Option<String>,
    pub source: Option<String>,
    pub template: Option<PathBuf>,
}

pub fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |what: &str| it.next().cloned().ok_or_else(|| format!("{a} needs {what}"));
        match a.as_str() {
            "-h" | "--help" => o.help = true,
            "--list" => o.list = true,
            "--dry-run" => o.dry_run = true,
            "--yes" | "-y" => o.yes = true,
            "--free-space" => o.free_space = true,
            "--disk" => o.disk = Some(val("a disk name")?),
            "--confirm" => o.confirm = Some(val("the disk name")?),
            "--zram" => o.zram = Some(val("auto, off or a size")?),
            "--swapfile" => o.swapfile = Some(val("off or a size")?),
            "--hostname" => o.hostname = Some(val("a name")?),
            "--table" => o.table = Some(val("auto, gpt or mbr")?),
            "--source" => o.source = Some(val("a disk name")?),
            "--template" => o.template = Some(PathBuf::from(val("a file")?)),
            s => return Err(format!("unknown option {s} (see --help)")),
        }
    }
    if let Some(t) = &o.table {
        TableChoice::parse(t)?;
    }
    if o.yes && (o.disk.is_none() || o.confirm.is_none()) && !o.dry_run {
        return Err("--yes needs --disk and --confirm <the same disk name>".into());
    }
    Ok(o)
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut l = String::new();
    let _ = std::io::stdin().lock().read_line(&mut l);
    l.trim().to_string()
}

/// One row per disk for the list: name, size, model, serial, and why not if refused.
pub fn table(disks: &[DiskInfo], source: &Path) -> String {
    let mut s = String::new();
    for d in disks {
        let v = install_verdict(d, source);
        let state = match v {
            Ok(()) => "ok".to_string(),
            Err(r) => format!("refused: {r}"),
        };
        let sectors = if d.logical_sector == 512 { String::new() } else { format!(" {}-byte sectors", d.logical_sector) };
        let table = if d.table.is_empty() || d.table == "none" { String::new() } else { format!(" [{}]", d.table) };
        s.push_str(&format!("{:<10} {:>9}  {:<26} {:<14} {state}{sectors}{table}\n", d.drive.name, human_size(d.drive.size_bytes), d.drive.model, d.serial));
        for (n, b, what) in &d.partitions {
            s.push_str(&format!("    {n:<14} {:>9}  {what}\n", human_size(*b)));
        }
    }
    s
}

fn install_verdict(d: &DiskInfo, source: &Path) -> Result<(), String> {
    if let Verdict::Refused(r) = drives::verdict(&d.drive, source) {
        return Err(r);
    }
    if let Some((dev, mp)) = d.drive.mounts.first() {
        return Err(format!("{dev} is mounted at {mp}"));
    }
    if source.file_name() == Some(std::ffi::OsStr::new(&d.drive.name)) {
        return Err("the disk IYAKKI is copied from".into());
    }
    if !matches!(d.logical_sector, 512 | 4096) {
        return Err(format!("{}-byte sectors are not supported", d.logical_sector));
    }
    if d.drive.size_bytes < install::MIN_DISK {
        return Err("smaller than 4 GB".into());
    }
    Ok(())
}

fn progress_printer() -> impl FnMut(Progress) {
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    move |p: Progress| {
        if p.done < p.total && last.elapsed() < std::time::Duration::from_millis(250) {
            return;
        }
        last = std::time::Instant::now();
        let pct = if p.total == 0 { 100 } else { p.done * 100 / p.total };
        let label = match p.phase {
            Phase::Checking => "Checking",
            Phase::Writing => "Writing",
            Phase::Syncing => "Flushing",
            Phase::Verifying => "Verifying",
        };
        print!("\r{label:<10} {pct:>3}%  {}  ", crate::ui::human_duration(p.elapsed));
        let _ = std::io::stdout().flush();
    }
}

/// The installer. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("iyakki install: {e}");
            1
        }
    }
}

fn run_inner(args: &[String]) -> Result<i32, String> {
    // the partition planner takes over for everything but the classic whole-disk questions
    if args.iter().any(|a| matches!(a.as_str(), "--plan" | "--free-space" | "--mode" | "--apply-plan")) {
        let mapped: Vec<String> = args.iter().flat_map(|a| if a == "--free-space" { vec!["--mode".to_string(), "free-space".to_string()] } else { vec![a.clone()] }).collect();
        return Ok(crate::plan_cli::run(&mapped));
    }
    let o = parse(args)?;
    if o.help {
        print!("{HELP}");
        return Ok(0);
    }
    let (sys, dev) = (Path::new("/sys"), Path::new("/dev"));
    let mounts = iyakki_storage::read_mounts().unwrap_or_default();
    let swaps = iyakki_storage::read_swaps().unwrap_or_default();
    let mut disks = install::list_disks(sys, dev, &mounts, &swaps);
    install::probe_contents(&mut disks, dev);
    let source = match (&o.template, &o.source) {
        (Some(t), _) => install::read_source(t)?,
        (None, Some(s)) => install::read_source(&dev.join(s.trim_start_matches("/dev/")))?,
        (None, None) => install::find_source(&disks, dev, o.disk.as_deref())?,
    };
    println!("Copying from: {}\n", source.path.display());
    let candidates: Vec<DiskInfo> = disks.into_iter().filter(|d| source.path.file_name() != Some(std::ffi::OsStr::new(&d.drive.name))).collect();
    println!("Disks in this computer:\n{}", table(&candidates, &source.path));
    if o.list {
        return Ok(0);
    }
    let interactive = !o.yes;
    let target = match &o.disk {
        Some(d) => d.clone(),
        None => {
            let t = ask("Disk to install on (its name, e.g. sda; Enter to cancel): ");
            if t.is_empty() {
                println!("Cancelled. Nothing was written.");
                return Ok(2);
            }
            t
        }
    };
    let mut choices = Choices { zram: o.zram.clone().unwrap_or_else(|| "auto".into()), swapfile: o.swapfile.clone().unwrap_or_else(|| "off".into()), hostname: o.hostname.clone() };
    if interactive && o.disk.is_none() {
        if o.zram.is_none() {
            let v = ask("Compressed memory (zram): Enter = automatic (recommended), off, or a size like 4G: ");
            if !v.is_empty() {
                choices.zram = v;
            }
        }
        if o.swapfile.is_none() {
            let v = ask("Swap file on the disk (off, or a size like 2G; Enter = off): ");
            if !v.is_empty() {
                choices.swapfile = v;
            }
        }
        if o.hostname.is_none() {
            let v = ask("Name of this computer (Enter = keep the default): ");
            if !v.is_empty() {
                choices.hostname = Some(v);
            }
        }
    }
    let table = TableChoice::parse(o.table.as_deref().unwrap_or("auto"))?;
    let plan = install::make_plan_with(&candidates, &target, &source, &choices, table, install::booted_uefi())?;
    println!("\n{}", plan.render());
    if o.dry_run {
        install::execute(&plan, true, &mounts, &mut |_| {})?;
        println!("\nDry run: nothing was written.");
        return Ok(0);
    }
    // SAFETY: geteuid takes no arguments.
    if unsafe { libc::geteuid() } != 0 {
        return Err("installing needs administrator rights: run it as root (or use --dry-run to see the plan)".into());
    }
    let d = &plan.target.drive;
    if o.yes {
        if o.confirm.as_deref().map(|c| c.trim_start_matches("/dev/")) != Some(d.name.as_str()) {
            return Err(format!("--confirm must repeat the disk name ({}); nothing was written", d.name));
        }
    } else {
        println!("\nThis ERASES everything on {} ({}).", d.path, human_size(d.size_bytes));
        let name = ask(&format!("Type the drive name ({}) to erase it, or Enter to cancel: ", d.name));
        let size = if name.is_empty() { String::new() } else { ask(&format!("Type its size ({}) to confirm: ", human_size(d.size_bytes))) };
        if !install::confirmation_ok(&name, &size, d) {
            println!("Cancelled. Nothing was written.");
            return Ok(2);
        }
    }
    let mounts = iyakki_storage::read_mounts().unwrap_or_default();
    let mut p = progress_printer();
    let r = install::execute(&plan, false, &mounts, &mut p)?;
    println!("\n\nDone. {} written to {} and verified.", human_size(r.written), d.path);
    println!("Remove the USB stick and restart. The first start formats the data partition and asks you to create the first account.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn options() {
        let o = parse(&s(&["--disk", "sda", "--zram", "4G", "--swapfile", "2G", "--hostname", "box", "--dry-run", "--template", "t.img"])).unwrap();
        assert_eq!((o.disk.as_deref(), o.zram.as_deref(), o.swapfile.as_deref(), o.hostname.as_deref(), o.dry_run), (Some("sda"), Some("4G"), Some("2G"), Some("box"), true));
        assert_eq!(o.template.as_deref(), Some(Path::new("t.img")));
        assert!(parse(&s(&["--yes"])).unwrap_err().contains("--confirm"));
        assert!(parse(&s(&["--yes", "--disk", "sda", "--confirm", "sda"])).is_ok());
        assert!(parse(&s(&["--disk"])).unwrap_err().contains("needs"));
        assert!(parse(&s(&["--frob"])).unwrap_err().contains("unknown"));
        assert!(parse(&s(&["-h"])).unwrap().help);
        assert!(HELP.contains("--dry-run") && HELP.contains("--table") && HELP.contains("4096"));
        assert_eq!(parse(&s(&["--table", "gpt"])).unwrap().table.as_deref(), Some("gpt"));
        assert!(parse(&s(&["--table", "ntfs"])).unwrap_err().contains("auto, gpt or mbr"));
        assert!(parse(&s(&["--table"])).unwrap_err().contains("needs"));
    }

    #[test]
    fn free_space_now_belongs_to_the_planner() {
        assert!(HELP.contains("--free-space") && HELP.contains("--plan"));
        // the planner's own parser accepts what --free-space maps to
        let o = crate::plan_cli::parse(&s(&["--mode", "free-space", "--dry-run"])).unwrap();
        assert_eq!(o.mode.as_deref(), Some("free-space"));
    }

    #[test]
    fn the_table_says_why_a_disk_is_refused() {
        let mk = |name: &str, mounted: bool, sector: u32| DiskInfo {
            drive: drives::Drive { path: format!("/dev/{name}"), name: name.into(), model: "M".into(), size_bytes: 500_000_000_000, removable: false, usb: false, read_only: false, mounts: if mounted { vec![(format!("/dev/{name}1"), "/data".into())] } else { vec![] }, swap: false },
            serial: "S".into(),
            logical_sector: sector,
            partitions: vec![(format!("{name}1"), 1_000_000_000, "ext4".into())],
            gpt: false,
            table: String::new(),
        };
        let t = table(&[mk("sda", false, 512), mk("sdb", true, 512), mk("sdc", false, 1024), mk("sdd", false, 4096)], Path::new("/dev/sdz"));
        let line = |n: &str| t.lines().find(|l| l.starts_with(n)).unwrap().to_string();
        assert!(line("sda").ends_with("ok"));
        assert!(line("sdb").contains("mounted at /data"));
        assert!(line("sdc").contains("1024-byte sectors are not supported"));
        assert!(line("sdd").contains("ok") && line("sdd").contains("4096-byte sectors"), "4Kn disks are installable now: {}", line("sdd"));
        assert!(t.contains("sda1") && t.contains("ext4"));
    }
}

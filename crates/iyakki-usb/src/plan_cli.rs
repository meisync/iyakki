//! The command line of the partition planner: `iyakki-usb --plan ...` on a computer, `iyakki install --plan ...` on the booted
//! IYAKKI stick, and the boot-time install mode (`iyakki.install=1`) which runs [`run_boot`].
//!
//! * with `--disk` and `--mode` it plans without asking: prints the plan table, `--plan-file FILE` saves it as JSON,
//!   `--dry-run` stops there (nothing is written, no administrator rights needed);
//! * without them it opens the arrow-key pages ([`crate::tui`]);
//! * `--apply-plan FILE` is the privileged writer the window installer starts through `pkexec`/`sudo`: it re-reads the disk,
//!   rebuilds the plan, checks its digest and the typed confirmation stored in the file, and writes.

use std::path::{Path, PathBuf};

use crate::apply::{self, ApplyEnv};
use crate::drives;
use crate::gpt_install::TableChoice;
use crate::install::{self, DiskInfo, SourceInfo};
use crate::json::Json;
use crate::planner::{build_plan, check_confirmation, confirmation_ok, Confirmation, read_model, DiskMeta, DiskModel, Mode, Plan, Request};
use crate::ui::human_size;
use crate::wizard::Wizard;
use crate::writer::{Phase, Progress};

pub const HELP: &str = "\
iyakki-usb --plan - install IYAKKI next to other systems, or on a whole disk, with a plan you confirm first

USAGE:
  iyakki-usb --plan [IMAGE]                          arrow-key pages: drive, mode, partitions, plan, confirmation
  iyakki-usb --plan --list [--json] [IMAGE]          the drives with their partitions and free space (administrator rights read the tables)
  iyakki-usb --plan --disk sdb --mode free-space --dry-run [IMAGE]
                                                     print the plan and exit; nothing is written
  iyakki-usb --apply-plan FILE                       write a plan file made by --plan-file or the window installer (the writer)

OPTIONS:
  --disk <name>          the drive (sdb, nvme1n1, ...)
  --mode <m>             whole-disk | free-space | reuse | custom
  --gap-start <sector>   use the free space that starts at this sector (default: the largest)
  --boot-part <n>        reuse: the partition number that becomes BOOT (FAT32, formatted)
  --data-part <n>        reuse: the partition number that becomes DATA (formatted)
  --keep-esp             dual boot: keep the disk's EFI System Partition; add only EFI/IYAKKI and a boot entry
  --boot-mib <n>, --data-mib <n>    custom sizes
  --table <t>            auto | gpt | mbr (a new table; existing tables are kept)
  --confirm-running-disk the disk holds the running system: changing its free space is allowed after the typed confirmation
  --zram <v> --swapfile <v> --hostname <name>      first-boot settings
  --plan-file <file>     save the plan as JSON
  --dry-run              show the plan, write nothing
  --yes --confirm <name> write without the pages: repeat the drive name (needs administrator rights)
  --source <disk> | --template <file> | IMAGE      the IYAKKI install image to copy from (default: the booted stick)
  --no-boot-entry        do not add a UEFI boot entry
  --machine              progress as lines ('@progress phase done total') for the window installer

Rules: nothing mounted is touched; the disk the system runs from needs the typed confirmation; partitions that are not in the plan
are never written; existing filesystems are never shrunk (free space is what the disk already has). Details: docs/INSTALLER.md.
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Opts {
    pub help: bool,
    pub list: bool,
    pub json: bool,
    pub dry_run: bool,
    pub yes: bool,
    pub machine: bool,
    pub no_boot_entry: bool,
    pub disk: Option<String>,
    pub confirm: Option<String>,
    pub mode: Option<String>,
    pub gap_start: Option<u64>,
    pub boot_part: Option<u32>,
    pub data_part: Option<u32>,
    pub keep_esp: bool,
    pub boot_mib: Option<u64>,
    pub data_mib: Option<u64>,
    pub table: Option<String>,
    pub confirm_running_disk: bool,
    pub zram: Option<String>,
    pub swapfile: Option<String>,
    pub hostname: Option<String>,
    pub plan_file: Option<PathBuf>,
    pub apply_plan: Option<PathBuf>,
    pub source: Option<String>,
    pub template: Option<PathBuf>,
    /// Testing: a regular file standing in for the disk, with its sector size.
    pub target_file: Option<PathBuf>,
    pub sector_size: Option<u32>,
    /// The size to type, for `--yes` with `--apply-plan` (the plan file carries its own).
    pub confirm_size: Option<String>,
}

pub fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    let num = |a: &str, v: Option<&String>| -> Result<u64, String> { v.and_then(|v| v.parse().ok()).ok_or_else(|| format!("{a} needs a whole number")) };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--plan" => {}
            "-h" | "--help" => o.help = true,
            "--list" => o.list = true,
            "--json" => o.json = true,
            "--dry-run" => o.dry_run = true,
            "--yes" | "-y" => o.yes = true,
            "--machine" => o.machine = true,
            "--no-boot-entry" => o.no_boot_entry = true,
            "--keep-esp" => o.keep_esp = true,
            "--confirm-running-disk" => o.confirm_running_disk = true,
            "--disk" => o.disk = Some(it.next().ok_or("--disk needs a drive name")?.trim_start_matches("/dev/").to_string()),
            "--confirm" => o.confirm = Some(it.next().ok_or("--confirm needs the drive name")?.trim_start_matches("/dev/").to_string()),
            "--confirm-size" => o.confirm_size = Some(it.next().ok_or("--confirm-size needs the size as shown")?.clone()),
            "--mode" => {
                let m = it.next().ok_or("--mode needs whole-disk, free-space, reuse or custom")?;
                Mode::parse(m)?;
                o.mode = Some(m.clone());
            }
            "--gap-start" => o.gap_start = Some(num(a, it.next())?),
            "--boot-part" => o.boot_part = Some(num(a, it.next())? as u32),
            "--data-part" => o.data_part = Some(num(a, it.next())? as u32),
            "--boot-mib" => o.boot_mib = Some(num(a, it.next())?),
            "--data-mib" => o.data_mib = Some(num(a, it.next())?),
            "--table" => {
                let t = it.next().ok_or("--table needs auto, gpt or mbr")?;
                TableChoice::parse(t)?;
                o.table = Some(t.clone());
            }
            "--zram" => o.zram = Some(it.next().ok_or("--zram needs a value")?.clone()),
            "--swapfile" => o.swapfile = Some(it.next().ok_or("--swapfile needs a value")?.clone()),
            "--hostname" => o.hostname = Some(it.next().ok_or("--hostname needs a name")?.clone()),
            "--plan-file" => o.plan_file = Some(PathBuf::from(it.next().ok_or("--plan-file needs a path")?)),
            "--apply-plan" => o.apply_plan = Some(PathBuf::from(it.next().ok_or("--apply-plan needs a path")?)),
            "--source" => o.source = Some(it.next().ok_or("--source needs a disk name")?.clone()),
            "--template" => o.template = Some(PathBuf::from(it.next().ok_or("--template needs a file")?)),
            "--target-file" => o.target_file = Some(PathBuf::from(it.next().ok_or("--target-file needs a path")?)),
            "--sector-size" => o.sector_size = Some(it.next().and_then(|v| v.parse().ok()).filter(|n| matches!(n, 512 | 4096)).ok_or("--sector-size needs 512 or 4096")?),
            s if s.starts_with('-') => return Err(format!("unknown option {s} (see --help)")),
            s => {
                if o.template.replace(PathBuf::from(s)).is_some() {
                    return Err("only one image file can be given".into());
                }
            }
        }
    }
    if o.yes && o.apply_plan.is_none() && !o.dry_run && (o.disk.is_none() || o.confirm.is_none() || o.mode.is_none()) {
        return Err("--yes needs --disk, --mode and --confirm <the same drive name>".into());
    }
    Ok(o)
}

impl Opts {
    pub fn request(&self) -> Result<Request, String> {
        let mode = Mode::parse(self.mode.as_deref().ok_or("--mode is needed")?)?;
        let r = Request {
            mode,
            gap_start: self.gap_start,
            boot_part: self.boot_part,
            data_part: self.data_part,
            keep_esp: self.keep_esp,
            boot_mib: self.boot_mib,
            data_mib: self.data_mib,
            table: TableChoice::parse(self.table.as_deref().unwrap_or("auto"))?,
            confirm_running_disk: self.confirm_running_disk,
            zram: self.zram.clone().unwrap_or_else(|| "auto".into()),
            swapfile: self.swapfile.clone().unwrap_or_else(|| "off".into()),
            hostname: self.hostname.clone(),
        };
        r.choices().validate()?;
        Ok(r)
    }
}

/// Where the machine's disks come from; tests point it at a made-up machine.
pub struct Machine {
    pub sys: PathBuf,
    pub dev: PathBuf,
    pub proc_mounts: String,
    pub swaps: String,
}

impl Machine {
    pub fn real() -> Machine {
        Machine { sys: "/sys".into(), dev: "/dev".into(), proc_mounts: std::fs::read_to_string("/proc/mounts").unwrap_or_default(), swaps: std::fs::read_to_string("/proc/swaps").unwrap_or_default() }
    }
}

fn swap_parts(swaps: &str) -> Vec<String> {
    swaps.lines().skip(1).filter_map(|l| l.split_whitespace().next()).map(|d| d.rsplit('/').next().unwrap_or(d).to_string()).collect()
}

/// Every disk of the machine as a [`DiskModel`] (reading the tables needs administrator rights; unreadable disks are skipped with
/// a note in `skipped`).
pub fn discover(m: &Machine, boot_media: Option<&str>, skipped: &mut Vec<String>) -> (Vec<DiskModel>, Vec<DiskInfo>) {
    let infos = install::list_disks(&m.sys, &m.dev, &m.proc_mounts, &m.swaps);
    let mounts = drives::parse_proc_mounts(&m.proc_mounts);
    let swaps = swap_parts(&m.swaps);
    let mut models = Vec::new();
    for i in &infos {
        let d = &i.drive;
        let meta = DiskMeta { name: d.name.clone(), path: d.path.clone(), model: d.model.clone(), serial: i.serial.clone(), removable: d.removable, usb: d.usb, read_only: d.read_only, swap_parts: swaps.clone() };
        match std::fs::File::open(&d.path) {
            Ok(mut f) => models.push(read_model(&mut f, &meta, d.size_bytes, i.logical_sector, &mounts, boot_media)),
            Err(e) => skipped.push(format!("{}: cannot read ({e}); run as administrator", d.path)),
        }
    }
    (models, infos)
}

fn source_of(o: &Opts, m: &Machine, infos: &[DiskInfo]) -> Result<SourceInfo, String> {
    match (&o.template, &o.source) {
        (Some(t), _) => install::read_source(t),
        (None, Some(s)) => install::read_source(&m.dev.join(s.trim_start_matches("/dev/"))),
        (None, None) => install::find_source(infos, &m.dev, o.disk.as_deref()),
    }
}

/// The disk list as text (for `--list`).
pub fn list_text(models: &[DiskModel]) -> String {
    let mut s = String::new();
    for d in models {
        let kind = if d.usb { "USB" } else if d.removable { "removable" } else { "internal" };
        let why = Wizard::disk_reason_of(d).map(|r| format!("  NOT USABLE: {r}")).unwrap_or_default();
        s.push_str(&format!("{:<10} {:>9}  {:<24} {:<8} {:<5} {}-byte sectors{}{why}\n", d.name, human_size(d.size_bytes), d.model, kind, d.table.map_or("none", |t| t.label()), d.sector, if d.running_system { "  [holds the running system]" } else { "" }));
        for p in &d.parts {
            s.push_str(&format!("    {:<12} #{:<2} {:>9}  {:<22} {} {}{}\n", p.name, p.index, human_size(p.bytes(d.sector)), p.kind, p.fs, if p.label.is_empty() { String::new() } else { format!("“{}”", p.label) }, p.in_use().map(|m| format!("  [{m}]")).unwrap_or_default()));
        }
        for g in d.free_gaps(d.table.unwrap_or(crate::gpt_install::TableKind::Gpt)) {
            s.push_str(&format!("    free space at sector {:<12} {:>9}\n", g.first_lba, human_size(g.bytes(d.sector))));
        }
    }
    s
}

fn progress_line(machine: bool) -> impl FnMut(Progress) {
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    move |p: Progress| {
        if p.done < p.total && last.elapsed() < std::time::Duration::from_millis(250) {
            return;
        }
        last = std::time::Instant::now();
        let label = match p.phase {
            Phase::Checking => "checking",
            Phase::Writing => "writing",
            Phase::Syncing => "flushing",
            Phase::Verifying => "verifying",
        };
        if machine {
            println!("@progress {label} {} {}", p.done, p.total);
        } else {
            let pct = if p.total == 0 { 100 } else { p.done * 100 / p.total };
            print!("\r{label:<10} {pct:>3}%  {}  ", crate::ui::human_duration(p.elapsed));
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }
}

/// Unmount the partitions the plan names (the person confirmed; the writer checks again afterwards).
pub fn unmount_planned(plan: &Plan, model: &DiskModel) -> Result<(), String> {
    use iyakki_storage::Mounter as _;
    for name in &plan.unmount {
        if let Some(p) = model.parts.iter().find(|p| &p.name == name) {
            for mp in p.mounts.iter().rev() {
                iyakki_storage::SysMounter.unmount(Path::new(mp)).map_err(|e| format!("could not unmount {mp}: {e} (close anything using it and try again)"))?;
            }
        }
    }
    Ok(())
}

fn efivars(o: &Opts, plan: &Plan) -> Option<PathBuf> {
    let d = PathBuf::from("/sys/firmware/efi/efivars");
    (!o.no_boot_entry && o.target_file.is_none() && plan.internal && d.is_dir()).then_some(d)
}

fn root() -> bool {
    // SAFETY: geteuid takes no arguments.
    unsafe { libc::geteuid() == 0 }
}

/// Write `plan` for `model`: unmount what the plan lists, then [`apply::apply`].
pub fn write_plan(o: &Opts, m: &Machine, model: &DiskModel, plan: &Plan, src: &SourceInfo, progress: &mut dyn FnMut(Progress)) -> Result<apply::ApplyReport, String> {
    unmount_planned(plan, model)?;
    let mounts_now = if o.target_file.is_some() { String::new() } else { std::fs::read_to_string("/proc/mounts").unwrap_or_else(|_| m.proc_mounts.clone()) };
    let env = ApplyEnv { proc_mounts: mounts_now, efivars: efivars(o, plan), dry_run: false };
    let path = o.target_file.clone().unwrap_or_else(|| PathBuf::from(&model.path));
    // the model is read again right before writing: the plan must still be the one the person confirmed
    let fresh = reread(o, m, model)?;
    apply::apply(&path, &fresh, &plan.request, src, Some(&plan.digest()), crate::install::booted_uefi(), &env, progress)
}

fn reread(o: &Opts, m: &Machine, model: &DiskModel) -> Result<DiskModel, String> {
    let mounts = drives::parse_proc_mounts(&std::fs::read_to_string("/proc/mounts").unwrap_or_else(|_| m.proc_mounts.clone()));
    let path = o.target_file.clone().unwrap_or_else(|| PathBuf::from(&model.path));
    let meta = DiskMeta { name: model.name.clone(), path: model.path.clone(), model: model.model.clone(), serial: model.serial.clone(), removable: model.removable, usb: model.usb, read_only: model.read_only, swap_parts: swap_parts(&m.swaps) };
    let mut f = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut fresh = read_model(&mut f, &meta, model.size_bytes, model.sector, &mounts, if model.booted_from { Some(model.name.as_str()) } else { None });
    fresh.booted_from = model.booted_from;
    Ok(fresh)
}

fn target_model(o: &Opts, f: &Path) -> Result<DiskModel, String> {
    let size = std::fs::metadata(f).map_err(|e| format!("{}: {e}", f.display()))?.len();
    let meta = DiskMeta { name: "testfile".into(), path: f.display().to_string(), model: "test file".into(), serial: String::new(), removable: true, usb: true, ..Default::default() };
    let mut h = std::fs::File::open(f).map_err(|e| e.to_string())?;
    Ok(read_model(&mut h, &meta, size, o.sector_size.unwrap_or(512), &[], None))
}

/// `iyakki-usb --plan` / `iyakki install --plan`. Returns the exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args, &Machine::real(), false) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("iyakki installer: {e}");
            1
        }
    }
}

/// The boot-time install mode: the same pages on the console, the stick IYAKKI booted from is never offered as a target.
pub fn run_boot() -> i32 {
    match run_inner(&[], &Machine::real(), true) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("iyakki installer: {e}");
            1
        }
    }
}

pub fn run_inner(args: &[String], m: &Machine, boot_mode: bool) -> Result<i32, String> {
    let o = parse(args)?;
    if o.help {
        print!("{HELP}");
        return Ok(0);
    }
    #[cfg(not(target_os = "linux"))]
    if o.target_file.is_none() {
        return Err("the partition planner reads disks through Linux (/sys) and is not available on this system yet (macOS and Windows hosts: docs/INSTALLER.md). `iyakki-usb --install` still writes a whole USB stick here.".into());
    }
    if let Some(f) = &o.apply_plan {
        return apply_plan_file(&o, m, f);
    }
    // the disks, and the image to copy from
    let mut skipped = Vec::new();
    let (mut models, infos, src) = if let Some(f) = &o.target_file {
        let model = target_model(&o, f)?;
        let src = match &o.template {
            Some(t) => install::read_source(t)?,
            None => return Err("--target-file needs the image as an argument or --template".into()),
        };
        (vec![model], vec![], src)
    } else {
        let (models0, infos) = discover(m, None, &mut skipped);
        let src = source_of(&o, m, &infos)?;
        let boot = src.path.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| models0.iter().any(|d| &d.name == n));
        let (models, _) = if boot.is_some() { discover(m, boot.as_deref(), &mut Vec::new()) } else { (models0, vec![]) };
        (models, infos, src)
    };
    let _ = infos;
    if boot_mode {
        models.retain(|d| !d.booted_from);
    }
    for s in &skipped {
        eprintln!("{s}");
    }
    let spec = apply::source_spec(&src)?;
    if o.list && o.json {
        let j = Json::obj(vec![("format", Json::str("iyakki-disk-list-1")), ("source", spec.to_json()), ("uefi", Json::Bool(crate::install::booted_uefi())), ("disks", Json::Arr(models.iter().map(DiskModel::to_json).collect()))]);
        print!("{}", j.pretty());
        return Ok(0);
    }
    if o.list {
        println!("Image: {} (boot volume {})\n", src.path.display(), human_size(spec.boot_bytes));
        print!("{}", list_text(&models));
        return Ok(0);
    }
    if o.disk.is_some() && o.mode.is_some() {
        let name = o.disk.clone().unwrap();
        let model = models.iter().find(|d| d.name == name || d.path == format!("/dev/{name}")).cloned().ok_or_else(|| format!("{name} is not a disk of this computer (see --list)"))?;
        let req = o.request()?;
        let plan = build_plan(&model, &req, &spec, crate::install::booted_uefi())?;
        println!("{}", plan.render());
        if let Some(f) = &o.plan_file {
            write_plan_file(f, &plan, None)?;
            println!("Plan saved to {}.", f.display());
        }
        if o.dry_run {
            println!("Dry run: nothing was written.");
            return Ok(0);
        }
        if !root() && o.target_file.is_none() {
            return Err("writing needs administrator rights: run it with sudo (or add --dry-run to see the plan)".into());
        }
        if o.yes {
            if o.confirm.as_deref() != Some(model.name.as_str()) {
                return Err(format!("--confirm must repeat the drive name ({}); nothing was written", model.name));
            }
        } else {
            println!("This writes to {} ({}).", model.path, human_size(model.size_bytes));
            let n = ask(&format!("Type the drive name ({}) to continue, or Enter to cancel: ", model.name));
            let z = if n.is_empty() { String::new() } else { ask(&format!("Type its size ({}) to confirm: ", human_size(model.size_bytes))) };
            if !confirmation_ok(&plan, &n, &z) {
                println!("Cancelled. Nothing was written.");
                return Ok(2);
            }
        }
        let mut p = progress_line(o.machine);
        let r = write_plan(&o, m, &model, &plan, &src, &mut p)?;
        println!();
        println!("Done. {} written and verified.", human_size(r.written));
        for n in &r.notes {
            println!("{n}");
        }
        println!("{}", AFTER);
        return Ok(0);
    }
    // the pages
    if !crate::term::is_tty() {
        return Err("the pages need a terminal; pass --disk and --mode (and --dry-run) to plan without one".into());
    }
    if models.is_empty() {
        return Err("no drives found".into());
    }
    let mut wiz = Wizard::new(models, spec, crate::install::booted_uefi());
    let models_for_write = wiz.disks.clone();
    let mut writer = |plan: &Plan, progress: &mut dyn FnMut(Progress)| -> Result<String, String> {
        if !root() && o.target_file.is_none() {
            return Err("writing needs administrator rights: start the installer with sudo".into());
        }
        let model = models_for_write.iter().find(|d| d.name == plan.disk_name).ok_or("the drive vanished")?;
        let r = write_plan(&o, m, model, plan, &src, progress)?;
        Ok(format!("{} written and verified.\n{}\n\n{}", human_size(r.written), r.notes.join("\n"), AFTER))
    };
    crate::tui::run(&mut wiz, &mut writer)?;
    Ok(0)
}

const AFTER: &str = "The first start of the new system formats its data partition (a few seconds) and asks for the first account. To start it: restart and open the firmware's boot menu (F12, F10, Esc or Option at power-on, depending on the make) and choose IYAKKI. Your other system is untouched and still starts as before.";

fn ask(prompt: &str) -> String {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut l = String::new();
    let _ = std::io::stdin().lock().read_line(&mut l);
    l.trim().to_string()
}

/// Save the plan as JSON; `confirmed` (the Install button, or name and size as typed) is stored by the window installer for the
/// writer to check.
pub fn write_plan_file(path: &Path, plan: &Plan, confirmed: Option<&Confirmation>) -> Result<(), String> {
    let mut j = plan.to_json();
    if let (Json::Obj(f), Some(c)) = (&mut j, confirmed) {
        f.push(("confirmed".into(), c.to_json()));
    }
    std::fs::write(path, j.pretty()).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// `--apply-plan FILE`: the privileged writer.
fn apply_plan_file(o: &Opts, m: &Machine, file: &Path) -> Result<i32, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
    // the plan file is private to whoever asked for the install: not writable by others
    // SAFETY: getuid takes no arguments.
    let caller = unsafe { libc::getuid() };
    let sudo_uid: Option<u32> = std::env::var("PKEXEC_UID").or_else(|_| std::env::var("SUDO_UID")).ok().and_then(|v| v.parse().ok());
    if meta.mode() & 0o022 != 0 || !(meta.uid() == caller || Some(meta.uid()) == sudo_uid) {
        return Err(format!("{} must be owned by the person who asked for the install and not writable by others; refused", file.display()));
    }
    let j = Json::parse(&std::fs::read_to_string(file).map_err(|e| e.to_string())?)?;
    if j.get("format").and_then(Json::as_str) != Some("iyakki-install-plan-1") {
        return Err("this is not an IYAKKI install plan file".into());
    }
    let disk = j.get("disk").ok_or("the plan file has no disk")?;
    let name = disk.get("name").and_then(Json::as_str).ok_or("the plan file has no drive name")?.to_string();
    let digest = j.get("digest").and_then(Json::as_str).ok_or("the plan file has no digest")?.to_string();
    let req = Request::from_json(j.get("request").ok_or("the plan file has no request")?)?;
    let confirmed = j.get("confirmed").ok_or("the plan file carries no confirmation; the writer needs the Install button's confirmation or the typed drive name and size")?;
    let confirmed = Confirmation::from_json(confirmed).ok_or("the confirmation in the plan file is not readable; nothing was written")?;
    if !root() && o.target_file.is_none() {
        return Err("the writer needs administrator rights (the window installer starts it through pkexec)".into());
    }
    let mut skipped = Vec::new();
    let (models, infos) = if let Some(f) = &o.target_file { (vec![target_model(o, f)?], vec![]) } else { discover(m, None, &mut skipped) };
    let src = match (&o.template, &o.source) {
        (None, None) => match j.get("source").and_then(Json::as_str) {
            Some(p) => install::read_source(Path::new(p))?,
            None => source_of(o, m, &infos)?,
        },
        _ => source_of(o, m, &infos)?,
    };
    let boot = src.path.file_name().map(|n| n.to_string_lossy().into_owned());
    let model = {
        let d = models.into_iter().find(|d| d.name == name).ok_or_else(|| format!("{name} is not a disk of this computer any more"))?;
        let mut d = d;
        if boot.as_deref() == Some(d.name.as_str()) {
            d.booted_from = true;
        }
        d
    };
    let spec = apply::source_spec(&src)?;
    let plan = build_plan(&model, &req, &spec, crate::install::booted_uefi())?;
    if plan.digest() != digest {
        return Err("the disk or the image changed since the plan was confirmed (the digests differ); nothing was written. Make the plan again.".into());
    }
    check_confirmation(&plan, &confirmed)?;
    let mut p = progress_line(o.machine);
    let r = write_plan(o, m, &model, &plan, &src, &mut p)?;
    println!("@done {}", r.written);
    for n in &r.notes {
        println!("@note {n}");
    }
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
        let o = parse(&s(&["--plan", "--disk", "/dev/sdb", "--mode", "reuse", "--boot-part", "2", "--data-part", "3", "--keep-esp", "--dry-run", "--plan-file", "p.json", "img.img"])).unwrap();
        assert_eq!((o.disk.as_deref(), o.boot_part, o.data_part, o.keep_esp, o.dry_run), (Some("sdb"), Some(2), Some(3), true, true));
        assert_eq!(o.template.as_deref(), Some(Path::new("img.img")));
        let r = o.request().unwrap();
        assert_eq!(r.mode, Mode::Reuse);
        assert!(parse(&s(&["--yes"])).unwrap_err().contains("--confirm"));
        assert!(parse(&s(&["--mode", "nope"])).is_err());
        assert!(parse(&s(&["--boot-part", "x"])).is_err());
        assert!(parse(&s(&["--frob"])).unwrap_err().contains("unknown"));
        assert!(parse(&s(&["--apply-plan", "f", "--yes"])).is_ok());
        assert!(HELP.contains("--dry-run") && HELP.contains("--plan-file") && HELP.contains("free-space"));
    }
}

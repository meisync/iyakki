//! Linux apps: a Debian or Ubuntu sandbox on the data volume (`docs/LINUX-APPS.md`).
//!
//! IYAKKI is musl + BusyBox, so programs built for glibc do not run on it. A sandbox is a Debian or Ubuntu root file system,
//! downloaded for the person at their click ([`crate::linuxapps_dist`]), unpacked under `<data>/linuxapps/<person>/<name>/rootfs`
//! ([`crate::rootfs`]) and entered with Linux namespaces ([`crate::linuxapps_init`]). `apt` inside installs the person's packages.
//!
//! Everything that starts a program goes through the [`Runner`] of `wsexec.rs` as an **argument array**, never a shell string, so a
//! test can swap in a fake runner and read the exact command. The command built for a sandbox is
//! `iyakki linuxapp-init --root ... --net off --bind-ro SRC DST ... -- <command>`.
//!
//! Isolation (plain words): the sandbox sees its own folder as `/` and nothing of the host; only folders the person mounts show up
//! (read-only unless they chose read-write); the network is a switch (off: an empty network; on: the host's network, outbound);
//! it runs as a user that is not root unless the person installs packages (then root *inside*, which is a mapped, powerless id on the
//! host); the vault, IYAKKI's sockets and `/var/lib/iyakki` are not reachable; no `/sys`, no devices except six harmless nodes.
//! Limits come from cgroups when the kernel has them. Every create, mount, run, install and delete is written to an audit log.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_proxy::Streamer;
use serde::{Deserialize, Serialize};

use crate::linuxapps_dist::{self as dist, Release};
use crate::linuxapps_init::{self as init, Bind, IdMap};
use crate::rootfs;
use crate::wsexec::{Out, Runner, Sink};

pub const DIR: &str = "linuxapps";
/// The job workspace Linux apps jobs use: the same as Quick install, so the top bar's installer indicator and list show them.
pub use crate::quickinstall::WORKSPACE;

pub type AuditFn = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

pub const MAX_MOUNTS: usize = 8;
pub const MAX_PACKAGES: usize = 40;
const OUT_CAP: usize = 256 * 1024;

// ------------------------------------------------------------------------------------------------ names

/// `a-z`, digits and `-`, starting with a letter, at most 30 characters.
pub fn valid_name(n: &str) -> bool {
    (1..=30).contains(&n.len()) && n.starts_with(|c: char| c.is_ascii_lowercase()) && !n.ends_with('-') && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A Debian package name: `[a-z0-9][a-z0-9+.-]*`, no trailing `-` (apt reads that as "remove"), at most 100 characters. Nothing that
/// starts with `-` can pass, so a name cannot become an option of `apt-get`.
pub fn valid_package(p: &str) -> bool {
    !p.is_empty() && p.len() <= 100 && p.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) && !p.ends_with('-') && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
}

pub fn check_packages(pkgs: &[String]) -> Result<(), String> {
    if pkgs.is_empty() {
        return Err("name at least one package".into());
    }
    if pkgs.len() > MAX_PACKAGES {
        return Err(format!("at most {MAX_PACKAGES} packages at a time"));
    }
    match pkgs.iter().find(|p| !valid_package(p)) {
        Some(p) => Err(format!("`{p}` is not a package name (lower-case letters, digits, + . - only, starting with a letter or digit)")),
        None => Ok(()),
    }
}

/// Search words: plain words, none starting with `-`.
pub fn check_search(words: &[String]) -> Result<(), String> {
    if words.is_empty() || words.len() > 6 {
        return Err("search for one to six words".into());
    }
    for w in words {
        if w.is_empty() || w.len() > 60 || w.starts_with('-') || !w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '_' | '-')) {
            return Err(format!("`{w}` is not a search word (letters, digits, + . _ - only)"));
        }
    }
    Ok(())
}

fn clean_user(u: &str) -> String {
    let c: String = u.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(40).collect();
    if c.is_empty() || c.starts_with('.') { "user".into() } else { c }
}

// ------------------------------------------------------------------------------------------------ the record of a sandbox

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    /// The folder on the host (absolute, resolved).
    pub src: String,
    /// Where it appears inside (`/mnt/work`).
    pub dst: String,
    pub rw: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sandbox {
    pub name: String,
    pub owner: String,
    /// `debian-12`, `ubuntu-24.04`, or `imported`.
    pub release: String,
    pub created: u64,
    pub network: bool,
    pub mounts: Vec<Mount>,
    /// 0: one id (the person's own); else the host id the sandbox's ids `0..65536` start at.
    pub id_base: u32,
    pub mem_mb: u32,
    pub cpu_pct: u32,
    pub pids: u32,
    /// What was checked about the download (a sentence).
    pub verified: String,
    pub source: String,
    pub arch: String,
}

impl Sandbox {
    pub fn title(&self) -> String {
        dist::release(&self.release).map_or_else(|| self.release.clone(), |r| r.title.to_string())
    }
}

// ------------------------------------------------------------------------------------------------ what the kernel offers

/// What this kernel offers for sandboxes. Read from `/proc` and `/sys`; tests point it at a folder of stand-ins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub user_ns: bool,
    pub mnt_ns: bool,
    pub pid_ns: bool,
    pub uts_ns: bool,
    pub ipc_ns: bool,
    pub net_ns: bool,
    /// `/proc/sys/user/max_user_namespaces` (0: user namespaces are switched off for everyone).
    pub max_user_ns: Option<u64>,
    pub cgroup_v2: bool,
    pub controllers: Vec<String>,
    pub seccomp_filter: bool,
    pub root: bool,
    /// `/proc/sys/kernel/unprivileged_userns_clone` is 0 (Debian-style switch) or AppArmor restricts them: matters when not root.
    pub unprivileged_blocked: bool,
}

impl Probe {
    pub fn read() -> Probe {
        // SAFETY: a plain getter.
        let root = unsafe { libc::geteuid() } == 0;
        Probe::read_at(Path::new("/proc"), Path::new("/sys/fs/cgroup"), root)
    }

    pub fn read_at(proc: &Path, cg: &Path, root: bool) -> Probe {
        let ns = |n: &str| proc.join("self/ns").join(n).exists();
        let controllers: Vec<String> = std::fs::read_to_string(cg.join("cgroup.controllers")).map(|t| t.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
        let one = |p: &str| std::fs::read_to_string(proc.join(p)).ok().map(|t| t.trim().to_string());
        Probe {
            user_ns: ns("user"),
            mnt_ns: ns("mnt"),
            pid_ns: ns("pid"),
            uts_ns: ns("uts"),
            ipc_ns: ns("ipc"),
            net_ns: ns("net"),
            max_user_ns: one("sys/user/max_user_namespaces").and_then(|v| v.parse().ok()),
            cgroup_v2: cg.join("cgroup.controllers").is_file(),
            controllers,
            seccomp_filter: proc.join("sys/kernel/seccomp/actions_avail").exists(),
            root,
            unprivileged_blocked: one("sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") || one("sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1") && !root,
        }
    }

    /// Whether the namespaced runtime can work, and the kernel options to turn on when it cannot.
    pub fn missing(&self) -> Vec<String> {
        let mut m = Vec::new();
        for (ok, opt) in [(self.user_ns, "CONFIG_USER_NS"), (self.mnt_ns, "CONFIG_NAMESPACES (mount namespaces)"), (self.pid_ns, "CONFIG_PID_NS"), (self.uts_ns, "CONFIG_UTS_NS"), (self.ipc_ns, "CONFIG_IPC_NS"), (self.net_ns, "CONFIG_NET_NS")] {
            if !ok {
                m.push(opt.to_string());
            }
        }
        if self.max_user_ns == Some(0) {
            m.push("sysctl user.max_user_namespaces is 0 (raise it)".into());
        }
        if !self.root && self.unprivileged_blocked {
            m.push("this kernel or AppArmor blocks user namespaces for a user that is not root".into());
        }
        m
    }

    /// Features that make the sandbox better but are not required: a plain list for the page.
    pub fn optional_missing(&self) -> Vec<String> {
        let mut m = Vec::new();
        if !self.cgroup_v2 {
            m.push("cgroup v2 at /sys/fs/cgroup (CONFIG_CGROUPS, cgroup2 mounted): memory, cpu and process limits are not enforced".into());
        } else {
            for (c, opt) in [("memory", "CONFIG_MEMCG"), ("cpu", "CONFIG_CGROUP_SCHED and CONFIG_CFS_BANDWIDTH"), ("pids", "CONFIG_CGROUP_PIDS")] {
                if !self.controllers.iter().any(|x| x == c) {
                    m.push(format!("the {c} cgroup controller ({opt}): that limit is not enforced"));
                }
            }
        }
        if !self.seccomp_filter {
            m.push("CONFIG_SECCOMP_FILTER: the system-call filter is not applied (the sandbox refuses to start with it switched on; switch it off in the sandbox settings)".into());
        }
        m
    }

    pub fn can_namespace(&self) -> bool {
        self.missing().is_empty()
    }
}

/// How sandboxes are run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    /// Namespaces and `pivot_root` ([`crate::linuxapps_init`]).
    Namespaced,
    /// Fallback: a pod in the dev cluster from a rootfs imported as an image (design in docs/LINUX-APPS.md; the image conversion is not built).
    K3sPod,
}

pub fn choose_runtime(p: &Probe, cluster_ready: bool) -> Result<Runtime, String> {
    if p.can_namespace() {
        Ok(Runtime::Namespaced)
    } else if cluster_ready {
        Ok(Runtime::K3sPod)
    } else {
        Err(format!("this kernel cannot run a sandbox. Missing: {}. A kernel rebuild with these options is the owner's to schedule (docs/LINUX-APPS.md).", p.missing().join(", ")))
    }
}

/// The pod a K3s-fallback sandbox would be: no service account, no privilege, the sandbox's image pinned by digest.
pub fn pod_manifest(user: &str, name: &str, image: &str) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": {"name": format!("linuxapp-{}-{name}", clean_user(user).to_ascii_lowercase().replace(['_', '.'], "-")), "namespace": "iyakki-apps", "labels": {"app.kubernetes.io/managed-by": "iyakki", "iyakki.dev/linux-app": name}},
        "spec": {
            "automountServiceAccountToken": false, "restartPolicy": "Never", "hostNetwork": false, "hostPID": false,
            "containers": [{"name": "sandbox", "image": image, "command": ["sleep", "infinity"],
                "securityContext": {"allowPrivilegeEscalation": false, "privileged": false, "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"}},
                "resources": {"limits": {"memory": "2Gi", "cpu": "2"}}}]
        }
    })
}

pub fn pod_exec_argv(pod: &str, cmd: &[String]) -> Vec<String> {
    let mut a: Vec<String> = ["kubectl", "exec", "-n", "iyakki-apps", pod, "--"].iter().map(|s| s.to_string()).collect();
    a.extend(cmd.iter().cloned());
    a
}

// ------------------------------------------------------------------------------------------------ folders the person mounts

/// Whether a host folder may be mounted into a sandbox, and its resolved path. The data volume (the vault, sockets, other people's
/// files) and the system folders are never offered; the people's `home` folders under the data volume are.
pub fn mount_source_ok(src: &str, data: &Path) -> Result<PathBuf, String> {
    if !src.starts_with('/') || src.contains('\0') {
        return Err(format!("{src}: give the full path of a folder"));
    }
    let canon = std::fs::canonicalize(src).map_err(|e| format!("{src}: {e}"))?;
    if canon.to_string_lossy().contains([':', '\n']) {
        return Err(format!("{src}: a folder with a colon or a line break in its path cannot be mounted; rename it"));
    }
    if !canon.is_dir() {
        return Err(format!("{src} is not a folder"));
    }
    let data_c = std::fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf());
    let homes = data_c.join("home");
    if canon == data_c || canon == homes {
        return Err(format!("{src} is not a folder of yours"));
    }
    if canon.starts_with(&data_c) && !canon.starts_with(&homes) {
        return Err(format!("{src} is inside IYAKKI's data folder, which holds the vault and other people's files; only folders under home can be mounted"));
    }
    if data_c.starts_with(&canon) && canon != data_c {
        return Err(format!("{src} contains IYAKKI's data folder; choose a folder inside your home"));
    }
    for bad in ["/", "/proc", "/sys", "/dev", "/run", "/etc", "/boot", "/var/lib/iyakki", "/etc/iyakki", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/var/run"] {
        if canon == Path::new(bad) || (bad != "/" && canon.starts_with(bad)) {
            return Err(format!("{src} is a system folder and cannot be mounted"));
        }
    }
    Ok(canon)
}

/// `/mnt/<name>` for a host folder (its last part, lower-cased and cleaned).
pub fn default_dst(src: &Path) -> String {
    let base: String = src.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    format!("/mnt/{}", if base.is_empty() { "folder".to_string() } else { base })
}

// ------------------------------------------------------------------------------------------------ the manager

/// Options of one run inside a sandbox.
#[derive(Clone)]
pub struct ExecOpts {
    /// Run as root inside (the id is a powerless mapped one on the host): for `apt`. Otherwise the sandbox's user.
    pub root: bool,
    pub timeout: Duration,
    pub sink: Option<Sink>,
    pub env: Vec<(String, String)>,
    /// A folder inside the sandbox; the user's home when empty.
    pub cwd: String,
}

impl Default for ExecOpts {
    fn default() -> ExecOpts {
        ExecOpts { root: false, timeout: Duration::from_secs(300), sink: None, env: vec![], cwd: String::new() }
    }
}

pub struct LinuxApps {
    /// `<data>/linuxapps`
    root: PathBuf,
    data: PathBuf,
    user: String,
    exe: PathBuf,
    run: Arc<dyn Runner>,
    audit: Mutex<Option<AuditFn>>,
    multi_ok: bool,
    free_space: Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>,
}

impl LinuxApps {
    /// `exe` is the IYAKKI program that runs `linuxapp-init` (this one).
    pub fn new(data: &Path, user: &str, exe: &Path, run: Arc<dyn Runner>) -> LinuxApps {
        // SAFETY: a plain getter.
        let root = unsafe { libc::geteuid() } == 0;
        LinuxApps { root: data.join(DIR), data: data.to_path_buf(), user: clean_user(user), exe: exe.to_path_buf(), run, audit: Mutex::new(None), multi_ok: root, free_space: Arc::new(crate::sharedsite::statvfs_free) }
    }

    pub fn set_audit(&self, f: AuditFn) {
        *self.audit.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Tests: pretend to be root (a wide id range) or not, and the free space of the volume.
    pub fn assume(&mut self, multi: bool, free: Option<u64>) {
        self.multi_ok = multi;
        self.free_space = Arc::new(move |_| free);
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    fn user_dir(&self) -> PathBuf {
        self.root.join(&self.user)
    }

    pub fn dir_of(&self, name: &str) -> PathBuf {
        self.user_dir().join(name)
    }

    pub fn rootfs_of(&self, name: &str) -> PathBuf {
        self.dir_of(name).join("rootfs")
    }

    pub fn audit_path(&self) -> PathBuf {
        self.root.join("audit.log")
    }

    fn audit(&self, action: &str, detail: &str) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let line = format!("{}\t{}\t{}\t{}\n", crate::clock::now_unix().max(0), self.user, action, detail.replace(['\n', '\t', '\r'], " "));
        if std::fs::create_dir_all(&self.root).is_ok() {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(self.audit_path()) {
                let _ = f.write_all(line.as_bytes());
            }
        }
        if let Some(f) = self.audit.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            f(&self.user, action, detail);
        }
    }

    /// The last `n` lines of the audit log as `(time, person, action, detail)`.
    pub fn audit_tail(&self, n: usize) -> Vec<(u64, String, String, String)> {
        let text = std::fs::read_to_string(self.audit_path()).unwrap_or_default();
        let mut v: Vec<(u64, String, String, String)> = text
            .lines()
            .filter_map(|l| {
                let mut p = l.splitn(4, '\t');
                Some((p.next()?.parse().ok()?, p.next()?.to_string(), p.next()?.to_string(), p.next().unwrap_or("").to_string()))
            })
            .collect();
        let skip = v.len().saturating_sub(n);
        v.drain(..skip);
        v
    }

    // ---- the list

    fn meta_path(&self, name: &str) -> PathBuf {
        self.dir_of(name).join("sandbox.json")
    }

    pub fn get(&self, name: &str) -> Result<Sandbox, String> {
        if !valid_name(name) {
            return Err(format!("`{name}` is not a sandbox name (lower-case letters, digits and -)"));
        }
        let t = std::fs::read_to_string(self.meta_path(name)).map_err(|_| format!("there is no sandbox named {name}"))?;
        serde_json::from_str(&t).map_err(|_| format!("the record of {name} is damaged"))
    }

    pub fn list(&self) -> Vec<Sandbox> {
        let mut v: Vec<Sandbox> = std::fs::read_dir(self.user_dir()).into_iter().flatten().flatten().filter_map(|e| self.get(&e.file_name().to_string_lossy()).ok()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    fn save(&self, s: &Sandbox) -> Result<(), String> {
        let tmp = self.dir_of(&s.name).join("sandbox.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.meta_path(&s.name)).map_err(|e| e.to_string())
    }

    /// Bytes the sandbox takes on disk.
    pub fn storage(&self, name: &str) -> u64 {
        rootfs::tree_size(&self.dir_of(name))
    }

    // ---- creating

    fn next_id_base(&self) -> u32 {
        if !self.multi_ok {
            return 0;
        }
        let f = self.root.join("next-id-base");
        let n: u32 = std::fs::read_to_string(&f).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
        let _ = std::fs::write(&f, (n + 1).to_string());
        2_000_000 + 65_536 * n
    }

    fn check_space(&self, need: u64) -> Result<(), String> {
        let _ = std::fs::create_dir_all(&self.root);
        match (self.free_space)(&self.root) {
            Some(free) if free < need => Err(format!("not enough free space on the data volume: {} MB needed, {} MB free", need >> 20, free >> 20)),
            _ => Ok(()),
        }
    }

    fn validate_mounts(&self, wanted: &[(String, String, bool)], scope: &dyn Fn(&Path, bool) -> Result<(), String>) -> Result<Vec<Mount>, String> {
        if wanted.len() > MAX_MOUNTS {
            return Err(format!("at most {MAX_MOUNTS} mounted folders"));
        }
        let mut out: Vec<Mount> = Vec::new();
        for (src, dst, rw) in wanted {
            let canon = mount_source_ok(src, &self.data)?;
            scope(&canon, *rw)?;
            let dst = if dst.is_empty() { default_dst(&canon) } else { dst.clone() };
            init::bind_dst_ok(&dst)?;
            if out.iter().any(|m| m.dst == dst) {
                return Err(format!("two folders would be mounted at {dst}"));
            }
            out.push(Mount { src: canon.display().to_string(), dst, rw: *rw });
        }
        Ok(out)
    }

    /// Create a sandbox: download the base, check it, unpack it, set up the user. `say` receives progress lines (the job's log).
    pub fn create(&self, req: &CreateReq, st: &dyn Streamer, scope: &dyn Fn(&Path, bool) -> Result<(), String>, say: &mut dyn FnMut(&str)) -> Result<Sandbox, String> {
        if !valid_name(&req.name) {
            return Err(format!("`{}` is not a sandbox name (lower-case letters, digits and -, starting with a letter, up to 30)", req.name));
        }
        let rel: &Release = dist::release(&req.release).ok_or_else(|| format!("`{}` is not one of: {}", req.release, dist::RELEASES.iter().map(|r| r.id).collect::<Vec<_>>().join(", ")))?;
        let mounts = self.validate_mounts(&req.mounts, scope)?;
        let dir = self.dir_of(&req.name);
        if dir.exists() {
            return Err(format!("a sandbox named {} exists already", req.name));
        }
        self.check_space((rel.unpacked_mb << 20) * 4 + (512 << 20))?;
        std::fs::create_dir_all(self.user_dir()).map_err(|e| e.to_string())?;
        std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.audit("sandbox.create.start", &format!("{} from {} ({})", req.name, rel.id, dist::arch()));
        let r = self.create_inner(req, rel, mounts, st, say);
        match &r {
            Ok(s) => self.audit("sandbox.create", &format!("{} {} network={} mounts={} verified: {}", s.name, s.release, s.network, s.mounts.len(), s.verified)),
            Err(e) => {
                let _ = rootfs::remove_tree(&dir);
                self.audit("sandbox.create.failed", &format!("{}: {e}", req.name));
            }
        }
        r
    }

    fn create_inner(&self, req: &CreateReq, rel: &Release, mounts: Vec<Mount>, st: &dyn Streamer, say: &mut dyn FnMut(&str)) -> Result<Sandbox, String> {
        let dir = self.dir_of(&req.name);
        say(dist::LICENCE_NOTE);
        say(&format!("[1/4] downloading {} from its distribution's servers", rel.title));
        let dl = dir.join("download");
        let (files, fetched) = dist::fetch(st, rel, &dl, &|total| self.check_space(total.saturating_mul(5).max(rel.unpacked_mb << 20) + (256 << 20)), &mut |d, t| {
            if t > 0 {
                say(&format!("      {} of {} MB", d >> 20, t >> 20));
            } else {
                say(&format!("      {} MB", d >> 20));
            }
        })?;
        say(&format!("[2/4] checked: {}", fetched.verified));
        let id_base = self.next_id_base();
        say(&format!("[3/4] unpacking{}", if id_base > 0 { " (owners mapped into this sandbox's own id range)" } else { "" }));
        let rootfs_dir = self.rootfs_of(&req.name);
        let mut pol = rootfs::Policy::base();
        pol.id_shift = (id_base > 0).then_some(id_base);
        pol.whiteouts = rel.distro == dist::Distro::Debian;
        let mut total = rootfs::Report::default();
        for f in &files {
            let rep = rootfs::unpack(f, &rootfs_dir, &dir.join("scratch"), &pol, &mut |n, b| say(&format!("      {n} files, {} MB", b >> 20)))?;
            total.files += rep.files;
            total.bytes += rep.bytes;
            total.skipped.extend(rep.skipped);
        }
        if !total.skipped.is_empty() {
            say(&format!("      left out (device nodes and pipes are never created): {}", total.skipped.join(", ")));
        }
        let _ = rootfs::remove_tree(&dl);
        let _ = rootfs::remove_tree(&dir.join("scratch"));
        say("[4/4] setting up the sandbox user and apt");
        finish_rootfs(&rootfs_dir, pol.id_shift)?;
        let s = Sandbox { name: req.name.clone(), owner: self.user.clone(), release: rel.id.into(), created: crate::clock::now_unix().max(0) as u64, network: req.network, mounts, id_base, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: fetched.verified, source: fetched.source, arch: dist::arch().into() };
        self.save(&s)?;
        say(&format!("done: sandbox {} is ready ({} files, {} MB)", s.name, total.files, total.bytes >> 20));
        Ok(s)
    }

    /// Make a sandbox from a root file system tarball the person assembled (for example with `debootstrap` inside another sandbox):
    /// "import tarball as image app". The tarball is unpacked with the same strict reader; nothing is checked against a vendor list.
    pub fn import(&self, name: &str, tarball: &Path, network: bool, say: &mut dyn FnMut(&str)) -> Result<Sandbox, String> {
        if !valid_name(name) {
            return Err(format!("`{name}` is not a sandbox name"));
        }
        let dir = self.dir_of(name);
        if dir.exists() {
            return Err(format!("a sandbox named {name} exists already"));
        }
        let size = std::fs::metadata(tarball).map_err(|e| format!("{}: {e}", tarball.display()))?.len();
        self.check_space(size.saturating_mul(6) + (512 << 20))?;
        std::fs::create_dir_all(self.user_dir()).map_err(|e| e.to_string())?;
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let r = (|| {
            let hash = {
                let mut f = std::fs::File::open(tarball).map_err(|e| e.to_string())?;
                let mut h = <sha2::Sha256 as sha2::Digest>::new();
                let mut b = vec![0u8; 1 << 16];
                loop {
                    use std::io::Read;
                    let n = f.read(&mut b).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    sha2::Digest::update(&mut h, &b[..n]);
                }
                sha2::Digest::finalize(h).iter().map(|x| format!("{x:02x}")).collect::<String>()
            };
            say(&format!("unpacking {} (SHA-256 {hash})", tarball.display()));
            let id_base = self.next_id_base();
            let mut pol = rootfs::Policy::base();
            pol.id_shift = (id_base > 0).then_some(id_base);
            let root = self.rootfs_of(name);
            let rep = rootfs::unpack(tarball, &root, &dir.join("scratch"), &pol, &mut |n, b| say(&format!("      {n} files, {} MB", b >> 20)))?;
            let _ = rootfs::remove_tree(&dir.join("scratch"));
            let _ = finish_rootfs(&root, pol.id_shift);
            let s = Sandbox { name: name.into(), owner: self.user.clone(), release: "imported".into(), created: crate::clock::now_unix().max(0) as u64, network, mounts: vec![], id_base, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: format!("none: imported from your file (SHA-256 {hash} computed here, nothing to compare it with)"), source: tarball.display().to_string(), arch: dist::arch().into() };
            self.save(&s)?;
            say(&format!("done: {} files, {} MB", rep.files, rep.bytes >> 20));
            Ok(s)
        })();
        match &r {
            Ok(s) => self.audit("sandbox.import", &format!("{} from {}", s.name, s.source)),
            Err(e) => {
                let _ = rootfs::remove_tree(&dir);
                self.audit("sandbox.import.failed", &format!("{name}: {e}"));
            }
        }
        r
    }

    // ---- changing

    /// Delete a sandbox and everything in it. `typed` must be the sandbox's name exactly.
    pub fn delete(&self, name: &str, typed: &str) -> Result<u64, String> {
        let s = self.get(name)?;
        if typed != s.name {
            self.audit("sandbox.delete.refused", &s.name);
            return Err(format!("type the name of the sandbox ({}) to delete it; nothing was deleted", s.name));
        }
        let freed = self.storage(name);
        rootfs::remove_tree(&self.dir_of(name))?;
        self.audit("sandbox.delete", &format!("{} ({} MB freed)", s.name, freed >> 20));
        Ok(freed)
    }

    pub fn set_network(&self, name: &str, on: bool) -> Result<(), String> {
        let mut s = self.get(name)?;
        s.network = on;
        self.save(&s)?;
        self.audit("sandbox.network", &format!("{name} {}", if on { "on" } else { "off" }));
        Ok(())
    }

    pub fn add_mount(&self, name: &str, src: &str, dst: &str, rw: bool, scope: &dyn Fn(&Path, bool) -> Result<(), String>) -> Result<Mount, String> {
        let mut s = self.get(name)?;
        let mut all: Vec<(String, String, bool)> = s.mounts.iter().map(|m| (m.src.clone(), m.dst.clone(), m.rw)).collect();
        all.push((src.into(), dst.into(), rw));
        let v = self.validate_mounts(&all, scope)?;
        let m = v.last().cloned().ok_or("no mount")?;
        s.mounts = v;
        self.save(&s)?;
        self.audit("sandbox.mount", &format!("{name}: {} at {} {}", m.src, m.dst, if m.rw { "read-write" } else { "read-only" }));
        Ok(m)
    }

    pub fn remove_mount(&self, name: &str, dst: &str) -> Result<(), String> {
        let mut s = self.get(name)?;
        let before = s.mounts.len();
        s.mounts.retain(|m| m.dst != dst);
        if s.mounts.len() == before {
            return Err(format!("nothing is mounted at {dst}"));
        }
        self.save(&s)?;
        self.audit("sandbox.unmount", &format!("{name}: {dst}"));
        Ok(())
    }

    // ---- running

    /// The exact argument array that runs `cmd` in the sandbox: `[iyakki, linuxapp-init, --root, ..., --, cmd...]`.
    pub fn exec_argv(&self, name: &str, cmd: &[String], o: &ExecOpts) -> Result<Vec<String>, String> {
        let s = self.get(name)?;
        self.exec_argv_with(&s, cmd, o, None)
    }

    fn exec_argv_with(&self, s: &Sandbox, cmd: &[String], o: &ExecOpts, extra: Option<Mount>) -> Result<Vec<String>, String> {
        let name = s.name.as_str();
        let root = self.rootfs_of(name);
        if !root.is_dir() {
            return Err(format!("the files of {name} are missing; delete the sandbox and create it again"));
        }
        if cmd.is_empty() || cmd[0].is_empty() {
            return Err("no command".into());
        }
        // a folder removed or turned into a link since it was mounted is refused now, not followed
        for m in &s.mounts {
            let ok = mount_source_ok(&m.src, &self.data)?;
            if ok.display().to_string() != m.src {
                return Err(format!("{} is not the folder that was mounted any more; remove the mount and add it again", m.src));
            }
        }
        let canon_root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
        let mut sp = init::Spec::new(&canon_root.display().to_string(), name, cmd.to_vec());
        sp.idmap = if s.id_base > 0 { IdMap::Multi(s.id_base) } else { IdMap::Single };
        (sp.uid, sp.gid) = if o.root { (0, 0) } else { (1000, 1000) };
        sp.net = s.network;
        sp.binds = s.mounts.iter().chain(extra.iter()).map(|m| Bind { src: m.src.clone(), dst: m.dst.clone(), rw: m.rw }).collect();
        sp.cwd = if !o.cwd.is_empty() { o.cwd.clone() } else if o.root { "/root".into() } else { "/home/user".into() };
        sp.env = o.env.clone();
        sp.mem_mb = s.mem_mb;
        sp.cpu_pct = s.cpu_pct;
        sp.pids = s.pids;
        sp.validate()?;
        let mut argv = vec![self.exe.display().to_string()];
        argv.extend(sp.to_args());
        Ok(argv)
    }

    /// [`LinuxApps::exec_argv`] with one more folder of IYAKKI's own mounted read-write inside (the runtime folder of a Linux window,
    /// `docs/LINUX-WINDOWS.md`): `run_dir` is made by IYAKKI under the sandbox's own folder, so it is not checked as a person's mount.
    pub fn exec_argv_with_run(&self, name: &str, cmd: &[String], o: &ExecOpts, run_dir: &Path, dst: &str) -> Result<Vec<String>, String> {
        let s = self.get(name)?;
        let own = self.dir_of(name);
        let canon = std::fs::canonicalize(run_dir).map_err(|e| format!("{}: {e}", run_dir.display()))?;
        let canon_own = std::fs::canonicalize(&own).map_err(|e| e.to_string())?;
        if canon.parent() != Some(canon_own.as_path()) {
            return Err("the runtime folder must be a folder of the sandbox itself".into());
        }
        self.exec_argv_with(&s, cmd, o, Some(Mount { src: canon.display().to_string(), dst: dst.to_string(), rw: true }))
    }

    /// Write one line to the Linux apps audit log (and the shell's audit log when one is attached).
    pub fn note(&self, action: &str, detail: &str) {
        self.audit(action, detail);
    }

    /// The id offset of this sandbox's files on the host (`None`: the person's own id only).
    pub fn id_shift(&self, name: &str) -> Option<u32> {
        self.get(name).ok().and_then(|s| (s.id_base > 0).then_some(s.id_base))
    }

    /// Run a command in the sandbox and wait for it.
    pub fn exec(&self, name: &str, cmd: &[String], o: &ExecOpts) -> Result<Out, String> {
        let argv = self.exec_argv(name, cmd, o)?;
        // the audit line names the program and the number of arguments, not the arguments (they can hold passwords)
        self.audit("sandbox.exec", &format!("{name}: {} ({} argument(s)){}", cmd[0], cmd.len() - 1, if o.root { " as root inside" } else { "" }));
        Ok(self.run.run(&self.dir_of(name), &argv, &[], o.timeout, OUT_CAP, o.sink.clone()))
    }

    /// A command line typed in the terminal: split into words (no shell, no expansion) and run as the sandbox user.
    pub fn run_line(&self, name: &str, line: &str, sink: Option<Sink>) -> Result<Out, String> {
        let words = crate::exec::split_words(line)?;
        self.exec(name, &words, &ExecOpts { sink, ..Default::default() })
    }

    // ---- apt

    fn apt_opts(&self, sink: Option<Sink>, secs: u64) -> ExecOpts {
        ExecOpts { root: true, timeout: Duration::from_secs(secs), sink, env: vec![("DEBIAN_FRONTEND".into(), "noninteractive".into()), ("LC_ALL".into(), "C.UTF-8".into())], cwd: "/root".into() }
    }

    fn need_net(&self, name: &str) -> Result<(), String> {
        if self.get(name)?.network { Ok(()) } else { Err(format!("{name} has its network switched off; turn the network on to download packages")) }
    }

    fn apt_run(&self, name: &str, args: &[&str], sink: Option<Sink>, secs: u64) -> Result<Out, String> {
        let cmd: Vec<String> = std::iter::once("apt-get").chain(args.iter().copied()).map(str::to_string).collect();
        self.exec(name, &cmd, &self.apt_opts(sink, secs))
    }

    fn check_disk(&self) -> Result<(), String> {
        self.check_space(512 << 20)
    }

    /// `apt-get update` then `apt-get install` of the named packages. Returns the output of the install.
    pub fn apt_install(&self, name: &str, pkgs: &[String], sink: Option<Sink>) -> Result<Out, String> {
        check_packages(pkgs)?;
        self.need_net(name)?;
        self.check_disk()?;
        self.audit("sandbox.apt.install", &format!("{name}: {}", pkgs.join(" ")));
        let u = self.apt_run(name, &["update"], sink.clone(), 900)?;
        if !u.ok() {
            return Ok(u);
        }
        let mut args = vec!["install", "-y", "--no-install-recommends"];
        args.extend(pkgs.iter().map(String::as_str));
        let o = self.apt_run(name, &args, sink, 3600)?;
        self.audit(if o.ok() { "sandbox.apt.installed" } else { "sandbox.apt.failed" }, &format!("{name}: {}", pkgs.join(" ")));
        Ok(o)
    }

    pub fn apt_remove(&self, name: &str, pkgs: &[String], sink: Option<Sink>) -> Result<Out, String> {
        check_packages(pkgs)?;
        self.audit("sandbox.apt.remove", &format!("{name}: {}", pkgs.join(" ")));
        let mut args = vec!["remove", "-y"];
        args.extend(pkgs.iter().map(String::as_str));
        self.apt_run(name, &args, sink, 1800)
    }

    pub fn apt_upgrade(&self, name: &str, sink: Option<Sink>) -> Result<Out, String> {
        self.need_net(name)?;
        self.check_disk()?;
        self.audit("sandbox.apt.upgrade", name);
        let u = self.apt_run(name, &["update"], sink.clone(), 900)?;
        if !u.ok() {
            return Ok(u);
        }
        self.apt_run(name, &["upgrade", "-y"], sink, 3600)
    }

    /// `apt-get update` alone (refresh the package lists).
    pub fn apt_update(&self, name: &str, sink: Option<Sink>) -> Result<Out, String> {
        self.need_net(name)?;
        self.audit("sandbox.apt.update", name);
        self.apt_run(name, &["update"], sink, 900)
    }

    pub fn apt_search(&self, name: &str, words: &[String]) -> Result<Vec<(String, String)>, String> {
        check_search(words)?;
        let mut cmd: Vec<String> = vec!["apt-cache".into(), "search".into()];
        cmd.extend(words.iter().cloned());
        let o = self.exec(name, &cmd, &ExecOpts { root: true, cwd: "/root".into(), timeout: Duration::from_secs(60), ..Default::default() })?;
        if !o.ok() {
            return Err(format!("apt-cache search failed: {}", o.combined().trim()));
        }
        Ok(o.stdout.lines().take(200).filter_map(|l| l.split_once(" - ").map(|(a, b)| (a.to_string(), b.to_string()))).collect())
    }

    /// What is installed: `(package, version)`.
    pub fn installed(&self, name: &str) -> Result<Vec<(String, String)>, String> {
        let cmd: Vec<String> = ["dpkg-query", "-W", "-f=${binary:Package}\t${Version}\n"].iter().map(|s| s.to_string()).collect();
        let o = self.exec(name, &cmd, &ExecOpts { root: true, cwd: "/root".into(), timeout: Duration::from_secs(60), ..Default::default() })?;
        if !o.ok() {
            return Err(format!("could not list packages: {}", o.combined().trim()));
        }
        Ok(o.stdout.lines().filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string()))).collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateReq {
    pub name: String,
    pub release: String,
    pub network: bool,
    /// `(host folder, mount point or empty for /mnt/<name>, read-write)`
    pub mounts: Vec<(String, String, bool)>,
}

/// The user, `apt` and the package manager's service policy inside a freshly unpacked root.
pub fn finish_rootfs(root: &Path, shift: Option<u32>) -> Result<(), String> {
    rootfs::make_dir_in(root, "home/user", 0o755, (1000, 1000), shift)?;
    rootfs::make_dir_in(root, "mnt", 0o755, (0, 0), shift)?;
    rootfs::make_dir_in(root, "tmp", 0o1777, (0, 0), shift)?;
    let shell = if root.join("bin/bash").exists() || root.join("usr/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
    let mut passwd = String::from_utf8_lossy(&rootfs::read_file_in(root, "etc/passwd").unwrap_or_default()).into_owned();
    if !passwd.lines().any(|l| l.starts_with("user:")) {
        if !passwd.is_empty() && !passwd.ends_with('\n') {
            passwd.push('\n');
        }
        passwd.push_str(&format!("user:x:1000:1000:IYAKKI sandbox user:/home/user:{shell}\n"));
        rootfs::write_file_in(root, "etc/passwd", passwd.as_bytes(), 0o644, (0, 0), shift)?;
    }
    let mut group = String::from_utf8_lossy(&rootfs::read_file_in(root, "etc/group").unwrap_or_default()).into_owned();
    if !group.lines().any(|l| l.starts_with("user:")) {
        if !group.is_empty() && !group.ends_with('\n') {
            group.push('\n');
        }
        group.push_str("user:x:1000:\n");
        rootfs::write_file_in(root, "etc/group", group.as_bytes(), 0o644, (0, 0), shift)?;
    }
    // no package may start a service inside the sandbox; apt runs its downloads as root (there is one powerless root id, no `_apt`)
    rootfs::write_file_in(root, "usr/sbin/policy-rc.d", b"#!/bin/sh\nexit 101\n", 0o755, (0, 0), shift)?;
    rootfs::write_file_in(root, "etc/apt/apt.conf.d/90iyakki", b"APT::Sandbox::User \"root\";\nAcquire::Retries \"3\";\nDpkg::Use-Pty \"0\";\n", 0o644, (0, 0), shift)?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the AI IDE's runner choice

static IDE_CONTEXT: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// The AI IDE tells which data folder and person it works for (like `set_cli_bin`), so its run/test commands can go to a sandbox.
pub fn set_ide_context(data: &Path, user: &str) {
    *IDE_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()) = Some((data.to_path_buf(), user.to_string()));
}

/// The data folder and person the AI IDE works for, when it has said (see [`set_ide_context`]).
pub fn ide_context() -> Option<(PathBuf, String)> {
    IDE_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The workspace folder that holds `cwd` (the one with a `.iyakki/project.json`).
pub fn workspace_root_of(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors().find(|a| crate::workspace::ProjectConfig::path(a).is_file()).map(Path::to_path_buf)
}

/// Choose where a workspace's commands run: a sandbox name, or `None` for the machine. The sandbox must exist.
pub fn set_workspace_sandbox(la: &LinuxApps, root: &Path, name: Option<&str>) -> Result<String, String> {
    let mut c = crate::workspace::ProjectConfig::load(root)?;
    match name {
        Some(n) => {
            la.get(n)?;
            c.sandbox = n.to_string();
        }
        None => c.sandbox.clear(),
    }
    c.save(root)?;
    la.audit_note("sandbox.runner", &format!("{} -> {}", root.display(), name.unwrap_or("this machine")));
    Ok(match name {
        Some(n) => format!("run and test commands of this workspace now run in the sandbox {n} (the workspace folder is mounted read-write at {WORKSPACE_MOUNT})"),
        None => "run and test commands run on this machine again".into(),
    })
}

/// Where the workspace appears inside a sandbox that runs its commands.
pub const WORKSPACE_MOUNT: &str = "/mnt/workspace";

/// A command of the AI IDE (run, test, terminal) for the working folder `cwd`: unchanged when the workspace has no sandbox chosen,
/// else the argument array that runs it in the sandbox with the workspace folder mounted read-write (never the rest of the host).
/// Commands are run as the sandbox user, in the same folder inside.
pub fn ide_wrap(cwd: &Path, argv: Vec<String>) -> Result<Vec<String>, String> {
    let ctx = IDE_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    ide_wrap_with(ctx, &exe, cwd, argv)
}

/// [`ide_wrap`] with the context and the program given (tests).
pub fn ide_wrap_with(ctx: Option<(PathBuf, String)>, exe: &Path, cwd: &Path, argv: Vec<String>) -> Result<Vec<String>, String> {
    let Some(root) = workspace_root_of(cwd) else { return Ok(argv) };
    let Some(name) = crate::workspace::ProjectConfig::load(&root).ok().map(|c| c.sandbox).filter(|s| !s.is_empty()) else { return Ok(argv) };
    let (data, user) = ctx.ok_or("the AI IDE has not said which person it works for, so the sandbox cannot be used")?;
    let la = LinuxApps::new(&data, &user, exe, Arc::new(crate::wsexec::System));
    la.ide_argv(&name, &root, cwd, &argv)
}

impl LinuxApps {
    fn audit_note(&self, action: &str, detail: &str) {
        self.audit(action, detail);
    }

    /// The argument array that runs `argv` in the sandbox `name` with the workspace `root` mounted read-write at [`WORKSPACE_MOUNT`].
    pub fn ide_argv(&self, name: &str, root: &Path, cwd: &Path, argv: &[String]) -> Result<Vec<String>, String> {
        let src = mount_source_ok(&root.display().to_string(), &self.data)?;
        let rel = std::fs::canonicalize(cwd).ok().and_then(|c| c.strip_prefix(&src).ok().map(Path::to_path_buf)).unwrap_or_default();
        let inside = if rel.as_os_str().is_empty() { WORKSPACE_MOUNT.to_string() } else { format!("{WORKSPACE_MOUNT}/{}", rel.display()) };
        let mut s = self.get(name)?;
        s.mounts.retain(|m| m.dst != WORKSPACE_MOUNT);
        // the sandbox's own record is not changed: the mount exists for this one command
        let o = ExecOpts { cwd: inside, ..Default::default() };
        let mut a = self.exec_argv_with(&s, argv, &o, Some(Mount { src: src.display().to_string(), dst: WORKSPACE_MOUNT.into(), rw: true }))?;
        self.audit("sandbox.ide-run", &format!("{name}: {} ({} argument(s)) in {}", argv.first().map_or("", String::as_str), argv.len().saturating_sub(1), WORKSPACE_MOUNT));
        a.shrink_to_fit();
        Ok(a)
    }
}

/// How many sandboxes exist for all people (Quick install's "installed" state).
pub fn count_all(data: &Path) -> usize {
    std::fs::read_dir(data.join(DIR)).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).map(|u| std::fs::read_dir(u.path()).into_iter().flatten().flatten().filter(|s| s.path().join("sandbox.json").is_file()).count()).sum()
}

// ------------------------------------------------------------------------------------------------ jobs

use crate::jobs::{Cmd, Spec};
use crate::quickinstall::Launch;

/// The detached job that runs `iyakki linux-apps --data D --user U <args>` (create, install packages, upgrade, ...).
pub fn job_spec(l: &Launch, user: &str, label: &str, args: &[String]) -> Spec {
    let mut argv = vec![l.exe.display().to_string(), "linux-apps".into(), "--data".into(), l.data.display().to_string(), "--user".into(), user.into()];
    argv.extend(args.iter().cloned());
    Spec { label: label.into(), cmd: Cmd::Argv(argv), cwd: l.data.clone(), workspace: WORKSPACE.into() }
}

/// Is this detached job one of Linux apps' (its command has the `linux-apps` word)?
pub fn is_linux_job(command: &str) -> bool {
    command.split_whitespace().nth(1) == Some("linux-apps")
}

/// `iyakki linux-apps --data D --user U <verb> ...`: what the jobs run.
///
/// * `create <name> <release> [--net on|off] [--mount SRC[:DST[:rw]]]...`
/// * `install <name> <package>...`, `remove <name> <package>...`, `upgrade <name>`, `update <name>`
/// * `run <name> [--root] -- <command>...`
/// * `import <name> <tarball> [--net on|off]`
pub fn cli_main(args: &[String]) -> i32 {
    let mut rest: Vec<String> = Vec::new();
    let (mut data, mut user) = (PathBuf::from("/var/lib/iyakki"), std::env::var("USER").unwrap_or_else(|_| "user".into()));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = PathBuf::from(it.next().cloned().unwrap_or_default()),
            "--user" => user = it.next().cloned().unwrap_or_default(),
            _ => rest.push(a.clone()),
        }
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            println!("error: cannot find this program: {e}");
            return 1;
        }
    };
    let la = LinuxApps::new(&data, &user, &exe, Arc::new(crate::wsexec::System));
    match cli_run(&la, &rest) {
        Ok(()) => 0,
        Err(e) => {
            println!("error: {e}");
            1
        }
    }
}

/// Run a command with the output of the sandbox copied to stdout while it runs.
pub(crate) fn stream<T>(f: impl FnOnce(Sink) -> T) -> T {
    let sink: Sink = Arc::new(Mutex::new(String::new()));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s2, d2) = (sink.clone(), done.clone());
    let printer = std::thread::spawn(move || {
        let mut printed = 0usize;
        loop {
            let finished = d2.load(std::sync::atomic::Ordering::SeqCst);
            {
                let g = s2.lock().unwrap_or_else(|e| e.into_inner());
                if g.len() > printed {
                    let mut end = g.len();
                    while !g.is_char_boundary(end) {
                        end -= 1;
                    }
                    print!("{}", &g[printed..end]);
                    printed = end;
                }
            }
            use std::io::Write;
            let _ = std::io::stdout().flush();
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    let r = f(sink);
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = printer.join();
    r
}

fn show(o: Result<Out, String>) -> Result<(), String> {
    let o = o?;
    if o.timed_out {
        return Err("timed out".into());
    }
    if o.ok() { Ok(()) } else { Err(format!("exit status {}", o.code.map_or("unknown".to_string(), |c| c.to_string()))) }
}

pub fn cli_run(la: &LinuxApps, args: &[String]) -> Result<(), String> {
    let verb = args.first().map(String::as_str).ok_or("usage: linux-apps create|install|remove|upgrade|update|run|import ...")?;
    let name = args.get(1).cloned().unwrap_or_default();
    let tail = if args.len() > 2 { &args[2..] } else { &[] };
    match verb {
        "create" => {
            let release = tail.first().cloned().ok_or("usage: create <name> <release>")?;
            let (mut net, mut mounts) = (false, Vec::new());
            let mut i = 1;
            while i < tail.len() {
                match tail[i].as_str() {
                    "--net" => {
                        net = tail.get(i + 1).map(String::as_str) == Some("on");
                        i += 1;
                    }
                    "--mount" => {
                        let spec = tail.get(i + 1).cloned().ok_or("--mount needs a value")?;
                        let rw = spec.ends_with(":rw");
                        let spec = spec.trim_end_matches(":rw").trim_end_matches(":ro").to_string();
                        let (src, dst) = spec.split_once(':').map_or((spec.clone(), String::new()), |(a, b)| (a.to_string(), b.to_string()));
                        mounts.push((src, dst, rw));
                        i += 1;
                    }
                    other => return Err(format!("unknown option {other}")),
                }
                i += 1;
            }
            let st = iyakki_proxy::https::UreqStreamer::default();
            let req = CreateReq { name, release, network: net, mounts };
            la.create(&req, &st, &|_, _| Ok(()), &mut |l| println!("{l}")).map(|_| ())
        }
        "install" => stream(|sink| show(la.apt_install(&name, tail, Some(sink)))),
        "remove" => stream(|sink| show(la.apt_remove(&name, tail, Some(sink)))),
        "upgrade" => stream(|sink| show(la.apt_upgrade(&name, Some(sink)))),
        "update" => stream(|sink| show(la.apt_update(&name, Some(sink)))),
        "run" => {
            let (root, cmd) = match tail.first().map(String::as_str) {
                Some("--root") => (true, tail.get(2..).unwrap_or(&[]).to_vec()),
                _ => (false, tail.get(1..).unwrap_or(&[]).to_vec()),
            };
            stream(|sink| show(la.exec(&name, &cmd, &ExecOpts { root, sink: Some(sink), timeout: Duration::from_secs(3600), ..Default::default() })))
        }
        "import" => {
            let tarball = tail.first().cloned().ok_or("usage: import <name> <tarball>")?;
            let net = tail.iter().position(|a| a == "--net").and_then(|i| tail.get(i + 1)).map(String::as_str) == Some("on");
            la.import(&name, Path::new(&tarball), net, &mut |l| println!("{l}")).map(|_| ())
        }
        other => Err(format!("unknown verb `{other}`")),
    }
}

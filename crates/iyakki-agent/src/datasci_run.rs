//! Running the data science toolkit's Python in the Linux apps sandbox with NO network (F-FLOW-31, docs/development/DATA-SCIENCE-ADDON-PLAN.md
//! section 3.5): the argument list of one run, the fixed self-test that the install and "Verify toolkit" run, and the answer in one sentence.
//! The runs go through the namespace launcher of the sandbox runner (`sandbox::exec::NsLauncher`, this program's `linuxapp-init`), so a test
//! passes a fake one and reads the arguments; nothing here fetches anything.
//!
//! Python runs in isolated mode (`-I`): it ignores `PYTHONPATH`, so the packages folder is put on the path by a one-line start-up that then
//! runs the script, instead of by the environment (decision D-DATASCI-RUN-ARGV).
// SPDX-License-Identifier: Apache-2.0

use crate::linuxapps_init::{Bind, IdMap, Spec};
use crate::sandbox::exec::{NsLauncher, NsOutput};
use crate::datasci_io::{self, RunIo, Sink};
use iyakki_shared::datasci;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the toolkit appears inside the sandbox.
pub const PY_IN: &str = "/opt/datasci/python";
pub const SITE_IN: &str = "/opt/datasci/site";
pub const WORK_IN: &str = "/work";
/// What one self-test may take.
pub const SELFTEST_SECS: u64 = 120;

/// The limits of one run (the plan's section 3.5 defaults).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub mem_mb: u32,
    pub cpu_pct: u32,
    pub pids: u32,
    pub tmp_mb: u32,
    pub secs: u64,
}

impl Limits {
    pub const SELFTEST: Limits = Limits { mem_mb: 1024, cpu_pct: 200, pids: 64, tmp_mb: 512, secs: SELFTEST_SECS };
}

/// The fixed self-test, embedded in IYAKKI (never read from a file the person can change). It prints one line starting `SELFTEST ok` or
/// `SELFTEST failed`.
pub const SELFTEST_SCRIPT: &str = r#"import sys, time
try:
    import numpy as np
    import sklearn
    from sklearn.ensemble import RandomForestClassifier
    import cv2
    rng = np.random.RandomState(7)
    n = 300
    x = rng.randn(n, 4)
    y = (x[:, 0] + 0.5 * x[:, 1] > 0).astype(int)
    t0 = time.time()
    m = RandomForestClassifier(n_estimators=30, random_state=0).fit(x[:200], y[:200])
    score = m.score(x[200:], y[200:])
    took = time.time() - t0
    if score <= 0.9:
        print("SELFTEST failed the model scored only %.2f" % score)
        sys.exit(1)
    img = (rng.rand(40, 60, 3) * 255).astype("uint8")
    small = cv2.resize(img, (30, 20))
    if small.shape != (20, 30, 3):
        print("SELFTEST failed OpenCV resized to the wrong shape")
        sys.exit(1)
    print("SELFTEST ok scikit-learn %s fits a small model in %.1f s; OpenCV %s resizes an image" % (sklearn.__version__, took, cv2.__version__))
except Exception as e:
    print("SELFTEST failed %s: %s" % (type(e).__name__, str(e)[:120].replace("\n", " ")))
    sys.exit(1)
"#;

/// The command inside the sandbox: isolated Python, the packages folder put on the path, then the script of the work folder.
pub fn command(script: &str) -> Vec<String> {
    vec![
        format!("{PY_IN}/bin/python3"),
        "-I".into(),
        "-B".into(),
        "-c".into(),
        format!("import sys, runpy; sys.path.insert(0, '{SITE_IN}'); runpy.run_path('{WORK_IN}/{script}', run_name='__main__')"),
    ]
}

/// The sandbox spec of one run: no network, the toolkit read-only, one scratch folder writable, limits on memory, processor share, process
/// count and temporary space, the system-call filter on, not root inside.
pub fn spec_for(root: &Path, data: &Path, work: &Path, script: &str, lim: Limits) -> Spec {
    let mut sp = Spec::new(&root.display().to_string(), "datasci", command(script));
    sp.uid = 1000;
    sp.gid = 1000;
    sp.idmap = IdMap::Single;
    sp.net = false;
    sp.veth = None;
    sp.binds = vec![
        Bind { src: datasci::dir(data).join("python").display().to_string(), dst: PY_IN.into(), rw: false },
        Bind { src: datasci::site_dir(data).display().to_string(), dst: SITE_IN.into(), rw: false },
        Bind { src: work.display().to_string(), dst: WORK_IN.into(), rw: true },
    ];
    sp.tmp_mb = lim.tmp_mb;
    sp.cwd = WORK_IN.into();
    sp.env = [("OMP_NUM_THREADS", "2"), ("OPENBLAS_NUM_THREADS", "2"), ("PYTHONDONTWRITEBYTECODE", "1"), ("HOME", WORK_IN)].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    sp.mem_mb = lim.mem_mb;
    sp.cpu_pct = lim.cpu_pct;
    sp.pids = lim.pids;
    sp.seccomp = true;
    sp
}

/// Reads the self-test's answer: the sentence after `SELFTEST ok`, or why it failed in words.
pub fn parse_selftest(out: &NsOutput) -> Result<String, String> {
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let line = text.lines().rev().find(|l| l.starts_with("SELFTEST ")).map(str::to_string);
    if out.timed_out {
        return Err(format!("the toolkit self-test did not finish within {SELFTEST_SECS} seconds and was stopped"));
    }
    match line {
        Some(l) if out.code == Some(0) && l.starts_with("SELFTEST ok ") => Ok(l["SELFTEST ok ".len()..].chars().filter(|c| !c.is_control()).take(200).collect()),
        Some(l) => Err(format!("the toolkit self-test failed: {}", l.trim_start_matches("SELFTEST failed").trim().chars().filter(|c| !c.is_control()).take(200).collect::<String>())),
        None => {
            let err = String::from_utf8_lossy(&out.stderr).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it printed nothing").chars().filter(|c| !c.is_control()).take(160).collect::<String>();
            Err(format!("the toolkit self-test did not run ({}): {err}", out.code.map_or_else(|| "stopped by a signal".to_string(), |c| format!("exit {c}"))))
        }
    }
}

/// Run the self-test in the sandbox with no network and say what it saw. `scratch` is a folder this call may make and empties afterwards.
pub fn selftest(launcher: &dyn NsLauncher, root: &Path, data: &Path, scratch: &Path) -> Result<String, String> {
    if !datasci::python_bin(data).exists() || !datasci::site_dir(data).is_dir() {
        return Err("the toolkit is not in place (no Python or no packages folder)".into());
    }
    let work: PathBuf = scratch.join("selftest");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let result = (|| {
        std::fs::write(work.join("selftest.py"), SELFTEST_SCRIPT).map_err(|e| e.to_string())?;
        let sp = spec_for(root, data, &work, "selftest.py", Limits::SELFTEST);
        sp.validate()?;
        let out = launcher.launch(&sp.to_args(), Duration::from_secs(SELFTEST_SECS), 64 * 1024)?;
        parse_selftest(&out)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The most a `python_run` script may be, and the longest it may run.
pub const SCRIPT_MAX: usize = 20_000;
pub const RUN_SECS_DEFAULT: u64 = 60;
pub const RUN_SECS_MAX: u64 = 300;
const OUT_FILES_MAX: usize = 20;
const OUT_FILE_BYTES: u64 = 8 << 20;

/// What a script run gave back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunReport {
    /// What the script printed (the end of it when it printed a lot; control characters other than new lines are gone).
    pub stdout: String,
    /// The files the script left in its work folder and where they were put (empty when it made none).
    pub files: Vec<String>,
    pub keep: Option<PathBuf>,
}

/// `python_run`: the person's script in the sandbox with no network (the run model of plan section 3.5 with the person's time limit).
/// `scratch` holds the work folder (emptied afterwards); the files the script made are copied to `keep` (at most 20, 8 MB each). Failure
/// words say what the sandbox saw; a toolkit that is not there runs nothing.
pub fn run_script(launcher: &dyn NsLauncher, root: &Path, data: &Path, scratch: &Path, keep: &Path, script: &str, secs: u64) -> Result<RunReport, String> {
    run_script_io(launcher, root, data, scratch, keep, script, secs, &RunIo::default(), &NoSink)
}

/// A sink for runs that ask for no tables or pictures: nothing is registered anywhere.
pub struct NoSink;

impl Sink for NoSink {
    fn table(&self, _: &str, _: &Path, _: usize, _: usize) -> Result<String, String> {
        Err("no Data service is attached to this run".into())
    }
    fn image(&self, _: &str, _: &Path) -> Result<String, String> {
        Err("no picture folder is attached to this run".into())
    }
}

/// `run_script` with inputs copied in, a frame cap and named outputs (see `datasci_io`). With an empty `io` it is the plain run: every
/// top-level file the script left is kept. With outputs listed, ONLY those files of `out/` are collected, checked by kind, and tables and
/// pictures are handed to `sink`.
#[allow(clippy::too_many_arguments)]
pub fn run_script_io(launcher: &dyn NsLauncher, root: &Path, data: &Path, scratch: &Path, keep: &Path, script: &str, secs: u64, io: &RunIo, sink: &dyn Sink) -> Result<RunReport, String> {
    if script.trim().is_empty() {
        return Err("the script is empty".into());
    }
    if script.len() > SCRIPT_MAX {
        return Err(format!("the script is longer than {SCRIPT_MAX} characters"));
    }
    if !(1..=RUN_SECS_MAX).contains(&secs) {
        return Err(format!("the time limit is 1 to {RUN_SECS_MAX} seconds"));
    }
    if !datasci::python_bin(data).exists() || !datasci::site_dir(data).is_dir() {
        return Err("the data science toolkit is not installed (Store, Setup)".into());
    }
    let work = scratch.join("run");
    datasci_io::unlock(&work);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let result = (|| {
        std::fs::write(work.join("script.py"), script).map_err(|e| e.to_string())?;
        let given = if io.is_empty() { Vec::new() } else { datasci_io::prepare(&work, io)? };
        let mut sp = spec_for(root, data, &work, "script.py", Limits { secs, ..Limits::SELFTEST });
        if !io.is_empty() {
            sp.env.push(("IYAKKI_FRAME_CAP".to_string(), io.cap().to_string()));
        }
        sp.validate()?;
        let out = launcher.launch(&sp.to_args(), Duration::from_secs(secs), 64 * 1024)?;
        if out.timed_out {
            return Err(format!("the script did not finish within {secs} seconds and was stopped"));
        }
        let clean = |b: &[u8]| String::from_utf8_lossy(b).chars().filter(|c| !c.is_control() || *c == '\n').collect::<String>();
        let stdout = clean(&out.stdout);
        if out.code != Some(0) {
            let err = clean(&out.stderr);
            let tail: String = err.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
            return Err(format!("the script failed ({}): {}", out.code.map_or_else(|| "stopped by a signal".to_string(), |c| format!("exit {c}")), tail.chars().take(400).collect::<String>()));
        }
        if !io.outputs.is_empty() {
            let mut files = given;
            files.extend(datasci_io::collect(&work, io, keep, sink)?);
            let tail = if stdout.len() > 4000 { format!("...{}", &stdout[stdout.char_indices().rev().nth(3999).map_or(0, |x| x.0)..]) } else { stdout };
            return Ok(RunReport { stdout: tail, keep: keep.is_dir().then(|| keep.to_path_buf()), files });
        }
        let mut files = Vec::new();
        let mut names: Vec<_> = std::fs::read_dir(&work).map_err(|e| e.to_string())?.flatten().collect();
        names.sort_by_key(|e| e.file_name());
        for e in names {
            let name = e.file_name().to_string_lossy().to_string();
            let meta = e.metadata().map_err(|e| e.to_string())?;
            // a link or a folder is not a result, and the script itself is not either
            if name == "script.py" || !meta.is_file() || meta.len() > OUT_FILE_BYTES || files.len() >= OUT_FILES_MAX || name.starts_with('.') {
                continue;
            }
            std::fs::create_dir_all(keep).map_err(|e| format!("{}: {e}", keep.display()))?;
            std::fs::copy(e.path(), keep.join(&name)).map_err(|e| e.to_string())?;
            files.push(format!("{name} ({} bytes)", meta.len()));
        }
        let tail = if stdout.len() > 4000 { format!("...{}", &stdout[stdout.char_indices().rev().nth(3999).map_or(0, |x| x.0)..]) } else { stdout };
        Ok(RunReport { stdout: tail, keep: (!files.is_empty()).then(|| keep.to_path_buf()), files })
    })();
    datasci_io::unlock(&work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// `run_script` in the person's Linux apps sandbox; the made files go to `<data>/datasci.out/<user>/<stamp>`.
pub fn run_script_for(data: &Path, user: &str, script: &str, secs: u64) -> Result<RunReport, String> {
    run_script_for_io(data, user, script, secs, &RunIo::default())
}

/// [`run_script_for`] with inputs, a frame cap and named outputs: tables go to the Data service, pictures to the person's pictures.
pub fn run_script_for_io(data: &Path, user: &str, script: &str, secs: u64, io: &RunIo) -> Result<RunReport, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let la = crate::linuxapps::LinuxApps::new(data, user, &exe, std::sync::Arc::new(crate::wsexec::System));
    let root = la.rootfs_of("debian");
    if !root.is_dir() {
        return Err("the script runs in the Linux apps sandbox, which is not installed: install Linux apps first".into());
    }
    let user_dir: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    let scratch = data.join("datasci.scratch");
    let keep = data.join("datasci.out").join(user_dir).join(iyakki_clock::now_unix().to_string());
    let sink = DataSink { user: user.to_string(), home: crate::scope::home_root_for_writes(user, data) };
    let r = run_script_io(&crate::sandbox::exec::SelfLauncher, &root, data, &scratch, &keep, script, secs, io, &sink);
    let _ = std::fs::remove_dir_all(&scratch);
    r
}

/// The real sink: a table is copied to `Data/ai-output/` in the person's home and opened in the Data service (so the Data tab lists it);
/// a picture is copied to `Pictures/ai-output/`. Neither ever overwrites: a taken name gets `-2`, `-3` ...
pub struct DataSink {
    pub user: String,
    pub home: PathBuf,
}

/// A free name in `dir`: `name`, else `stem-2.ext`, `stem-3.ext` ...
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').map_or((name, String::new()), |(s, e)| (s, format!(".{e}")));
    (2..).map(|n| dir.join(format!("{stem}-{n}{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

impl DataSink {
    fn put(&self, folder: &str, name: &str, file: &Path) -> Result<PathBuf, String> {
        let dir = self.home.join(folder).join("ai-output");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let to = free_name(&dir, name);
        std::fs::copy(file, &to).map_err(|e| e.to_string())?;
        Ok(to)
    }
}

impl Sink for DataSink {
    fn table(&self, name: &str, file: &Path, _rows: usize, _columns: usize) -> Result<String, String> {
        let to = self.put("Data", name, file)?;
        crate::datatools::register_file(&self.user, &to)
    }
    fn image(&self, name: &str, file: &Path) -> Result<String, String> {
        let to = self.put("Pictures", name, file)?;
        Ok(format!("Kept as Pictures/ai-output/{}.", to.file_name().and_then(|n| n.to_str()).unwrap_or(name)))
    }
}

/// What the item shows about the installed toolkit compared with the table of this IYAKKI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Installed and the table asks for the same files.
    Current { selftest: String },
    /// Installed, but the table of this IYAKKI pins other versions or hashes: the Upgrade button.
    UpgradeAvailable { have: usize, wanted: usize },
    /// The folder is there but the record is not usable.
    Broken(String),
}

/// The record's lines are `name version sha256`: the toolkit is current when every pin of the table is in the record with the same hash.
pub fn status(data: &Path, pins: &[datasci::Pin]) -> Status {
    let dir = datasci::dir(data);
    if !dir.exists() {
        return Status::NotInstalled;
    }
    let Some(st) = datasci::installed(data) else { return Status::Broken("the record of the installed toolkit is missing or does not match its folders".into()) };
    let have: Vec<&str> = st.files.iter().map(String::as_str).collect();
    let wanted: Vec<String> = pins.iter().map(|p| format!("{} {} {}", p.name, p.version, p.sha256)).collect();
    if wanted.iter().all(|w| have.contains(&w.as_str())) && wanted.len() == have.len() {
        Status::Current { selftest: st.selftest }
    } else {
        Status::UpgradeAvailable { have: have.len(), wanted: wanted.len() }
    }
}

/// "Verify toolkit": the record is read again and the self-test is run again in the sandbox; a broken toolkit says why.
pub fn verify(launcher: &dyn NsLauncher, root: &Path, data: &Path, scratch: &Path, pins: &[datasci::Pin]) -> Result<String, String> {
    match status(data, pins) {
        Status::NotInstalled => Err("the toolkit is not installed".into()),
        Status::Broken(why) => Err(why),
        Status::Current { .. } | Status::UpgradeAvailable { .. } => selftest(launcher, root, data, scratch),
    }
}

/// The Setup page's view of the toolkit: a short code (`none`, `current`, `upgrade`, `broken`) and a sentence for the card. Reads the record
/// only; nothing runs.
pub fn state_text(data: &Path, pins: &[datasci::Pin]) -> (&'static str, String) {
    match status(data, pins) {
        Status::NotInstalled => ("none", "The data science toolkit is not installed.".into()),
        Status::Current { selftest } => ("current", format!("Installed and up to date. Last self-test: {selftest}")),
        Status::UpgradeAvailable { have, wanted } => ("upgrade", format!("Installed, but this IYAKKI pins other files ({wanted} pinned, {have} installed): Upgrade fetches the pinned ones.")),
        Status::Broken(why) => ("broken", format!("The installed toolkit is not usable: {why}. Install it again.")),
    }
}

/// "Verify toolkit" for a person: the self-test again in the Linux apps sandbox of that person, with no network. Refused in words when the
/// sandbox is not there.
pub fn verify_for(data: &Path, user: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let la = crate::linuxapps::LinuxApps::new(data, user, &exe, std::sync::Arc::new(crate::wsexec::System));
    let root = la.rootfs_of("debian");
    if !root.is_dir() {
        return Err("the toolkit self-test runs in the Linux apps sandbox, which is not installed: install Linux apps first".into());
    }
    let scratch = data.join("datasci.scratch");
    let r = verify(&crate::sandbox::exec::SelfLauncher, &root, data, &scratch, &datasci::PINS);
    let _ = std::fs::remove_dir_all(&scratch);
    r
}

#[cfg(test)]
#[path = "datasci_run_tests.rs"]
mod tests;

//! DOCTOR: what is wrong with this computer, in plain words, each with one safe fix button.
//!
//! [`Facts`] is everything the rules read (a plain struct: tests make up a machine); every rule is one small function from the facts
//! to at most one [`Finding`]; [`diagnose`] runs them all and sorts the result (problems, then warnings, then notes; inside a
//! group the rules' own order). Nothing here reads the machine: the DOCTOR tile gathers the facts on a thread of its own
//! (`apps/doctor.rs`), so a slow probe never freezes the screen.
//!
//! **Fixes.** A [`Fix`] is either "open this Settings page" or one action that needs nobody's consent and changes little
//! (extending developer mode's switch-off time, by the same call and audit line as the Settings button). A fix never accepts a
//! permission popup, a licence or a vault password for the person, never restarts the cluster and never changes the firewall: it
//! opens the page where the person does that (plan decision D4 is still open: no silent restart of k3s).
//!
//! **Privacy.** [`Finding::text`] is a plain sentence for everybody; [`Finding::detail`] may hold an address, a port or a log line
//! and is shown to administrators only (the same split as the startup strip). The diagnostics bundle redacts both.

/// How bad a finding is. The order is the sort order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something that should work does not.
    Problem,
    /// Will turn into a problem, or works badly.
    Warning,
    /// Worth knowing.
    Note,
}

impl Severity {
    pub fn word(self) -> &'static str {
        match self {
            Severity::Problem => "Problem",
            Severity::Warning => "Warning",
            Severity::Note => "Note",
        }
    }
}

/// What a fix button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open this Settings page (`tab:firewall`, the item Settings' `open_item` takes).
    Settings(&'static str),
    /// Developer mode: push its switch-off time four hours on (administrators; audited like the Settings button).
    ExtendDeveloperMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    /// The button's words.
    pub label: &'static str,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Stable name of the rule (tests, the bundle, the audit line of a fix).
    pub id: &'static str,
    pub severity: Severity,
    /// One plain sentence.
    pub text: String,
    /// Addresses, ports, log lines: administrators only.
    pub detail: String,
    pub fix: Option<Fix>,
}

fn open(label: &'static str, item: &'static str) -> Option<Fix> {
    Some(Fix { label, action: Action::Settings(item) })
}

fn finding(id: &'static str, severity: Severity, text: impl Into<String>, detail: impl Into<String>, fix: Option<Fix>) -> Finding {
    Finding { id, severity, text: text.into(), detail: detail.into(), fix }
}

/// A service of the startup registry that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedService {
    pub id: String,
    pub name: String,
    pub reason: String,
    /// The Settings item its Open button leads to, if it has one.
    pub open: Option<&'static str>,
}

/// Everything the rules read.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub now: i64,
    pub uptime_secs: u64,
    // the dev cluster
    pub cluster_installed: bool,
    /// `stopped`, `starting`, `running`, `stopping`, `failed`.
    pub cluster_state: String,
    pub cluster_autostart: bool,
    pub cluster_failed_reason: String,
    /// When k3s last ended without being asked to (unix seconds; 0: never).
    pub cluster_last_exit_at: i64,
    /// The fixed node address is wanted (set up, and not switched off after a refusal).
    pub node_ip_wanted: bool,
    /// An interface carries it now.
    pub node_ip_present: bool,
    // developer mode
    pub dev_on: bool,
    /// When it switches itself off (unix seconds; 0: never).
    pub dev_until: i64,
    pub dev_port: u16,
    /// The firewall's own sentence when it blocks the developer port (`iyakki_fw::devrules::hint`).
    pub dev_firewall_hint: Option<String>,
    // the vault
    /// The signed-in person's vault is locked.
    pub vault_locked: bool,
    /// What waits for the vault, in the notices' words ("ClickHouse", "Matrix registration token").
    pub vault_waiting: Vec<String>,
    // the browser engine (from the kept log, `enginelog.rs`)
    pub engine: crate::enginelog::Health,
    // sound
    pub sound_cards: usize,
    /// The output the person chose (empty: automatic).
    pub sound_chosen: String,
    /// The chosen output is connected (true when automatic).
    pub sound_chosen_present: bool,
    // the disk that holds the data
    pub disk_total: u64,
    pub disk_free: u64,
    // time
    pub route: bool,
    pub clock_synced: bool,
    /// Network time is switched on in Settings.
    pub network_time_on: bool,
    /// The clock is ahead of the network time and was left alone until the person agrees (seconds).
    pub clock_ahead_secs: Option<u64>,
    // exposure (the dev cluster's own scan writes the audit line; DOCTOR only reads it)
    /// The detail of the newest `devcluster.exposure.unexpected` audit line that no later exposure line cleared.
    pub exposure_unexpected: Option<String>,
    // model servers
    /// Enabled model servers that did not answer (their ids).
    pub models_unreachable: Vec<String>,
    // the startup registry
    pub failed_services: Vec<FailedService>,
    /// The browser graphics mode in words (`webgraphics::Status::line`; empty: unknown) and whether the engine fell back to software.
    pub graphics_line: String,
    pub graphics_fallback: bool,
    // memory (kilobytes read from /proc/meminfo, in bytes here; total 0: unknown)
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    /// Cameras the kernel lists (None: not looked at).
    pub cameras: Option<usize>,
    /// The connection test (slow; None: not run yet).
    pub net: Option<NetProbe>,
    /// Developer mode is on: does something answer on its port on this computer? (slow; None: not asked)
    pub dev_port_answers: Option<bool>,
    /// The firewall is switched on (None: not known).
    pub firewall_on: Option<bool>,
    /// A system update did not finish, in one sentence.
    pub update_note: Option<String>,
    /// The last session ended without a clean shutdown (the kept kernel log has the end of it); the date, or empty.
    pub crash_note: Option<String>,
    /// The state of the VPN that is switched on (None: none is).
    pub vpn: Option<crate::vpn_mgr::Light>,
}

/// What the connection test found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetProbe {
    /// A default route exists.
    pub route: bool,
    /// A well-known name resolved within the wait (only asked when there is a route).
    pub dns: bool,
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
/// Developer mode "about to switch off": this long before.
pub const DEV_SOON_SECS: i64 = 3600;
/// A browser engine fault counts as recent for this long.
pub const ENGINE_WINDOW_SECS: i64 = 24 * 3600;
/// After this long without a network time the clock is called unsynchronised (the cluster waits two minutes at most).
pub const CLOCK_GRACE_SECS: u64 = 300;
/// A cluster that ended on its own this long ago still counts.
const EXIT_RECENT_SECS: i64 = 24 * 3600;

/// `3 minutes`, `1 hour 5 minutes`.
pub fn span_words(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    match m {
        0 | 1 => "a minute".into(),
        2..=59 => format!("{m} minutes"),
        _ => {
            let (h, r) = (m / 60, m % 60);
            let hw = if h == 1 { "1 hour".to_string() } else { format!("{h} hours") };
            match r {
                0 => hw,
                1 => format!("{hw} 1 minute"),
                _ => format!("{hw} {r} minutes"),
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ the rules

/// The dev cluster stopped (or failed) when it should be running.
pub fn cluster_stopped(f: &Facts) -> Option<Finding> {
    if !f.cluster_installed {
        return None;
    }
    match f.cluster_state.as_str() {
        "failed" => Some(finding("cluster-stopped", Severity::Problem, "The dev cluster stopped with an error.", f.cluster_failed_reason.clone(), open("Open Dev cluster", "tab:devcluster"))),
        "stopped" if f.cluster_autostart => Some(finding("cluster-stopped", Severity::Problem, "The dev cluster is stopped, although it is set to start by itself.", f.cluster_failed_reason.clone(), open("Open Dev cluster", "tab:devcluster"))),
        "stopped" if f.cluster_last_exit_at > 0 && f.now - f.cluster_last_exit_at < EXIT_RECENT_SECS => Some(finding("cluster-stopped", Severity::Warning, "The dev cluster stopped on its own a short while ago and is not running.", f.cluster_failed_reason.clone(), open("Open Dev cluster", "tab:devcluster"))),
        _ => None,
    }
}

/// The cluster's fixed node address is wanted but no interface carries it.
pub fn node_ip_missing(f: &Facts) -> Option<Finding> {
    if !f.cluster_installed || !f.node_ip_wanted || f.node_ip_present {
        return None;
    }
    // while it is stopped the address is not needed yet; the stopped rule speaks
    if matches!(f.cluster_state.as_str(), "stopped" | "stopping") && !f.cluster_autostart {
        return None;
    }
    Some(finding("node-ip-missing", Severity::Problem, "The dev cluster's own network address is missing, so it cannot start properly.", format!("expected {}", crate::k3sdev::NODE_IP), open("Open Dev cluster", "tab:devcluster")))
}

/// Developer mode is on and the firewall lets nothing through to its port.
pub fn firewall_blocks_developer_port(f: &Facts) -> Option<Finding> {
    if !f.dev_on {
        return None;
    }
    let hint = f.dev_firewall_hint.as_ref()?;
    Some(finding("firewall-dev-port", Severity::Problem, "The firewall blocks the developer-mode port, so a connected computer cannot reach this one.", hint.clone(), open("Open Firewall", "tab:firewall")))
}

/// Developer mode switches itself off within the hour.
pub fn developer_mode_ending(f: &Facts) -> Option<Finding> {
    if !f.dev_on || f.dev_until <= 0 {
        return None;
    }
    let left = f.dev_until - f.now;
    if left > DEV_SOON_SECS {
        return None;
    }
    Some(finding("dev-mode-ending", Severity::Warning, format!("Developer mode switches off in {}.", span_words(left)), String::new(), Some(Fix { label: "Keep it on for 4 more hours", action: Action::ExtendDeveloperMode })))
}

/// The vault is locked and something waits for it.
pub fn vault_locked_with_waiting(f: &Facts) -> Option<Finding> {
    if !f.vault_locked || f.vault_waiting.is_empty() {
        return None;
    }
    let names = f.vault_waiting.join(", ");
    let text = if f.vault_waiting.len() == 1 { format!("Your vault is locked and a password is waiting to be saved in it ({names}).") } else { format!("Your vault is locked and {} passwords are waiting to be saved in it ({names}).", f.vault_waiting.len()) };
    Some(finding("vault-locked", Severity::Warning, text, String::new(), open("Open the vault to unlock it", "tab:vault")))
}

/// The browser engine draws in software because the last session stopped right after it started on the graphics card.
pub fn engine_graphics_fallback(f: &Facts) -> Option<Finding> {
    if !f.graphics_fallback {
        return None;
    }
    Some(finding("engine-graphics-fallback", Severity::Note, "The browser draws in software (slower) because the last session stopped right after it started on this graphics card.", format!("Browser graphics: {}", f.graphics_line), open("Open Browser settings", "tab:browser")))
}

/// The web engine crashed lately.
pub fn engine_crashed(f: &Facts) -> Option<Finding> {
    if f.engine.crashed == 0 {
        return None;
    }
    let n = f.engine.crashed;
    Some(finding("engine-crashed", Severity::Warning, if n == 1 { "The web engine stopped unexpectedly in the last day.".to_string() } else { format!("The web engine stopped unexpectedly {n} times in the last day.") }, f.engine.last.clone(), open("Open Browser settings", "tab:browser")))
}

/// A page could not use the camera.
pub fn engine_camera(f: &Facts) -> Option<Finding> {
    if f.engine.camera_trouble == 0 {
        return None;
    }
    Some(finding("engine-camera", Severity::Warning, "The web engine had trouble using the camera in the last day (another program may be holding it).", f.engine.last.clone(), open("Open Browser settings", "tab:browser")))
}

/// A page could not use the microphone.
pub fn engine_microphone(f: &Facts) -> Option<Finding> {
    if f.engine.mic_trouble == 0 {
        return None;
    }
    Some(finding("engine-microphone", Severity::Warning, "The web engine had trouble using the microphone in the last day.", f.engine.last.clone(), open("Open Sound", "tab:sound")))
}

/// No sound card, or the chosen output is not connected.
pub fn no_sound(f: &Facts) -> Option<Finding> {
    if f.sound_cards == 0 {
        // a card shows up late sometimes
        if f.uptime_secs < 60 {
            return None;
        }
        return Some(finding("sound-none", Severity::Warning, "No sound card was found.", String::new(), open("Open Sound", "tab:sound")));
    }
    if !f.sound_chosen.is_empty() && !f.sound_chosen_present {
        return Some(finding("sound-missing", Severity::Warning, "The sound output you chose is not connected, so nothing is heard.", f.sound_chosen.clone(), open("Choose a sound output", "tab:sound")));
    }
    None
}

/// The data disk is nearly full.
pub fn disk_nearly_full(f: &Facts) -> Option<Finding> {
    if f.disk_total == 0 {
        return None;
    }
    let pct_free = f.disk_free as f64 * 100.0 / f.disk_total as f64;
    let detail = format!("{} free of {}", iyakki_storage::human(f.disk_free), iyakki_storage::human(f.disk_total));
    if f.disk_free < 512 * MIB || pct_free < 5.0 {
        Some(finding("disk-full", Severity::Problem, "The disk is almost full.", detail, open("Open Storage", "tab:storage")))
    } else if f.disk_free < 2 * GIB || pct_free < 10.0 {
        Some(finding("disk-low", Severity::Warning, "The disk is getting full.", detail, open("Open Storage", "tab:storage")))
    } else {
        None
    }
}

/// The clock was never set from the network although the network is up, or it is ahead and waits for the person.
pub fn time_not_synchronised(f: &Facts) -> Option<Finding> {
    if let Some(ahead) = f.clock_ahead_secs {
        return Some(finding("time-ahead", Severity::Warning, "The clock is ahead of the network time and was left alone until you agree to put it back.", format!("{ahead} seconds ahead"), open("Open Time", "tab:time")));
    }
    if f.clock_synced || !f.route || f.uptime_secs < CLOCK_GRACE_SECS {
        return None;
    }
    if !f.network_time_on {
        return Some(finding("time-off", Severity::Note, "Network time is switched off, so the clock is not checked.", String::new(), open("Open Time", "tab:time")));
    }
    Some(finding("time-unsynced", Severity::Warning, "The clock has not been set from the network time.", String::new(), open("Open Time", "tab:time")))
}

/// The dev cluster's own scan found a service open to the network that should not be.
pub fn unexpected_exposure(f: &Facts) -> Option<Finding> {
    let d = f.exposure_unexpected.as_ref()?;
    Some(finding("exposure", Severity::Problem, "A service of the dev cluster is open to the local network and is not on the list of services that should be.", d.clone(), open("Open Dev cluster", "tab:devcluster")))
}

/// A model server that is switched on does not answer.
pub fn model_server_unreachable(f: &Facts) -> Option<Finding> {
    if f.models_unreachable.is_empty() {
        return None;
    }
    let n = f.models_unreachable.len();
    Some(finding("model-unreachable", Severity::Warning, if n == 1 { "A model server does not answer.".to_string() } else { format!("{n} model servers do not answer.") }, f.models_unreachable.join(", "), open("Open Models", "tab:models")))
}

/// No connection, or a connection where names do not resolve.
pub fn connection(f: &Facts) -> Option<Finding> {
    let n = f.net?;
    if !n.route {
        // a machine that never had a network is not "broken"
        return Some(finding("net-none", Severity::Warning, "This computer is not connected to a network.", String::new(), open("Open Network", "tab:network")));
    }
    if !n.dns {
        return Some(finding("net-dns", Severity::Warning, "The network is connected but names do not resolve, so websites and updates will not load.", String::new(), open("Open Network", "tab:network")));
    }
    None
}

/// Developer mode is on and nothing answers on its port.
pub fn developer_port_silent(f: &Facts) -> Option<Finding> {
    if !f.dev_on || f.dev_port_answers != Some(false) {
        return None;
    }
    Some(finding("dev-port-silent", Severity::Problem, "Developer mode is on but nothing answers on its port, so a connected computer cannot reach this one.", format!("port {}", f.dev_port), open("Open Developer mode", "tab:developer")))
}

/// Memory nearly used up, or swap nearly full.
pub fn memory_low(f: &Facts) -> Option<Finding> {
    if f.mem_total == 0 {
        return None;
    }
    let pct = f.mem_available as f64 * 100.0 / f.mem_total as f64;
    let detail = format!("{} available of {}", iyakki_storage::human(f.mem_available), iyakki_storage::human(f.mem_total));
    if pct < 5.0 {
        return Some(finding("memory-low", Severity::Problem, "The computer is almost out of memory; close an app or a browser tab.", detail, open("Open Overview", "tab:overview")));
    }
    if pct < 10.0 {
        return Some(finding("memory-tight", Severity::Warning, "Memory is getting tight; close what you are not using.", detail, open("Open Overview", "tab:overview")));
    }
    if f.swap_total > 0 && (f.swap_free as f64 * 100.0 / f.swap_total as f64) < 10.0 {
        return Some(finding("swap-full", Severity::Warning, "The swap space is nearly full, so the computer will slow down.", format!("{} free of {}", iyakki_storage::human(f.swap_free), iyakki_storage::human(f.swap_total)), open("Open Overview", "tab:overview")));
    }
    None
}

/// No camera is listed.
pub fn no_camera(f: &Facts) -> Option<Finding> {
    (f.cameras == Some(0)).then(|| finding("camera-none", Severity::Note, "No camera was found (fine if this computer has none).", String::new(), open("Open Browser settings", "tab:browser")))
}

/// The firewall is switched off.
pub fn firewall_off(f: &Facts) -> Option<Finding> {
    (f.firewall_on == Some(false)).then(|| finding("firewall-off", Severity::Warning, "The firewall is switched off, so every service on this computer is open to the network.", String::new(), open("Open Firewall", "tab:firewall")))
}

/// The vault is locked and nothing waits for it (the waiting case has its own, louder rule).
pub fn vault_locked_note(f: &Facts) -> Option<Finding> {
    (f.vault_locked && f.vault_waiting.is_empty()).then(|| finding("vault-locked-note", Severity::Note, "Your vault is locked, so saved passwords and keys are not available until you unlock it.", String::new(), open("Open the vault to unlock it", "tab:vault")))
}

/// A system update did not finish.
pub fn update_pending(f: &Facts) -> Option<Finding> {
    let t = f.update_note.as_ref()?;
    Some(finding("update-unfinished", Severity::Warning, "A system update did not finish.", t.clone(), open("Open System update", "tab:updates")))
}

/// The last session ended without a clean shutdown.
pub fn crashed_last_time(f: &Facts) -> Option<Finding> {
    let d = f.crash_note.as_ref()?;
    Some(finding("crash-marker", Severity::Warning, "The last session ended unexpectedly; its last kernel messages were kept.", d.clone(), open("Open the session log", "tab:sessionlog")))
}

/// The VPN that is switched on is not protecting the traffic.
pub fn vpn_not_connected(f: &Facts) -> Option<Finding> {
    use crate::vpn_mgr::Light;
    match f.vpn? {
        Light::Open => Some(finding("vpn-open", Severity::Warning, "A VPN is switched on but not connected, so traffic goes direct.", String::new(), open("Open VPN", "tab:vpn"))),
        Light::Blocked => Some(finding("vpn-blocked", Severity::Problem, "A VPN is switched on but down; the guard blocks the traffic that must not go direct.", String::new(), open("Open VPN", "tab:vpn"))),
        Light::Secure | Light::Working => None,
    }
}

/// Services of the startup registry that failed and have no rule of their own above.
pub fn failed_services(f: &Facts) -> Vec<Finding> {
    f.failed_services
        .iter()
        .filter(|s| !matches!(s.id.as_str(), "k3s" | "sound" | "clock"))
        .map(|s| {
            let fix = s.open.map(|item| Fix { label: "Open its page", action: Action::Settings(item) });
            finding("service-failed", Severity::Problem, format!("{} did not start.", s.name), s.reason.clone(), fix)
        })
        .collect()
}

/// Every rule, in the order they are listed.
pub fn diagnose(f: &Facts) -> Vec<Finding> {
    let single: [fn(&Facts) -> Option<Finding>; 23] = [
        cluster_stopped,
        node_ip_missing,
        unexpected_exposure,
        firewall_blocks_developer_port,
        developer_mode_ending,
        vault_locked_with_waiting,
        engine_crashed,
        engine_graphics_fallback,
        engine_camera,
        engine_microphone,
        no_sound,
        disk_nearly_full,
        time_not_synchronised,
        model_server_unreachable,
        connection,
        developer_port_silent,
        memory_low,
        no_camera,
        firewall_off,
        vault_locked_note,
        update_pending,
        crashed_last_time,
        vpn_not_connected,
    ];
    let mut v: Vec<Finding> = single.iter().filter_map(|r| r(f)).collect();
    v.extend(failed_services(f));
    // stable: inside a severity the rules keep their order
    v.sort_by_key(|x| x.severity);
    v
}

/// The text DOCTOR prints for `iyakki doctor` and puts in the bundle: one block per finding, or one calm line.
pub fn report_lines(found: &[Finding], with_detail: bool) -> Vec<String> {
    if found.is_empty() {
        return vec!["Everything checked is fine.".into()];
    }
    let mut out = Vec::new();
    for x in found {
        out.push(format!("[{}] {}", x.severity.word(), x.text));
        if with_detail && !x.detail.is_empty() {
            out.push(format!("    {}", x.detail));
        }
        if let Some(fix) = &x.fix {
            out.push(format!("    fix: {}", fix.label));
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ small readers the tile uses

/// (total, free) bytes of the volume holding `p`, for a normal user.
pub fn disk_space(p: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` is plain data that statvfs fills in.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        let fr = st.f_frsize as u64;
        Some(((st.f_blocks as u64).saturating_mul(fr), (st.f_bavail as u64).saturating_mul(fr)))
    }
}

/// The dev cluster's own exposure scan writes `devcluster.exposure.unexpected` to the audit log; the newest exposure line decides
/// (a later line of another kind, such as a clean scan, clears it). `recent` is newest first, in the audit log's display form
/// (`<time> <user> <event> <detail>`). Returns the detail of the unexpected line.
pub fn exposure_from_audit(recent: &[String]) -> Option<String> {
    for l in recent {
        let Some(i) = l.find(" devcluster.exposure.") else { continue };
        let rest = &l[i + 1..];
        let (event, detail) = rest.split_once(' ').unwrap_or((rest, ""));
        return (event == "devcluster.exposure.unexpected").then(|| detail.trim().to_string());
    }
    None
}

/// Memory figures from the text of `/proc/meminfo`: (total, available, swap total, swap free) in bytes.
pub fn parse_meminfo(t: &str) -> (u64, u64, u64, u64) {
    let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(k)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()).unwrap_or(0) * 1024;
    (get("MemTotal:"), get("MemAvailable:"), get("SwapTotal:"), get("SwapFree:"))
}

/// Does `name` resolve within `wait`? The lookup runs on a thread that is left behind if it hangs.
pub fn resolves(name: &str, wait: std::time::Duration) -> bool {
    use std::net::ToSocketAddrs;
    let (tx, rx) = std::sync::mpsc::channel();
    let n = format!("{name}:443");
    std::thread::spawn(move || {
        let _ = tx.send(n.to_socket_addrs().map(|mut a| a.next().is_some()).unwrap_or(false));
    });
    rx.recv_timeout(wait).unwrap_or(false)
}

/// `host:port` of a model server's address when it is on this computer or the local network (a cloud service is never probed).
pub fn local_endpoint(base_url: &str) -> Option<String> {
    let (scheme, rest) = base_url.split_once("://").unwrap_or(("http", base_url));
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?;
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h.trim_matches(['[', ']']), p.parse::<u16>().ok()?),
        _ => (auth.trim_matches(['[', ']']), if scheme.eq_ignore_ascii_case("https") { 443 } else { 80 }),
    };
    let local = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => crate::modelhost::is_local_ip(ip),
        Err(_) => crate::modelhost::is_local_name(host),
    };
    local.then(|| if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") })
}

/// Does something answer at `host:port` within `wait`? (A connection only; nothing is sent.)
pub fn answers(hostport: &str, wait: std::time::Duration) -> bool {
    use std::net::ToSocketAddrs;
    hostport.to_socket_addrs().map(|mut a| a.any(|sa| std::net::TcpStream::connect_timeout(&sa, wait).is_ok())).unwrap_or(false)
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;

/// Seconds since the machine started (a huge value when unknown, so "just booted" is never claimed by mistake).
pub fn uptime_secs() -> u64 {
    crate::sysinfo::uptime_f64(crate::sysinfo::proc_root()).map_or(u64::MAX / 2, |v| v as u64)
}

/// How many sound cards the kernel lists.
pub fn sound_card_count() -> usize {
    std::fs::read_to_string("/proc/asound/cards").map(|t| t.lines().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).count()).unwrap_or(0)
}

/// How many camera devices the kernel lists (`/dev/video*`; one camera can list two).
pub fn camera_count() -> usize {
    std::fs::read_dir("/sys/class/video4linux").map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("video")).count()).unwrap_or(0)
}

/// The sound outputs the kernel lists now.
pub fn sound_outputs() -> Vec<crate::audioout::Output> {
    crate::audioout::scan(std::path::Path::new("/proc"), &|_| None)
}

/// Whether the machine has a default network route.
pub fn has_default_route() -> bool {
    crate::k3sdev::has_default_route(&std::fs::read_to_string(crate::k3sdev::ROUTE_FILE).unwrap_or_default())
}

/// The boot log, empty when there is none.
pub fn boot_log() -> String {
    std::fs::read_to_string("/var/log/iyakki-boot.log").unwrap_or_default()
}

/// Whether one of the machine's network interfaces holds this IPv4 address.
pub fn has_address(ip: &str) -> bool {
    iyakki_devctl::ipv4_addresses().iter().any(|(_, a)| a == ip)
}

/// What this computer is, as text, for the diagnostics bundle.
pub fn hardware_text() -> String {
    iyakki_hwinfo::collect(&iyakki_hwinfo::Roots::real()).to_text()
}

/// The text of the kernel's memory table (a function so tiles ask core instead of naming a kernel path themselves); empty when unreadable.
pub fn read_meminfo() -> String {
    std::fs::read_to_string("/proc/meminfo").unwrap_or_default()
}

/// The date of an unseen unclean shutdown recorded under `dir` (the user data directory), if any.
pub fn unseen_unclean_day(dir: &std::path::Path) -> Option<String> {
    iyakki_storage::crashlog::read_unclean(&dir.join(iyakki_storage::crashlog::LOG_SUBDIR))
        .filter(|u| !u.seen)
        .map(|u| iyakki_storage::crashlog::wall(u.started).split(' ').next().unwrap_or("").to_string())
}

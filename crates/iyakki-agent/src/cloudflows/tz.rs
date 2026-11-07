//! The tenant's time zone for cron triggers (tenant configuration `schedule.timezone`, default UTC).
//!
//! A trigger names its own zone (`tz` on the cron trigger, default `local`). In the cloud `local` means the tenant's setting, never the
//! pod's clock. No time zone database is in the repository: a zone name is read from the system's zone files (`/usr/share/zoneinfo`,
//! the same reader the desktop uses) when the image has them; the shipped `FROM scratch` container does not, so there only `UTC`, `Z`
//! and fixed offsets (`+05:30`, `-0800`) work and any other name falls back to UTC with a visible warning in the status. No rule table
//! was written by hand (daylight saving changes are the system files' job).

use iyakki_flow::sched::{basic_tz, FixedTz, Tz, TzResolver};
use std::path::PathBuf;
use std::sync::Arc;

pub const ZONEINFO: &str = "/usr/share/zoneinfo";

struct Zoned(crate::clock::Zone);
impl Tz for Zoned {
    fn offset_secs(&self, unix: i64) -> i32 {
        self.0.local(unix).offset
    }
}

fn zone_file(root: &Option<PathBuf>, name: &str) -> Option<Arc<dyn Tz>> {
    let root = root.as_ref()?;
    if name.is_empty() || name.contains("..") || name.starts_with('/') || !name.chars().all(|c| c.is_ascii_alphanumeric() || "/_-+".contains(c)) {
        return None;
    }
    let b = std::fs::read(root.join(name)).ok()?;
    crate::clock::Zone::parse(&b).map(|z| Arc::new(Zoned(z)) as Arc<dyn Tz>)
}

/// How a setting is understood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Utc,
    Fixed(i32),
    Zone(String),
    /// The name could not be resolved here; UTC is used.
    FellBackToUtc(String),
}

pub fn mode_of(setting: &str, zoneinfo: &Option<PathBuf>) -> Mode {
    let s = setting.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("utc") || s.eq_ignore_ascii_case("z") || s.eq_ignore_ascii_case("local") {
        return Mode::Utc;
    }
    if let Some(t) = basic_tz(s) {
        return Mode::Fixed(t.offset_secs(0));
    }
    if zone_file(zoneinfo, s).is_some() {
        return Mode::Zone(s.to_string());
    }
    Mode::FellBackToUtc(s.to_string())
}

/// The resolver the scheduler uses: `local` and the empty name mean the tenant's setting.
pub fn tenant_resolver(setting: &str, zoneinfo: Option<PathBuf>) -> TzResolver {
    let setting = setting.trim().to_string();
    Arc::new(move |name: &str| {
        let n = name.trim();
        let n = if n.is_empty() || n.eq_ignore_ascii_case("local") { setting.as_str() } else { n };
        if let Some(t) = basic_tz(n) {
            return Some(t);
        }
        Some(zone_file(&zoneinfo, n).unwrap_or_else(|| Arc::new(FixedTz(0))))
    })
}

/// Is this a value `schedule.timezone` may take? (Offsets and UTC always; a name only when the zone files know it.)
pub fn setting_ok(setting: &str, zoneinfo: &Option<PathBuf>) -> Result<(), String> {
    match mode_of(setting, zoneinfo) {
        Mode::FellBackToUtc(n) => Err(format!("`{n}` is not a zone this image knows (use UTC or an offset like +05:30)")),
        _ => Ok(()),
    }
}

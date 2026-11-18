//! Which flows may tell the person through an outside channel (Matrix) and how often.
//!
//! * A flow that came from outside (an import, a store template) starts with the switch OFF: its INBOX notes still work, but no
//!   message leaves the computer until the person switches it on for that flow.
//! * Whatever the switch says, one flow sends at most one outside message per [`COOLDOWN_SECS`]; the next ones in that time are
//!   refused with a sentence (the INBOX note is always made, and a dry run costs nothing).
//! * The file is `<flows data>/external-channels.json`. A damaged file, or one written by a newer version, is moved aside (never
//!   overwritten) and every flow is switched off until the person turns each one on again: failing closed.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One outside message per flow per this many seconds.
pub const COOLDOWN_SECS: i64 = 300;
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct File {
    version: u32,
    /// Flows without an entry are on (`true`) or off (`false`).
    default_on: bool,
    #[serde(default)]
    off: BTreeSet<String>,
    #[serde(default)]
    on: BTreeSet<String>,
}

impl File {
    fn open() -> File {
        File { version: VERSION, default_on: true, off: BTreeSet::new(), on: BTreeSet::new() }
    }
    fn closed() -> File {
        File { default_on: false, ..File::open() }
    }
    fn allows(&self, flow: &str) -> bool {
        if self.on.contains(flow) {
            return true;
        }
        self.default_on && !self.off.contains(flow)
    }
}

pub struct ChannelSwitch {
    path: PathBuf,
    last: Mutex<HashMap<String, (i64, String)>>,
}

impl ChannelSwitch {
    pub fn new(dir: &Path) -> ChannelSwitch {
        ChannelSwitch { path: dir.join("external-channels.json"), last: Mutex::new(HashMap::new()) }
    }

    fn load(&self) -> File {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(_) => return File::open(),
        };
        match serde_json::from_slice::<File>(&bytes) {
            Ok(f) if f.version <= VERSION => f,
            _ => {
                if let Some(to) = iyakki_schema::move_aside(&self.path, iyakki_schema::now_secs()) {
                    iyakki_schema::report(format!("The flow channel switches could not be read; moved to {}. Every flow's outside messages are off until you switch them on.", to.display()));
                }
                let f = File::closed();
                let _ = self.save(&f);
                f
            }
        }
    }

    fn save(&self, f: &File) -> Result<(), String> {
        iyakki_schema::write_atomic(&self.path, &serde_json::to_vec(f).map_err(|e| e.to_string())?, 0o600)
    }

    /// Is this flow allowed to send outside messages?
    pub fn allowed(&self, flow: &str) -> bool {
        self.load().allows(flow)
    }

    /// A flow that came from outside starts off.
    pub fn block_new(&self, flow: &str) {
        let mut f = self.load();
        f.on.remove(flow);
        f.off.insert(flow.to_string());
        let _ = self.save(&f);
    }

    /// The person's choice for one flow.
    pub fn set(&self, flow: &str, on: bool) -> Result<(), String> {
        let mut f = self.load();
        if on {
            f.off.remove(flow);
            f.on.insert(flow.to_string());
        } else {
            f.on.remove(flow);
            f.off.insert(flow.to_string());
        }
        self.save(&f)
    }

    /// May this flow send an outside message now? The switch first, then the cool-down. A retry of the message just sent (same `key`) is
    /// allowed: the homeserver treats it as the same event. Nothing is recorded here; call [`ChannelSwitch::sent`] after a real send.
    pub fn permit(&self, flow: &str, key: &str, now: i64) -> Result<(), String> {
        if !self.allowed(flow) {
            return Err(format!("the flow `{flow}` is not allowed to send outside messages (its channel switch is off; switch it on for this flow if you trust it)"));
        }
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, k)) = last.get(flow) {
            if k != key && now - *t < COOLDOWN_SECS {
                return Err(format!("the flow `{flow}` sent an outside message {} s ago; one every {} minutes is the limit (the INBOX note was made)", now - *t, COOLDOWN_SECS / 60));
            }
        }
        Ok(())
    }

    /// A message went out: start the cool-down.
    pub fn sent(&self, flow: &str, key: &str, now: i64) {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).insert(flow.to_string(), (now, key.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-chsw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_new_flow_may_send_an_imported_one_may_not_until_switched_on_and_it_is_kept() {
        let d = dir("switch");
        let s = ChannelSwitch::new(&d);
        assert!(s.allowed("mine"));
        s.block_new("imp");
        assert!(!s.allowed("imp") && s.allowed("mine"));
        assert!(s.permit("imp", "k", 100).unwrap_err().contains("not allowed"));
        s.set("imp", true).unwrap();
        assert!(s.permit("imp", "k", 100).is_ok());
        assert!(ChannelSwitch::new(&d).allowed("imp"), "kept across a restart");
        s.set("imp", false).unwrap();
        assert!(!ChannelSwitch::new(&d).allowed("imp"));
    }

    #[test]
    fn one_outside_message_per_flow_per_cool_down_a_retry_of_the_same_message_and_a_dry_run_cost_nothing() {
        let s = ChannelSwitch::new(&dir("cool"));
        assert!(s.permit("f", "k1", 1000).is_ok(), "nothing is recorded by asking (a dry run only asks)");
        assert!(s.permit("f", "k2", 1001).is_ok());
        s.sent("f", "k1", 1002);
        assert!(s.permit("f", "k1", 1003).is_ok(), "the same message again is the same event");
        let e = s.permit("f", "k2", 1002 + COOLDOWN_SECS - 1).unwrap_err();
        assert!(e.contains("one every 5 minutes"), "{e}");
        assert!(s.permit("g", "k2", 1003).is_ok(), "another flow has its own");
        assert!(s.permit("f", "k2", 1002 + COOLDOWN_SECS).is_ok());
    }

    #[test]
    fn a_damaged_or_newer_file_is_moved_aside_and_every_flow_is_off_until_switched_on() {
        for (tag, bytes) in [("bad", b"{not json".to_vec()), ("newer", br#"{"version":99,"default_on":true}"#.to_vec())] {
            let d = dir(tag);
            std::fs::write(d.join("external-channels.json"), &bytes).unwrap();
            let s = ChannelSwitch::new(&d);
            assert!(!s.allowed("any"), "{tag}: fails closed");
            let aside: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("corrupt-")).collect();
            assert_eq!(aside.len(), 1, "{tag}: {aside:?}");
            assert_eq!(std::fs::read(d.join(&aside[0])).unwrap(), bytes, "{tag}: the old bytes are kept");
            s.set("any", true).unwrap();
            assert!(ChannelSwitch::new(&d).allowed("any") && !ChannelSwitch::new(&d).allowed("other"));
        }
    }
}

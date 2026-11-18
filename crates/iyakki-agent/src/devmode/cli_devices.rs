//! `iyakki dev devices` on the developer's computer: the list with last-seen address and time and the marks for duplicate and stale
//! entries, `--default`, `--forget`, `--rename OLD NEW` and `--prune [--yes]`. The same functions feed the chat tool of the paired
//! devices (`devtools`), so the chat and the command line say the same thing.

use super::client::{self, Device, Home};

/// An entry nobody used for this long may be stale (a device that is merely switched off, or developer mode on it timed out, looks
/// the same from here, so `--prune` never removes an entry used more recently than this).
pub const GRACE_SECS: i64 = 7 * 24 * 3600;

pub use iyakki_shared::devmode_types::pairings::ago;

/// One line per paired device, plus the marks.
pub fn list_lines(home: &Home, now: i64) -> Vec<String> {
    let list = home.devices();
    let def = home.default_name();
    let dups = home.duplicates();
    let mut out = Vec::new();
    for d in &list {
        let mut marks = Vec::new();
        if let Some((_, keep)) = dups.iter().find(|(drop, _)| drop.name == d.name) {
            marks.push(format!("DUPLICATE of {} (same device and account; `--prune` merges them)", keep.name));
        }
        let age = now - d.last_used();
        if d.last_used() > 0 && age > GRACE_SECS {
            marks.push(format!("STALE? not used for {} days (`--prune` checks whether it answers)", age / 86400));
        }
        out.push(format!(
            "{} {:<16} {}{}:{}  as {}  key {}  last seen {} at {}{}",
            if d.name == def { "*" } else { " " },
            d.name,
            d.host,
            if d.addr.is_empty() { String::new() } else { format!(" ({})", d.addr) },
            d.port,
            d.account,
            d.fingerprint.chars().take(23).collect::<String>(),
            ago(now, d.seen_at.max(d.paired_at)),
            d.last_address(),
            if marks.is_empty() { String::new() } else { format!("  [{}]", marks.join("; ")) }
        ));
    }
    out
}

/// What `--prune` would remove.
#[derive(Debug, Default)]
pub struct PrunePlan {
    /// `(dropped, kept)`: the same pairing twice.
    pub duplicates: Vec<(Device, Device)>,
    /// Not used for a week, and neither the address nor the name nor the key announcement answers.
    pub stale: Vec<Device>,
}

impl PrunePlan {
    pub fn names(&self) -> Vec<String> {
        self.duplicates.iter().map(|(d, _)| d.name.clone()).chain(self.stale.iter().map(|d| d.name.clone())).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.duplicates.is_empty() && self.stale.is_empty()
    }

    pub fn lines(&self, now: i64) -> Vec<String> {
        let mut v: Vec<String> = self.duplicates.iter().map(|(d, k)| format!("  {}  duplicate of {} (kept: used {})", d.name, k.name, ago(now, k.last_used()))).collect();
        v.extend(self.stale.iter().map(|d| format!("  {}  does not answer at {} or by name or key; last seen {}", d.name, d.last_address(), ago(now, d.last_used()))));
        v
    }
}

/// What a prune would remove. `alive` says whether a device answers (by address, by name or by its announced key); it is asked only for
/// entries that are not duplicates and have not been used within the grace time.
pub fn prune_plan(home: &Home, now: i64, alive: &dyn Fn(&Device) -> bool) -> PrunePlan {
    let duplicates = home.duplicates();
    let stale = home.devices().into_iter().filter(|d| !duplicates.iter().any(|(x, _)| x.name == d.name)).filter(|d| now - d.last_used() > GRACE_SECS).filter(|d| !alive(d)).collect();
    PrunePlan { duplicates, stale }
}

/// Remove what the plan lists.
pub fn apply(home: &Home, plan: &PrunePlan) -> Result<usize, String> {
    let names = plan.names();
    // a default that is dropped as a duplicate moves to the entry that stays
    let def = home.default_name();
    if let Some((_, keep)) = plan.duplicates.iter().find(|(d, _)| d.name == def) {
        home.set_default(&keep.name)?;
    }
    home.remove_many(&names)?;
    Ok(names.len())
}

/// `--prune`: print the plan, ask unless `yes`, remove. `confirm` shows the question and returns the answer.
pub fn prune(home: &Home, yes: bool, now: i64, alive: &dyn Fn(&Device) -> bool, confirm: &mut dyn FnMut(&str) -> bool, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let plan = prune_plan(home, now, alive);
    if plan.is_empty() {
        out("Nothing to remove: no duplicates, and every entry used in the last week or still answering stays.");
        return Ok(());
    }
    out(&format!("This would remove {} entr{} from this computer (the devices keep their side: remove this computer there too, Settings, Developer mode):", plan.names().len(), if plan.names().len() == 1 { "y" } else { "ies" }));
    for l in plan.lines(now) {
        out(&l);
    }
    if !yes && !confirm("Remove them? [y/N] ") {
        out("Nothing was removed.");
        return Ok(());
    }
    let n = apply(home, &plan)?;
    out(&format!("Removed {n}."));
    Ok(())
}

/// Does the device answer now? By its address, else its name, else the key it announces; the pinned certificate decides every time
/// (a different certificate at the address does not count). A device that moved gets its new address saved.
pub fn answers(home: &Home, d: &Device) -> bool {
    let key = match home.key() {
        Ok(k) => k,
        Err(_) => return false,
    };
    let c = client::DevClient::with_key(d.clone(), key);
    let probe = |host: &str| client::https(host, d.port, Some(&d.fingerprint), "GET", "/v1/dev/hello", &[], b"", std::time::Duration::from_secs(5)).map(|_| ());
    client::reach_by_key(&c.device, Some(home), &client::resolve_local, &client::find_by_key, &mut |h| probe(h), &mut |_| {}).is_ok()
}

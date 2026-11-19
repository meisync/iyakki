//! `iyakki.money` (SDK 1.2) and the per-app SDK state file it shares with the first-use answers of the other new namespaces.
//!
//! The file is `appdata/tile-sdk/<person>/<app>.sdk2.json`. It is read again for every call that needs it and written under one process-wide lock,
//! so the person's own changes (Info panel, the chat tool [`set_limit`]) and the app's spends never overwrite each other. The rules are in
//! `iyakki_core_api::sdkbridge::money`: whole minor units of one currency, a daily limit that is **zero until the person raises it**, checked before every
//! spend and again when the person answers the card. A file this release cannot read or write safely (damaged, or from a newer IYAKKI) is read-only:
//! the limit it holds still applies but nothing can be spent, because the spent amount could not be kept. **No payment provider is called here.**

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_core_api::sdkbridge::money::{self, Limit, Refusal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sdkv2::{plain_line, unknown, with_ignored, Sdk2, ThenFn};
use super::tile::Caps;
use super::tilesdk::{code, SdkError};
use crate::clock::Zone;
use crate::usage::UsageLog;

/// The version of the SDK 1.2 state file. Readers ignore fields they do not know; a newer file is read-only.
pub const FILE_VERSION: u32 = 1;
const KEEP_DAYS: usize = 60;
/// Payment refs are kept for the last days they were used and at most this many, so an old `ref` stays a repeat.
const KEEP_REFS: usize = 5000;
const KEEP_REF_DAYS: usize = 7;
/// The receipt of a `ref` whose charge is running: the same `ref` is a repeat until the call ends.
const PENDING: &str = "pending";

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoneyFile {
    pub currency: String,
    pub per_day_minor: u64,
    /// Every spend shows a card (the default); the person may switch it off for amounts inside the limit.
    pub confirm_each: bool,
    /// Spent by local day (`2026-11-12`), minor units.
    pub spent: BTreeMap<String, u64>,
    /// `ref` of a spend -> its receipt, so a repeated call is not charged twice.
    pub refs: BTreeMap<String, String>,
    /// `ref` -> the local day it was used (to forget the oldest first).
    pub ref_days: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl MoneyFile {
    /// Forget the refs of days beyond the last [`KEEP_REF_DAYS`], then the oldest ones beyond [`KEEP_REFS`].
    fn trim_refs(&mut self) {
        let day_of = |m: &MoneyFile, k: &str| m.ref_days.get(k).cloned().unwrap_or_default();
        loop {
            let days: std::collections::BTreeSet<String> = self.refs.keys().map(|k| day_of(self, k)).collect();
            if days.len() <= KEEP_REF_DAYS {
                break;
            }
            let oldest = days.into_iter().next().unwrap_or_default();
            let drop: Vec<String> = self.refs.keys().filter(|k| day_of(self, k) == oldest).cloned().collect();
            for k in drop {
                self.refs.remove(&k);
            }
        }
        while self.refs.len() > KEEP_REFS {
            let Some(k) = self.refs.keys().min_by_key(|k| (day_of(self, k), (*k).clone())).cloned() else { break };
            self.refs.remove(&k);
        }
        let live: std::collections::BTreeSet<String> = self.refs.keys().cloned().collect();
        self.ref_days.retain(|k, _| live.contains(k));
    }
}

impl Default for MoneyFile {
    fn default() -> MoneyFile {
        MoneyFile { currency: String::new(), per_day_minor: 0, confirm_each: true, spent: BTreeMap::new(), refs: BTreeMap::new(), ref_days: BTreeMap::new(), extra: BTreeMap::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppFile {
    pub v: u32,
    pub first_use: Vec<String>,
    pub money: MoneyFile,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
    /// Not written. The file could not be read as this release writes it: nothing is saved over it.
    #[serde(skip)]
    pub read_only: bool,
}

impl Default for AppFile {
    fn default() -> AppFile {
        AppFile { v: FILE_VERSION, first_use: vec![], money: MoneyFile::default(), extra: BTreeMap::new(), read_only: false }
    }
}

impl AppFile {
    fn load(p: &Path) -> AppFile {
        match std::fs::read_to_string(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AppFile::default(),
            Err(_) => AppFile { read_only: true, ..AppFile::default() },
            Ok(t) => match serde_json::from_str::<AppFile>(&t) {
                Ok(mut f) => {
                    if f.v > FILE_VERSION {
                        f.read_only = true;
                    }
                    f
                }
                Err(_) => AppFile { read_only: true, ..AppFile::default() },
            },
        }
    }

    fn save(&self, p: &Path) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }

    /// Read the file at `p` (a missing one is the default), run `f`, and save when `write` and the file is writable.
    pub fn edit<R>(p: &Path, write: bool, f: impl FnOnce(&mut AppFile) -> R) -> R {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = AppFile::load(p);
        let r = f(&mut file);
        if write && !file.read_only {
            file.v = file.v.max(FILE_VERSION);
            file.money.spent = trim_days(std::mem::take(&mut file.money.spent));
            file.money.trim_refs();
            file.save(p);
        }
        r
    }

    fn limit(&self) -> Option<Limit> {
        (!self.money.currency.is_empty()).then(|| Limit { currency: self.money.currency.clone(), per_day_minor: self.money.per_day_minor })
    }
}

fn trim_days(mut m: BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    while m.len() > KEEP_DAYS {
        let k = m.keys().next().cloned().unwrap_or_default();
        m.remove(&k);
    }
    m
}

// ------------------------------------------------------------------------------------------------ what the person sees and sets

/// Where an app's state file is.
pub fn state_path(dir: &Path, user: &str, app: &str) -> PathBuf {
    dir.join("appdata").join("tile-sdk").join(user).join(format!("{app}.sdk2.json"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoneyStatus {
    pub currency: String,
    pub per_day_minor: u64,
    pub spent_today_minor: u64,
    pub confirm_each: bool,
    pub read_only: bool,
}

impl MoneyStatus {
    pub fn words(&self) -> String {
        if self.currency.is_empty() || self.per_day_minor == 0 {
            return "may not spend money (the daily limit is nothing)".into();
        }
        format!(
            "may spend up to {} a day (spent today: {}); {}{}",
            money::format_minor(self.per_day_minor, &self.currency),
            money::format_minor(self.spent_today_minor, &self.currency),
            if self.confirm_each { "every payment asks you first" } else { "payments inside the limit do not ask" },
            if self.read_only { "; the limits file is read-only (written by a newer IYAKKI or damaged)" } else { "" }
        )
    }
}

fn day_key(zone: &Zone, unix: i64) -> String {
    crate::clock::format(zone, unix, "%Y-%m-%d")
}

/// The money limit and what was spent today, for the Info panel and the chat tool.
pub fn status(dir: &Path, user: &str, app: &str, zone: &Zone, unix: i64) -> MoneyStatus {
    AppFile::edit(&state_path(dir, user, app), false, |f| MoneyStatus { currency: f.money.currency.clone(), per_day_minor: f.money.per_day_minor, spent_today_minor: f.money.spent.get(&day_key(zone, unix)).copied().unwrap_or(0), confirm_each: f.money.confirm_each, read_only: f.read_only })
}

/// The person sets an app's limit (only a person-facing path calls this: the Info panel and the chat tool, which asks first). `per_day_minor` 0 takes the
/// permission away. `confirm_each` `None` leaves it as it is.
pub fn set_limit(dir: &Path, user: &str, app: &str, currency: &str, per_day_minor: u64, confirm_each: Option<bool>) -> Result<(), String> {
    if !money::valid_currency(currency) {
        return Err("the currency is three capital letters, like USD".into());
    }
    if per_day_minor > money::MAX_MINOR {
        return Err("that limit is too large".into());
    }
    AppFile::edit(&state_path(dir, user, app), true, |f| {
        if f.read_only {
            return Err("this app's limits file is read-only (written by a newer IYAKKI or damaged); nothing was changed".to_string());
        }
        f.money.currency = currency.into();
        f.money.per_day_minor = per_day_minor;
        if let Some(c) = confirm_each {
            f.money.confirm_each = c;
        }
        Ok(())
    })
}

// ------------------------------------------------------------------------------------------------ the calls

fn stop(r: &Refusal, resets: i64, limit: u64, remaining: u64, cur: &str) -> SdkError {
    match r {
        Refusal::BadAmount => SdkError::bad("amountMinor", &r.message()),
        _ => SdkError::new(code::BUDGET_STOP, r.message()).with("resets_at", resets).with("limit", "money per day").with("limitMinor", limit).with("remaining", remaining).with("currency", cur),
    }
}

impl Sdk2 {
    fn declared(caps: &Caps) -> String {
        caps.v2.get("money").and_then(Value::as_str).unwrap_or("").to_string()
    }

    pub(super) fn money_day(&self) -> (String, i64) {
        let now = self.unix();
        (day_key(self.zone(), now), UsageLog::today_span(now, self.zone()).1)
    }

    pub(super) fn money(&self, op: &str, a: &Value, caps: &Caps) -> Result<Value, SdkError> {
        let cur = Self::declared(caps);
        match op {
            "money.limits" => {
                let ignored = unknown(a, &[]);
                let (day, resets) = self.money_day();
                let (per_day, spent, confirm) = self.with_file(false, |f| (if f.money.currency == cur { f.money.per_day_minor } else { 0 }, f.money.spent.get(&day).copied().unwrap_or(0), f.money.confirm_each));
                Ok(with_ignored(json!({"currency": cur, "perDayMinor": per_day, "spentTodayMinor": spent, "remainingMinor": per_day.saturating_sub(spent), "confirmEach": confirm, "resetsAt": resets}), ignored))
            }
            "money.spend" => self.money_spend(a, &cur),
            _ => self.money_raise(a, &cur),
        }
    }

    fn money_spend(&self, a: &Value, declared: &str) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["amountMinor", "currency", "purpose", "ref"]);
        let amount = a.get("amountMinor").and_then(Value::as_u64).filter(|n| *n >= 1 && *n <= money::MAX_MINOR).ok_or_else(|| SdkError::bad("amountMinor", &Refusal::BadAmount.message()))?;
        let currency = a.get("currency").and_then(Value::as_str).filter(|c| money::valid_currency(c)).ok_or_else(|| SdkError::bad("currency", "three capital letters, like USD"))?.to_string();
        if currency != declared {
            return Err(SdkError::bad("currency", &format!("this app declared {declared}")));
        }
        let purpose = a.get("purpose").and_then(Value::as_str).map(|p| plain_line(p, 120)).filter(|p| !p.trim().is_empty()).ok_or_else(|| SdkError::bad("purpose", "say in a few words what the payment is for"))?;
        let reff = match a.get("ref") {
            None | Some(Value::Null) => None,
            Some(Value::String(r)) if !r.is_empty() && r.len() <= 64 && r.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c)) => Some(r.clone()),
            Some(_) => return Err(SdkError::bad("ref", "letters, digits and . _ : -")),
        };
        let (day, resets) = self.money_day();
        // a repeated ref gets its receipt back; nothing is charged twice
        if let Some(r) = &reff {
            if let Some(rec) = self.with_file(false, |f| f.money.refs.get(r).cloned()) {
                return Ok(with_ignored(json!({"receipt": rec, "repeated": true}), ignored));
            }
        }
        // checked before the person is asked and before anything else happens
        let (lim, spent, confirm, read_only) = self.with_file(false, |f| (f.limit(), f.money.spent.get(&day).copied().unwrap_or(0), f.money.confirm_each, f.read_only));
        if let Err(r) = money::check_spend(lim.as_ref(), spent, &currency, amount) {
            self.audit("app.money.stop", &format!("{} was refused {} ({:?})", self.app(), money::format_minor(amount, &currency), r));
            let (l, rem) = lim.as_ref().map_or((0, 0), |l| (l.per_day_minor, l.per_day_minor.saturating_sub(spent)));
            return Err(stop(&r, resets, l, rem, &currency));
        }
        if read_only {
            return Err(SdkError::new(code::UNAVAILABLE, "this app's limits file is read-only, so a payment could not be counted").with("fix", "Update IYAKKI, or ask the administrator to look at the app's limits in Settings -> Apps access."));
        }
        // nobody is asked to approve a payment that cannot be made
        self.ba("money.available", &self.be.payments, &json!({}))?;
        let app = self.app().to_string();
        let run: ThenFn = Box::new(move |s: &Sdk2| s.spend_now(amount, &currency, &purpose, reff, ignored));
        if !confirm {
            return run(self);
        }
        let (lim, spent) = (lim.unwrap_or(Limit { currency: String::new(), per_day_minor: 0 }), spent);
        let cur2 = lim.currency.clone();
        let pre = self.money_card_text(&app, amount, &cur2, spent, lim.per_day_minor);
        self.ask("money.spend", None, "An app wants to spend money", pre, run)
    }

    fn money_card_text(&self, app: &str, amount: u64, cur: &str, spent: u64, limit: u64) -> Vec<String> {
        vec![
            format!("“{app}” would like to make a payment of {}.", money::format_minor(amount, cur)),
            format!("Its daily limit is {}; spent today: {}.", money::format_minor(limit, cur), money::format_minor(spent, cur)),
            "Only you can raise the limit. No payment goes out unless you allow it here.".into(),
        ]
    }

    /// Reserve the amount under the lock (checked again against the file as it is now), call the provider, and keep or give back the reservation.
    fn spend_now(&self, amount: u64, currency: &str, purpose: &str, reff: Option<String>, ignored: Vec<String>) -> Result<Value, SdkError> {
        let (day, resets) = self.money_day();
        let reserved = self.with_file(true, |f| {
            if f.read_only {
                return Err(SdkError::new(code::UNAVAILABLE, "this app's limits file is read-only, so a payment could not be counted"));
            }
            // a `ref` that is taken (paid, or being paid by a call that is running at this moment) is never charged a second time
            if let Some(r) = reff.as_deref() {
                if let Some(rec) = f.money.refs.get(r) {
                    return Ok(Err(rec.clone()));
                }
            }
            let spent = f.money.spent.get(&day).copied().unwrap_or(0);
            match money::check_spend(f.limit().as_ref(), spent, currency, amount) {
                Ok(left) => {
                    *f.money.spent.entry(day.clone()).or_insert(0) += amount;
                    if let Some(r) = reff.as_deref() {
                        f.money.refs.insert(r.to_string(), PENDING.to_string());
                        f.money.ref_days.insert(r.to_string(), day.clone());
                    }
                    Ok(Ok(left))
                }
                Err(r) => Err(stop(&r, resets, f.money.per_day_minor, f.money.per_day_minor.saturating_sub(spent), currency)),
            }
        });
        let left = match reserved {
            Ok(Ok(l)) => l,
            Ok(Err(rec)) => return Ok(with_ignored(json!({"receipt": if rec == PENDING { String::new() } else { rec }, "repeated": true}), ignored)),
            Err(e) => {
                self.audit("app.money.stop", &format!("{} was refused {} when the person answered", self.app(), money::format_minor(amount, currency)));
                return Err(e);
            }
        };
        let r = self.ba("money.charge", &self.be.payments, &json!({"amountMinor": amount, "currency": currency, "purpose": purpose, "ref": reff}));
        match r {
            Ok(v) => {
                let receipt = v.get("receipt").and_then(Value::as_str).unwrap_or("").to_string();
                if let Some(r) = &reff {
                    self.with_file(true, |f| {
                        f.money.refs.insert(r.clone(), receipt.clone());
                    });
                }
                self.audit("app.money.spend", &format!("{} paid {} for: {}", self.app(), money::format_minor(amount, currency), purpose));
                Ok(with_ignored(json!({"receipt": receipt, "remainingMinor": left}), ignored))
            }
            Err(e) => {
                // nothing was charged: the reservation is given back
                self.with_file(true, |f| {
                    if let Some(s) = f.money.spent.get_mut(&day) {
                        *s = s.saturating_sub(amount);
                    }
                    if let Some(r) = &reff {
                        if f.money.refs.get(r).is_some_and(|v| v == PENDING) {
                            f.money.refs.remove(r);
                        }
                    }
                });
                self.audit("app.money.failed", &format!("{} payment of {} did not go through", self.app(), money::format_minor(amount, currency)));
                Err(e)
            }
        }
    }

    fn money_raise(&self, a: &Value, declared: &str) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["perDayMinor", "why"]);
        let want = a.get("perDayMinor").and_then(Value::as_u64).filter(|n| *n >= 1 && *n <= money::MAX_MINOR).ok_or_else(|| SdkError::bad("perDayMinor", "a whole number of the smallest unit, more than 0"))?;
        let why = a.get("why").and_then(Value::as_str).map(|w| plain_line(w, 200)).filter(|w| !w.trim().is_empty()).ok_or_else(|| SdkError::bad("why", "say why the app needs it"))?;
        let cur = declared.to_string();
        let now = self.with_file(false, |f| if f.money.currency == cur { f.money.per_day_minor } else { 0 });
        if want <= now {
            return Err(SdkError::bad("perDayMinor", "the limit is already that high"));
        }
        let app = self.app().to_string();
        let lines = vec![
            format!("“{app}” asks to be allowed to spend more: from {} to {} a day.", money::format_minor(now, &cur), money::format_minor(want, &cur)),
            format!("It says: {why}"),
            "This is a limit, not a payment. Every payment is checked against it. You can lower it again at any time.".into(),
        ];
        self.ask("money.requestLimit", None, "An app asks to spend more money", lines, Box::new(move |s: &Sdk2| {
            let ok = s.with_file(true, |f| {
                if f.read_only {
                    return false;
                }
                f.money.currency = cur.clone();
                f.money.per_day_minor = want;
                true
            });
            if !ok {
                return Err(SdkError::new(code::UNAVAILABLE, "this app's limits file is read-only, so the limit could not be changed"));
            }
            s.audit("app.money.limit", &format!("{} limit {} -> {}", s.app(), money::format_minor(now, &cur), money::format_minor(want, &cur)));
            Ok(with_ignored(json!({"perDayMinor": want}), ignored))
        }))
    }
}

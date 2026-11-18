//! The periodic update check (F-UPD-04): looks at every store source the person or administrator configured and, when the
//! administrator set one, the system update index, and keeps the answer for the page, the Overview and a badge.
//!
//! * It only asks addresses that were set: the store's own sources (the official catalog on the portal, a mirror, a folder or a stick)
//!   and `system_url`, which is empty until an administrator fills it in. There is no built-in address here.
//! * It only reports. It never downloads a package or a system update and never installs anything: the person clicks Update in the
//!   STORE, or installs a system pack from the System update page.
//! * An app update that asks for permissions the installed version did not have is marked, and is never applied without the
//!   person reading them (the install review already shows them; the mark makes the list say so before).
//! * Packages that are not in any catalog (a developer's local ones) never appear: only catalog entries are compared.
//! * [`due`] decides when the next check is: daily with a small stable jitter so a fleet does not call at the same second, never when
//!   it is off, and not on a metered link unless the person allows it. The shell does not know which links are metered yet, so it
//!   passes `None` and the page says so.
//!
//! The last answer is kept in `update-check.json` in the data folder; a check that cannot reach a source keeps the older answer for
//! that source's apps out of the list (it is reported as an error, not as "up to date").

use std::path::Path;

use serde::Deserialize;

use iyakki_sec::policy::Policy;

use super::catalog::Fetch;
use super::install::Actor;
use super::{State, Store};
use crate::devmode::update::compare_versions;
use crate::services::Services;

pub use iyakki_shared::store_updatecheck::{check_url, due, jitter_secs, CheckSettings, Due, Kind, Mode, Report, Update, DAY, FILE, NEEDS_NEWER};
/// The system update index is a small file.
const MAX_INDEX: usize = 64 * 1024;

/// An app was installed: drop it from the kept answer when the installed version is the one the check offered (or newer), so the badge
/// and the page stop counting it at once instead of at the next check. Returns whether the answer changed.
pub fn forget_installed(dir: &Path, kind: &str, name: &str, version: &str) -> bool {
    let Some(mut r) = Report::load(dir) else { return false };
    let id = format!("{kind}:{name}");
    let before = r.updates.len();
    r.updates.retain(|u| !(u.kind == Kind::App && u.id == id && compare_versions(&u.to, version) != std::cmp::Ordering::Greater));
    if r.updates.len() == before {
        return false;
    }
    r.save(dir).is_ok()
}

/// The system update index: what an administrator's mirror (or the portal) says is the newest version.
#[derive(Debug, Deserialize)]
struct Index {
    version: String,
    #[serde(default)]
    variant: String,
    #[serde(default)]
    notes: String,
}

/// Look at the sources. Slow (network): call it off the UI thread. `refresh` reads every store source and updates the cached
/// copies; the comparison is then done on what is cached, so a source that could not be reached is an error line, not silence.
#[allow(clippy::too_many_arguments)]
pub fn run(store: &Store, svc: &Services, policy: &Policy, user: &str, fetch: &dyn Fetch, settings: &CheckSettings, running: &str, variant: &str) -> Report {
    let mut rep = Report { checked_at: store.now(), ..Report::default() };
    if settings.mode == Mode::Off {
        return rep;
    }
    // 1. the apps: every enabled source, then the view the STORE shows
    let cfg = store.config();
    rep.asked.extend(cfg.sources.iter().filter(|s| s.enabled).map(|s| s.name.clone()));
    let refreshed = store.refresh(svc, fetch);
    rep.errors.extend(refreshed.errors);
    let actor = Actor::from_policy(user, policy);
    let loaded = store.local_catalogs(svc);
    let items = store.items(svc, &actor, &loaded);
    let installed = store.installed(svc, policy, user, &items);
    for it in &items {
        let State::Update(to) = &it.state else { continue };
        let now_perms: Vec<String> = it.entry.meta.permissions.iter().filter_map(|p| super::manifest::Permission::parse(p).ok()).map(|p| p.words(user)).collect();
        let had: Vec<&String> = installed.iter().find(|e| e.kind == it.entry.kind && e.name == it.entry.name).map(|e| e.permissions.iter().collect()).unwrap_or_default();
        let added: Vec<String> = now_perms.into_iter().filter(|p| !had.contains(&p)).collect();
        rep.updates.push(Update {
            kind: Kind::App,
            id: format!("{}:{}", it.entry.kind, it.entry.name),
            title: it.entry.title().to_string(),
            from: it.installed_version.clone().unwrap_or_default(),
            to: to.clone(),
            source: it.source_name.clone(),
            note: {
                let min = it.entry.meta.min_version.trim();
                let older = !min.is_empty() && crate::devmode::update::compare_versions(running, min) == std::cmp::Ordering::Less;
                match (older, added.is_empty()) {
                    (true, _) => format!("{NEEDS_NEWER}{min} or newer first: update IYAKKI, then this app{}", if added.is_empty() { "." } else { "; it also asks for permissions the installed version did not." }),
                    (false, true) => String::new(),
                    (false, false) => "asks for permissions the installed version did not".into(),
                }
            },
            new_permissions: added,
            driver: it.entry.meta.ext_kind == "driver",
        });
    }
    // 2. the system: only the address an administrator set (or a store the person added, see `sysannounce`)
    if !settings.system_url.is_empty() {
        rep.asked.push(settings.system_url.clone());
        let found: Result<Option<Index>, String> = if super::sysannounce::is_store_announcement(&settings.system_url) {
            let url = settings.system_url.trim();
            match cfg.sources.iter().find(|s| !s.pin.is_empty() && s.kind == super::catalog::SourceKind::Url && super::sysannounce::url_for_catalog(&s.location) == url) {
                None => Err("this address is a store's announcement, but that store is not added with its key checked here, so it is not trusted".into()),
                Some(src) => match check_url(url).and_then(|u| fetch.get(&u, super::sysannounce::MAX)) {
                    Err(e) if e.ends_with("answered 404") => Ok(None),
                    Err(e) => Err(e),
                    Ok(b) => super::sysannounce::verify(&b, &src.pin).map(|a| Some(Index { version: a.version, variant: a.variant, notes: a.notes })),
                },
            }
        } else {
            match check_url(&settings.system_url).and_then(|u| fetch.get(&u, MAX_INDEX)) {
                Err(e) => Err(e),
                Ok(b) => serde_json::from_slice::<Index>(&b).map(Some).map_err(|_| "the answer is not an update index".to_string()),
            }
        };
        match found {
            Err(e) => rep.errors.push(("System update".into(), e)),
            Ok(None) => {}
            Ok(Some(ix)) => {
                if !ix.variant.is_empty() && !variant.is_empty() && ix.variant != variant {
                    // an index for another build of IYAKKI says nothing about this one
                } else if compare_versions(&ix.version, running) == std::cmp::Ordering::Greater {
                    rep.updates.push(Update { kind: Kind::System, id: "system".into(), title: "IYAKKI".into(), from: running.into(), to: ix.version, source: "System update".into(), new_permissions: Vec::new(), note: ix.notes.chars().take(300).collect(), driver: false });
                }
            }
        }
    }
    rep
}

/// [`run`], and the answer is kept for the page.
#[allow(clippy::too_many_arguments)]
pub fn run_and_keep(store: &Store, svc: &Services, policy: &Policy, user: &str, fetch: &dyn Fetch, settings: &CheckSettings, running: &str, variant: &str) -> Report {
    let mut rep = run(store, svc, policy, user, fetch, settings, running, variant);
    if settings.mode != Mode::Off {
        super::updatenotice::merge_extra(&mut rep, svc.dir(), running);
    }
    let _ = rep.save(svc.dir());
    // one grouped notice for everything pending (replaced in place; never installs anything)
    if settings.mode != Mode::Off {
        super::updatenotice::announce_report(svc, user, &rep);
    }
    rep
}

//! Taking a capability away from an installation (the writing side of the approvals file).

use std::path::Path;

/// Give a capability an end time (`hours` from `now`; `None` takes the limit away). `Ok(false)`: the installation does not hold it.
/// The caller writes the audit line (`access.limit`).
pub fn limit(data: &Path, installation: &str, capability: &str, hours: Option<u32>, now: i64) -> Result<bool, String> {
    if hours.is_some_and(|h| h == 0 || h > 24 * 366) {
        return Err("a time limit is 1 hour to 366 days (8784 hours); leave it out to take the limit away".into());
    }
    let store = crate::store::Store::open(data);
    let mut book = crate::store::approvals::FileBook(&store);
    use iyakki_core_api::appcaps::ApprovalBook;
    let Some(mut a) = book.get(installation) else { return Err("no such installation".into()) };
    let changed = a.set_end(capability, hours.map(|h| now + i64::from(h) * 3600), now);
    if changed {
        book.put(installation, a);
    }
    Ok(changed)
}

/// Take a capability away from an installation. `Ok(false)`: it did not hold it. The caller writes the audit line (`access.revoke`).
pub fn revoke(data: &Path, installation: &str, capability: &str) -> Result<bool, String> {
    let store = crate::store::Store::open(data);
    let mut book = crate::store::approvals::FileBook(&store);
    use iyakki_core_api::appcaps::ApprovalBook;
    let Some(mut a) = book.get(installation) else { return Err("no such installation".into()) };
    let changed = a.revoke(capability);
    if changed {
        book.put(installation, a);
    }
    Ok(changed)
}


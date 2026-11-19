//! Taking a capability away from an installation (the writing side of the approvals file).

use std::path::Path;

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


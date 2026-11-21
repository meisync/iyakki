//! What the Private store card shows (data only). Reading the install and the store config stays in `iyakki_agent::store::privatestore`.

/// What the Private store card shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StatusInfo {
    /// The deployed instance, if any (`id`, catalog address, state word).
    pub instance: Option<(String, String)>,
    /// The catalog source already added for it (source id, name).
    pub added: Option<(String, String)>,
    /// The source `publish` goes to by default.
    pub default_target: Option<String>,
}

pub fn status_text(info: &StatusInfo) -> String {
    let mut t = match &info.instance {
        None => "No private store is deployed from this device (STORE, Settings, Private store, Deploy to k3s, or the assistant's platform_deploy with service private-store).".to_string(),
        Some((id, url)) => format!("Private store {id} is deployed; catalog {url}."),
    };
    match &info.added {
        Some((id, name)) => t.push_str(&format!(" It is added as the catalog {name} ({id}), key pinned.")),
        None if info.instance.is_some() => t.push_str(" It is not added to this device's STORE yet (Settings, Private store, Add this store)."),
        None => {}
    }
    if let Some(d) = &info.default_target {
        t.push_str(&format!(" Publishing goes to {d} by default."));
    }
    t
}

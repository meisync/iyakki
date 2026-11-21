use crate::store_core::Probe;

/// A private store the installed-services list knows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Deployed {
    /// The instance id (`private-store-iyakki-private-store`).
    pub instance: String,
    pub title: String,
    pub namespace: String,
    /// The address of its `catalog.json` as recorded at deploy.
    pub catalog_url: String,
    /// The catalog source already added for it (id, name).
    pub added: Option<(String, String)>,
}

/// What the Publish form and the assistant need to offer "add it and publish": the first deployed store that is not yet a source,
/// with what its catalog says now (the key fingerprint) or why that could not be read.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Offer {
    pub dep: Deployed,
    pub probe: Result<Probe, String>,
}

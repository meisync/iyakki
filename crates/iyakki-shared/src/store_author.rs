

/// What a store server answered to a publish.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Published {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub trust: String,
    pub serial: u64,
}

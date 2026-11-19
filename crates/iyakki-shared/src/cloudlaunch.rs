//! Types moved out of `iyakki_agent::cloudlaunch` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Where the image comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageStep {
    /// The cluster can already pull it (a registry the person runs, or an image imported before).
    Existing,
    /// IYAKKI makes its one-file image and pushes it to a plain-HTTP registry (`HOST:PORT/REPO:TAG`; the tenant's image must be that name).
    Push { dest: String },
    /// Only a tar file is made, with the command that loads it on the cluster's node (nothing is imported from here).
    Tar { path: String },
}

/// Which vault the tenant uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultPlan {
    /// None: the vaults stay on the tenant's own volume.
    None,
    /// An address the person gave (`https://...`).
    Remote(String),
    /// The cluster's own vault service is made (`cloudvault.rs`).
    Own,
}

/// The cluster as the K8S tile knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterRef {
    /// Its name as shown (`local k3s`, a context name).
    pub name: String,
    /// The tile's own judgement (`devcluster::ClusterProfile::looks_like_production`).
    pub looks_like_production: bool,
    /// Why changes are blocked on it right now (`devcluster::read_only_reason`), when they are.
    pub read_only: Option<String>,
    /// The person allowed changes on a production-looking cluster for this session.
    pub changes_allowed: bool,
}

/// What would be done, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// (file name, YAML) applied in this order.
    pub documents: Vec<(String, String)>,
    /// Things to know; they do not stop the launch.
    pub notes: Vec<String>,
    /// Things that stop it.
    pub blockers: Vec<String>,
    pub image: Option<ImageStep>,
}

/// What the Launch cloud mode card asks for, before it is turned into the agent's tenant configuration.
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    /// The tenant's name.
    pub tenant: String,
    /// The image the tenant runs (empty: the default one); a pushed image replaces it.
    pub image: String,
    pub vault: VaultPlan,
    pub routes: crate::cloudroute::RouteTable,
    pub step: ImageStep,
    pub cluster: ClusterRef,
}

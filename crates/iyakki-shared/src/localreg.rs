pub use crate::platform_types::Wait;
use crate::regsetup::{self, Facts, KubeCtx};

use crate::regsetup::IN_NS;


use std::path::PathBuf;

/// The registry's address as the pods and the nodes pull it, for a NodePort.
pub fn pull_address(port: u16) -> String {
    format!("localhost:{port}")
}

/// A cluster and how to reach it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Target {
    pub name: String,
    pub kubeconfig: String,
    pub context: String,
    /// The host this machine reaches the nodes' NodePorts at: `localhost` for a cluster on this machine.
    pub node_host: String,
}

impl Target {
    pub fn kube(&self) -> KubeCtx {
        KubeCtx { kubeconfig: self.kubeconfig.clone(), context: self.context.clone() }
    }

    pub fn push_address(&self, port: u16) -> String {
        let h = if self.node_host.is_empty() { "localhost" } else { self.node_host.as_str() };
        format!("{h}:{port}")
    }

    pub fn on_this_machine(&self) -> bool {
        matches!(self.node_host.as_str(), "" | "localhost" | "127.0.0.1" | "::1")
    }
}

#[derive(Clone, Debug)]
pub enum Dependency {
    Met { nodes_ready: usize, storage_class: String, facts: Facts },
    /// One plain sentence: what is missing and what to do.
    Missing(String),
}

impl Dependency {
    pub fn met(&self) -> bool {
        matches!(self, Dependency::Met { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    /// Where this machine pushes (`localhost:30500`, or `<node>:30500`).
    pub push: String,
    /// What pods and nodes pull from.
    pub pull: String,
    pub node_port: u16,
    pub ready: bool,
    /// Images (tags) the registry holds; None: it did not answer from this machine.
    pub images: Option<usize>,
    pub repositories: usize,
    /// `10Gi`, or `emptyDir` for a registry created without a volume.
    pub storage: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Absent,
    /// The objects exist, the pod is not ready: the reason when it is known.
    Starting(Info, String),
    Ready(Info),
    /// The cluster could not be asked.
    Unknown(String),
}

impl State {
    pub fn info(&self) -> Option<&Info> {
        match self {
            State::Starting(i, _) | State::Ready(i) => Some(i),
            _ => None,
        }
    }
}

/// "registry ready · localhost:30500 · used by 3 images" and the like (the one line the deploy screen and the menu show).
pub fn status_line(s: &State) -> String {
    match s {
        State::Absent => "no local registry yet".into(),
        State::Unknown(e) => format!("registry state unknown: {e}"),
        State::Starting(i, why) => format!("registry starting · {} · {why}", i.pull),
        State::Ready(i) => {
            let used = match i.images {
                Some(0) => "no images yet".to_string(),
                Some(1) => "used by 1 image".to_string(),
                Some(n) => format!("used by {n} images"),
                None => "not reachable from this machine yet".to_string(),
            };
            format!("registry ready \u{b7} {} \u{b7} {used}", i.pull)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub size_gi: u32,
    /// 0: 30500 or the next free NodePort.
    pub node_port: u16,
    pub wait: Wait,
    /// Where the manifest is written (it is applied from a file).
    pub work_dir: PathBuf,
}

impl Opts {
    pub fn new(work_dir: PathBuf) -> Opts {
        Opts { size_gi: regsetup::DEFAULT_SIZE_GI, node_port: 0, wait: Wait::REAL, work_dir }
    }
}

/// The volume sizes the form offers.
pub const SIZES_GI: [u32; 4] = [5, 10, 20, 50];

/// The text shown before the button is pressed: what will be created, in plain words.
pub fn describe_plan(size_gi: u32, t: &Target) -> String {
    format!(
        "A local image registry will be created in the cluster{}:\n- namespace {IN_NS}\n- the CNCF Distribution registry (docker.io/library/registry:2, Apache-2.0), pulled once from Docker Hub by the cluster\n- a {size_gi} Gi volume (StorageClass local-path unless the cluster has another default), so the images survive restarts\n- a ClusterIP Service and a port on 127.0.0.1 of the node (30500, or the next free port): pods and the node pull localhost:<port>, IYAKKI pushes to {}:<port>\nThe cluster is NOT restarted and no node file is changed. The registry has no sign-in, so it is NOT published on the network (no NodePort): only this machine can reach it. A registry from an older version that was open on NodePort 30500 is converted the same way.",
        if t.name.is_empty() { String::new() } else { format!(" \"{}\"", t.name) },
        if t.on_this_machine() { "localhost" } else { t.node_host.as_str() }
    )
}

/// The text of the confirmation: what is lost.
pub fn describe_remove(s: &State) -> String {
    let images = s.info().and_then(|i| i.images).map(|n| format!("{n} image(s)")).unwrap_or_else(|| "its images".into());
    format!("Remove the local image registry? This deletes the namespace {IN_NS} with its volume and {images}. Services whose images came from it (the private store) keep running, but their pods cannot be pulled again after a restart until you create the registry and deploy them again. Type {IN_NS} to confirm.")
}

//! P2P swarm: LAN auto-discovery (mDNS) and WASM task distribution over libp2p.
//!
//! Each node listens on TCP (Noise-encrypted, Yamux-multiplexed), announces itself over
//! mDNS, and serves [`TaskRequest`]s by running the module in a locally configured
//! [`Sandbox`] with **no capabilities** (no files, no network) and local fuel/memory caps —
//! peers are not trusted, so a remote task can only compute.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use iyakki_wasm::{Sandbox, SandboxConfig};
use futures::StreamExt;
use libp2p_core::muxing::StreamMuxerBox;
use libp2p_core::transport::Transport as _;
use libp2p_core::upgrade::Version;
use libp2p_core::Multiaddr;
use libp2p_identity::{Keypair, PeerId};
use libp2p_request_response::{self as request_response, ProtocolSupport};
use libp2p_swarm::{NetworkBehaviour, StreamProtocol, Swarm, SwarmEvent};
use libp2p_mdns as mdns;
use libp2p_noise as noise;
use libp2p_tcp as tcp;
use libp2p_yamux as yamux;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

pub use libp2p_core::Multiaddr as Addr;
pub use libp2p_identity::PeerId as Peer;

pub const MAX_WASM_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRequest {
    /// Wasm binary (or WAT text).
    pub wasm: Vec<u8>,
    pub entry: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskResponse {
    pub ok: bool,
    pub result: Option<i32>,
    pub log: Vec<u8>,
    pub error: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SwarmError {
    #[error("swarm setup: {0}")]
    Setup(String),
    #[error("no peers available")]
    NoPeers,
    #[error("request failed: {0}")]
    Request(String),
    #[error("node stopped")]
    Stopped,
}

#[derive(NetworkBehaviour)]
#[behaviour(prelude = "libp2p_swarm::derive_prelude")]
struct Behaviour {
    mdns: mdns::tokio::Behaviour,
    tasks: request_response::cbor::Behaviour<TaskRequest, TaskResponse>,
}

enum Cmd {
    Peers(oneshot::Sender<Vec<PeerId>>),
    Addrs(oneshot::Sender<Vec<Multiaddr>>),
    Dial(Multiaddr, oneshot::Sender<Result<(), String>>),
    Submit(PeerId, TaskRequest, oneshot::Sender<Result<TaskResponse, String>>),
}

#[derive(Clone)]
pub struct Node {
    pub peer_id: PeerId,
    tx: mpsc::Sender<Cmd>,
}

fn execute(sandbox: &Sandbox, req: TaskRequest) -> TaskResponse {
    let fail = |e: String| TaskResponse { ok: false, result: None, log: vec![], error: e };
    if req.wasm.len() > MAX_WASM_BYTES {
        return fail("module too large".into());
    }
    match sandbox.run(&req.wasm, &req.entry, Default::default(), Default::default(), vec![]) {
        Ok(o) => TaskResponse { ok: true, result: o.result, log: o.log, error: String::new() },
        Err(e) => fail(e.to_string()),
    }
}

impl Node {
    /// Start a node listening on `listen` (e.g. `/ip4/0.0.0.0/tcp/0`). Must be called inside a tokio runtime.
    pub fn start(listen: Multiaddr, cfg: SandboxConfig) -> Result<Node, SwarmError> {
        let e = |x: &dyn std::fmt::Display| SwarmError::Setup(x.to_string());
        let key = Keypair::generate_ed25519();
        let peer = key.public().to_peer_id();
        let transport = tcp::tokio::Transport::new(tcp::Config::default())
            .upgrade(Version::V1Lazy)
            .authenticate(noise::Config::new(&key).map_err(|x| e(&x))?)
            .multiplex(yamux::Config::default())
            .map(|(p, m), _| (p, StreamMuxerBox::new(m)))
            .boxed();
        let behaviour = Behaviour {
            mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), peer).map_err(|x| e(&x))?,
            tasks: request_response::cbor::Behaviour::new(
                [(StreamProtocol::new("/iyakki/task/1"), ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(Duration::from_secs(60)),
            ),
        };
        let mut swarm = Swarm::new(
            transport,
            behaviour,
            peer,
            libp2p_swarm::Config::with_tokio_executor().with_idle_connection_timeout(Duration::from_secs(60)),
        );
        swarm.listen_on(listen).map_err(|x| e(&x))?;
        let peer_id = *swarm.local_peer_id();
        let sandbox = Arc::new(Sandbox::new(cfg).map_err(|x| e(&x))?);
        let (tx, mut rx) = mpsc::channel::<Cmd>(64);
        let (done_tx, mut done_rx) = mpsc::channel::<(request_response::ResponseChannel<TaskResponse>, TaskResponse)>(64);

        tokio::spawn(async move {
            let mut peers: HashSet<PeerId> = HashSet::new();
            let mut listens: Vec<Multiaddr> = Vec::new();
            let mut pending: HashMap<request_response::OutboundRequestId, oneshot::Sender<Result<TaskResponse, String>>> =
                HashMap::new();
            loop {
                tokio::select! {
                    cmd = rx.recv() => match cmd {
                        None => break,
                        Some(Cmd::Peers(r)) => { let _ = r.send(peers.iter().copied().collect()); }
                        Some(Cmd::Addrs(r)) => { let _ = r.send(listens.clone()); }
                        Some(Cmd::Dial(a, r)) => { let _ = r.send(swarm.dial(a).map_err(|x| x.to_string())); }
                        Some(Cmd::Submit(p, req, r)) => {
                            let id = swarm.behaviour_mut().tasks.send_request(&p, req);
                            pending.insert(id, r);
                        }
                    },
                    Some((ch, resp)) = done_rx.recv() => {
                        let _ = swarm.behaviour_mut().tasks.send_response(ch, resp);
                    }
                    ev = swarm.select_next_some() => match ev {
                        SwarmEvent::NewListenAddr { address, .. } => listens.push(address),
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => { peers.insert(peer_id); }
                        SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => { peers.remove(&peer_id); }
                        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                            for (p, a) in list {
                                if p == peer_id { continue; } // our own announcement
                                log::info!("mdns: discovered {p} at {a}");
                                if !peers.contains(&p) {
                                    // Every advertised address is tried; unreachable ones (e.g. network
                                    // addresses of virtual interfaces) just fail while another succeeds.
                                    let opts = libp2p_swarm::dial_opts::DialOpts::peer_id(p)
                                        .addresses(vec![a])
                                        .extend_addresses_through_behaviour()
                                        .build();
                                    let _ = swarm.dial(opts);
                                }
                            }
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                            for (p, _) in list { if !swarm.is_connected(&p) { peers.remove(&p); } }
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Tasks(request_response::Event::Message { message, .. })) => match message {
                            request_response::Message::Request { request, channel, .. } => {
                                let (sb, dt) = (sandbox.clone(), done_tx.clone());
                                tokio::task::spawn_blocking(move || {
                                    let resp = execute(&sb, request);
                                    let _ = dt.blocking_send((channel, resp));
                                });
                            }
                            request_response::Message::Response { request_id, response } => {
                                if let Some(r) = pending.remove(&request_id) { let _ = r.send(Ok(response)); }
                            }
                        },
                        SwarmEvent::Behaviour(BehaviourEvent::Tasks(request_response::Event::OutboundFailure { request_id, error, .. })) => {
                            if let Some(r) = pending.remove(&request_id) { let _ = r.send(Err(error.to_string())); }
                        }
                        _ => {}
                    }
                }
            }
        });
        Ok(Node { peer_id, tx })
    }

    async fn ask<T>(&self, f: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> Result<T, SwarmError> {
        let (r, rx) = oneshot::channel();
        self.tx.send(f(r)).await.map_err(|_| SwarmError::Stopped)?;
        rx.await.map_err(|_| SwarmError::Stopped)
    }

    /// Connected peers (including those found via mDNS).
    pub async fn peers(&self) -> Result<Vec<PeerId>, SwarmError> {
        self.ask(Cmd::Peers).await
    }

    pub async fn listen_addrs(&self) -> Result<Vec<Multiaddr>, SwarmError> {
        self.ask(Cmd::Addrs).await
    }

    /// Manually connect to a peer (e.g. across subnets where mDNS doesn't reach).
    pub async fn dial(&self, addr: Multiaddr) -> Result<(), SwarmError> {
        self.ask(|r| Cmd::Dial(addr, r)).await?.map_err(SwarmError::Request)
    }

    pub async fn submit(&self, peer: PeerId, req: TaskRequest) -> Result<TaskResponse, SwarmError> {
        self.ask(|r| Cmd::Submit(peer, req, r)).await?.map_err(SwarmError::Request)
    }

    /// Run on any connected peer (first in the list; callers wanting scheduling policy can use `peers()`).
    pub async fn submit_any(&self, req: TaskRequest) -> Result<(PeerId, TaskResponse), SwarmError> {
        let p = *self.peers().await?.first().ok_or(SwarmError::NoPeers)?;
        Ok((p, self.submit(p, req).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAT: &str = r#"(module (import "iyakki" "log" (func $l (param i32 i32))) (memory (export "memory") 1)
      (data (i32.const 0) "remote!") (func (export "main") (result i32) (call $l (i32.const 0) (i32.const 7)) i32.const 7))"#;

    async fn node(fuel: u64) -> Node {
        Node::start("/ip4/127.0.0.1/tcp/0".parse().unwrap(), SandboxConfig { fuel, ..Default::default() }).unwrap()
    }

    async fn wait_addr(n: &Node) -> Multiaddr {
        for _ in 0..100 {
            if let Some(a) = n.listen_addrs().await.unwrap().first() {
                return a.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no listen addr");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_task_on_remote_peer_with_remote_limits() {
        let (a, b) = (node(1_000_000).await, node(10_000).await);
        let baddr = wait_addr(&b).await;
        a.dial(baddr).await.unwrap();
        for _ in 0..100 {
            if !a.peers().await.unwrap().is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (p, r) = a.submit_any(TaskRequest { wasm: WAT.into(), entry: "main".into() }).await.unwrap();
        assert_eq!(p, b.peer_id);
        assert!(r.ok, "{}", r.error);
        assert_eq!((r.result, r.log.as_slice()), (Some(7), &b"remote!"[..]));

        // The *remote's* fuel cap applies: an infinite loop is stopped, and reported.
        let spin = r#"(module (func (export "main") (loop br 0)))"#;
        let r = a.submit(b.peer_id, TaskRequest { wasm: spin.into(), entry: "main".into() }).await.unwrap();
        assert!(!r.ok && r.error.contains("fuel"), "{r:?}");

        // Remote code gets no capabilities: file access is denied.
        let w = r#"(module (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1) (data (i32.const 64) "/x")
          (func (export "main") (result i32) (call $fw (i32.const 0) (i32.const 64) (i32.const 2) (i32.const 0) (i32.const 0))))"#;
        let r = a.submit(b.peer_id, TaskRequest { wasm: w.into(), entry: "main".into() }).await.unwrap();
        assert_eq!(r.result, Some(iyakki_wasm::errno::DENIED));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_peers_is_an_error() {
        let a = node(1000).await;
        assert!(matches!(
            a.submit_any(TaskRequest { wasm: vec![], entry: "x".into() }).await,
            Err(SwarmError::NoPeers)
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_module_rejected() {
        let (a, b) = (node(1000).await, node(1000).await);
        a.dial(wait_addr(&b).await).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let r = a.submit(b.peer_id, TaskRequest { wasm: vec![0; MAX_WASM_BYTES + 1], entry: "x".into() }).await;
        // either the remote rejects it, or the transport's message-size limit does first
        match r {
            Ok(resp) => assert!(!resp.ok),
            Err(_) => {}
        }
    }

    /// Needs working multicast on the host (not available in many CI containers):
    /// run with `cargo test -p iyakki-swarm -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn mdns_auto_discovery() {
        let any = || Node::start("/ip4/0.0.0.0/tcp/0".parse().unwrap(), SandboxConfig::default()).unwrap();
        let (a, b) = (any(), any());
        for _ in 0..150 {
            if a.peers().await.unwrap().contains(&b.peer_id) { return; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("peers did not discover each other via mDNS");
    }
}

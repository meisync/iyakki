//! Port-forwards the person asks for (F-K8S-24, F-K8S-26): the code is the core facet `iyakki_core_api::portforward` (`Os::port_forwards`; the real one
//! is `iyakki_core_local::LocalPortForwards`, a fake is `iyakki_core_fake::FakePortForwards`), so the K8S tile, the chat tool `k8s_port_forward`
//! (`k8stools.rs`, asks first) and `dev forward` share one list. This module keeps the old paths.

pub use iyakki_core_api::portforward::{command, forwarding_port, Forwards, Info, PortForwards, Request, Target, MAX_FORWARDS};

/// Queue the Terminal's request for a shell in a pod (the item the K8S tile writes too: `pod:` and six lines); the shell takes it on its next beat.
pub fn queue_pod_terminal(e: &iyakki_core_api::terminal::PtyExec) {
    iyakki_shared::podterm::queue(&format!("pod:{}\n{}\n{}\n{}\n{}\n{}", e.kubeconfig, e.context, e.namespace, e.pod, e.container, e.shell));
}

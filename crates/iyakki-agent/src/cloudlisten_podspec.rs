//! The pod side of the TLS link as Kubernetes manifests (F-CLOUD-49): the sidecar container and its certificate mount for the Deployment
//! behind a route that has pod TLS, the Service change that points the route at the sidecar, and the Secret that holds the pod's end of
//! the link.
//!
//! The repo does not generate the Deployments of a route's target (they are the person's own), so this gives strategic-merge patches that
//! `kubectl patch deployment NAME --patch-file F` applies: containers and volumes are merged by name, so the person's containers are
//! untouched and a second patch changes nothing. Golden tests (`podspec_tests`) hold the exact text.
//!
//! * [`deployment_patch`]: adds the container `iyakki-pod-tls` (the IYAKKI image running `cloud pod-tls serve` on the TLS port, forwarding
//!   to the application's plain port on 127.0.0.1 of the same pod) and the volume `pod-tls` from the Secret [`secret_name`]. The container
//!   is hardened like the tenant's own (non-root, no privilege escalation, read-only root, all capabilities dropped, seccomp default) and
//!   small (limits 100m, 64Mi).
//! * [`service_patch`]: sets the `targetPort` of the route's Service port to the TLS port, so the listener reaches the sidecar and never
//!   the application.
//! * [`secret_manifest`]: the Secret `NAME-pod-tls` with the pod's CA, certificate and key. It holds a private key, so the command prints
//!   it only into `kubectl` (`--apply`), never to the terminal.
//!
//! The application should listen on 127.0.0.1 only (the sidecar is the only way in). The Secret volume is readable by the pod's own
//! containers (mode 0444): the application shares the pod and could read the key; the key is for this pod only and rotation replaces it.

use super::podtls::Material;
use crate::cloudroute::{RouteTable, Scheme, Target};
use crate::cloudtenant::dns_label;

pub const CONTAINER: &str = "iyakki-pod-tls";
pub const VOLUME: &str = "pod-tls";
pub const MOUNT: &str = "/pod-tls";
pub const DEFAULT_TLS_PORT: u16 = 8443;

/// The Secret of the pod's end of the link `name`.
pub fn secret_name(name: &str) -> String {
    format!("{name}-pod-tls")
}

pub struct SpecOpts<'a> {
    /// The pod link name (as in `--pod-tls HOST=NAME`).
    pub name: &'a str,
    /// The IYAKKI image that runs the sidecar.
    pub image: &'a str,
    /// The port the sidecar listens on in the pod.
    pub tls_port: u16,
    /// The application's plain port, on 127.0.0.1.
    pub backend_port: u16,
}

fn check(o: &SpecOpts) -> Result<(), String> {
    if !crate::cloudstorage::plain_name(o.name) || !dns_label(&secret_name(o.name)) {
        return Err(format!("\"{}\" is not a name that can name a Secret (lower case letters, digits and -)", o.name));
    }
    if o.tls_port == 0 || o.backend_port == 0 || o.tls_port == o.backend_port {
        return Err("the TLS port and the application port must be two different ports above 0".into());
    }
    if o.image.is_empty() || o.image.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'' || c.is_control()) {
        return Err("the image is a reference such as registry.example.org/iyakki:tag".into());
    }
    Ok(())
}

/// The strategic-merge patch for the Deployment `deployment`.
pub fn deployment_patch(deployment: &str, o: &SpecOpts) -> Result<String, String> {
    check(o)?;
    if !dns_label(deployment) {
        return Err(format!("\"{deployment}\" is not a Deployment name"));
    }
    Ok(format!(
        "# kubectl patch deployment {deployment} --patch-file THIS-FILE\n\
         spec:\n  template:\n    spec:\n      containers:\n        - name: {CONTAINER}\n          image: \"{image}\"\n          imagePullPolicy: IfNotPresent\n          args: [\"cloud\", \"pod-tls\", \"serve\", \"--name\", \"{name}\", \"--listen\", \"0.0.0.0:{tls}\", \"--backend-port\", \"{backend}\", \"--cert-source\", \"file\", \"--cert-dir\", \"{MOUNT}\"]\n          ports:\n            - {{ name: pod-tls, containerPort: {tls} }}\n          securityContext:\n            runAsNonRoot: true\n            allowPrivilegeEscalation: false\n            readOnlyRootFilesystem: true\n            capabilities:\n              drop: [\"ALL\"]\n            seccompProfile:\n              type: RuntimeDefault\n          resources:\n            requests: {{ cpu: 10m, memory: 16Mi }}\n            limits: {{ cpu: 100m, memory: 64Mi }}\n          volumeMounts:\n            - {{ name: {VOLUME}, mountPath: {MOUNT}, readOnly: true }}\n      volumes:\n        - name: {VOLUME}\n          secret:\n            secretName: {secret}\n            defaultMode: 292\n",
        image = o.image,
        name = o.name,
        tls = o.tls_port,
        backend = o.backend_port,
        secret = secret_name(o.name),
    ))
}

/// The patch for the Service `service`: its port `port` goes to the sidecar.
pub fn service_patch(service: &str, port: u16, o: &SpecOpts) -> Result<String, String> {
    check(o)?;
    if !dns_label(service) || port == 0 {
        return Err(format!("service \"{service}\" port {port} is not valid"));
    }
    Ok(format!("# kubectl patch service {service} --patch-file THIS-FILE\nspec:\n  ports:\n    - port: {port}\n      targetPort: {}\n", o.tls_port))
}

/// The Secret with the pod's CA, certificate and key, in the file names `cloud pod-tls serve --cert-source file` reads. Holds a private
/// key: hand it to `kubectl` only.
pub fn secret_manifest(namespace: &str, name: &str, m: &Material) -> Result<String, String> {
    if !dns_label(namespace) {
        return Err(format!("namespace \"{namespace}\" is not a name"));
    }
    if !crate::cloudstorage::plain_name(name) || !dns_label(&secret_name(name)) {
        return Err(format!("\"{name}\" is not a name that can name a Secret"));
    }
    let b = |s: &str| iyakki_link::sigscheme::base64(s.as_bytes());
    Ok(format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {}\n  namespace: {namespace}\n  labels:\n    app.kubernetes.io/managed-by: iyakki\ntype: Opaque\ndata:\n  {name}.pod-ca: {}\n  {name}.pod-server.crt: {}\n  {name}.pod-server.key: {}\n",
        secret_name(name),
        b(&m.ca),
        b(&m.chain),
        b(&m.key)
    ))
}

/// The Service (name and port) a route with pod TLS points at: the route of `host` must be an http or https route to a Service.
pub fn service_of(t: &RouteTable, host: &str) -> Result<(String, u16), String> {
    let r = t.routes.iter().find(|r| r.host.eq_ignore_ascii_case(host)).ok_or_else(|| format!("the routes file has no route for {host}"))?;
    if r.scheme == Scheme::Passthrough {
        return Err(format!("{host} is a TLS passthrough route: the listener does not open it, so there is no link to wrap"));
    }
    match &r.target {
        Target::Service { name, port } => Ok((name.clone(), *port)),
        Target::Local { .. } => Err(format!("{host} points at a local port, not at a pod")),
        Target::Tunnel { .. } | Target::Peer { .. } => Err(format!("{host} points at an SSH tunnel or a VPN peer, not at a pod")),
    }
}

#[cfg(test)]
#[path = "cloudlisten_podspec_tests.rs"]
mod podspec_tests;

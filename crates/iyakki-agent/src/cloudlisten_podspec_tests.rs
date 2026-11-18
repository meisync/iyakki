//! Golden tests of the pod-side manifests (`cloudlisten_podspec.rs`): the exact text is held, so a change to the sidecar is a visible diff.

use super::*;
use crate::cloudlisten::podtls::{Material, PodOptions, PodServer};
use crate::cloudlisten::{CertSource, FileCerts};

fn opts() -> SpecOpts<'static> {
    SpecOpts { name: "shop", image: "registry.example.org/iyakki:1.2", tls_port: 8443, backend_port: 8080 }
}

const DEPLOYMENT: &str = r#"# kubectl patch deployment shop-web --patch-file THIS-FILE
spec:
  template:
    spec:
      containers:
        - name: iyakki-pod-tls
          image: "registry.example.org/iyakki:1.2"
          imagePullPolicy: IfNotPresent
          args: ["cloud", "pod-tls", "serve", "--name", "shop", "--listen", "0.0.0.0:8443", "--backend-port", "8080", "--cert-source", "file", "--cert-dir", "/pod-tls"]
          ports:
            - { name: pod-tls, containerPort: 8443 }
          securityContext:
            runAsNonRoot: true
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
            seccompProfile:
              type: RuntimeDefault
          resources:
            requests: { cpu: 10m, memory: 16Mi }
            limits: { cpu: 100m, memory: 64Mi }
          volumeMounts:
            - { name: pod-tls, mountPath: /pod-tls, readOnly: true }
      volumes:
        - name: pod-tls
          secret:
            secretName: shop-pod-tls
            defaultMode: 292
"#;

const SERVICE: &str = "# kubectl patch service shop-web --patch-file THIS-FILE\nspec:\n  ports:\n    - port: 80\n      targetPort: 8443\n";

#[test]
fn the_deployment_patch_adds_one_hardened_sidecar_and_one_secret_volume() {
    assert_eq!(deployment_patch("shop-web", &opts()).unwrap(), DEPLOYMENT);
    // the patch is data: it names no secret value and no address
    assert!(!DEPLOYMENT.contains("PRIVATE") && !DEPLOYMENT.contains("password"));
    // every flag it passes is one the command reads
    for f in ["--name", "--listen", "--backend-port", "--cert-source", "--cert-dir"] {
        assert!(crate::cloudlisten::podtls::USAGE.contains(f), "{f}");
    }
}

#[test]
fn the_service_patch_points_the_route_port_at_the_sidecar() {
    assert_eq!(service_patch("shop-web", 80, &opts()).unwrap(), SERVICE);
}

#[test]
fn the_secret_has_the_file_names_the_sidecar_reads_and_the_sidecar_starts_from_them() {
    let m = Material { ca: "CA-TEXT\n".into(), chain: "CHAIN-TEXT\n".into(), key: "KEY-TEXT\n".into() };
    let y = secret_manifest("team-a", "shop", &m).unwrap();
    assert_eq!(
        y,
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: shop-pod-tls\n  namespace: team-a\n  labels:\n    app.kubernetes.io/managed-by: iyakki\ntype: Opaque\ndata:\n  shop.pod-ca: Q0EtVEVYVAo=\n  shop.pod-server.crt: Q0hBSU4tVEVYVAo=\n  shop.pod-server.key: S0VZLVRFWFQK\n"
    );
    // the same names in a folder (what a Secret volume looks like) are what the sidecar's file source loads, and a real pod link starts
    let d = std::env::temp_dir().join(format!("iyakki-podspec-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let mem = crate::cloudlisten::vault::VaultCerts::new(Box::new(Mem::default()));
    let _ = mem;
    let store = Mem::default();
    crate::cloudlisten::podtls::provision(&store, "shop", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64, 30).unwrap();
    let served = crate::cloudlisten::podtls::read_material(&store, "shop", "server").unwrap();
    // decode what the manifest would carry and write it under the Secret's keys
    let manifest = secret_manifest("team-a", "shop", &served).unwrap();
    for (key, file) in [("shop.pod-ca", "shop.pod-ca"), ("shop.pod-server.crt", "shop.pod-server.crt"), ("shop.pod-server.key", "shop.pod-server.key")] {
        let line = manifest.lines().find(|l| l.trim_start().starts_with(&format!("{key}:"))).unwrap();
        let v = iyakki_link::sigscheme::unbase64(line.split_once(": ").unwrap().1).unwrap();
        std::fs::write(d.join(file), v).unwrap();
    }
    let loaded = FileCerts::new(&d).load_pod_server("shop").unwrap();
    let server = PodServer::start("127.0.0.1:0".parse().unwrap(), &loaded, PodOptions::new(1)).unwrap();
    assert!(server.addr().port() > 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[derive(Clone, Default)]
struct Mem(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>);
impl crate::cloudlisten::vault::SecretReader for Mem {
    fn read(&self, n: &str) -> Result<Vec<u8>, String> {
        self.0.lock().unwrap().get(n).cloned().ok_or_else(|| "404".into())
    }
}
impl crate::cloudlisten::podtls::SecretWriter for Mem {
    fn write(&self, n: &str, v: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().insert(n.into(), v.to_vec());
        Ok(())
    }
}

#[test]
fn bad_names_ports_and_images_are_refused_and_the_route_decides_the_service() {
    let o = opts();
    assert!(deployment_patch("Shop Web", &o).is_err());
    assert!(deployment_patch("shop-web", &SpecOpts { name: "Shop_X", ..opts() }).is_err(), "a Secret name is lower case");
    assert!(deployment_patch("shop-web", &SpecOpts { tls_port: 8080, ..opts() }).is_err(), "the two ports differ");
    assert!(deployment_patch("shop-web", &SpecOpts { backend_port: 0, ..opts() }).is_err());
    assert!(deployment_patch("shop-web", &SpecOpts { image: "x\" ; evil", ..opts() }).is_err(), "no quote or space in the image");
    assert!(service_patch("shop-web", 0, &o).is_err());
    assert!(secret_manifest("Team A", "shop", &Material { ca: String::new(), chain: String::new(), key: String::new() }).is_err());
    use crate::cloudroute::{Access, Route, RouteTable, Scheme, Target};
    let r = |host: &str, target: Target, scheme: Scheme| Route { host: host.into(), path_prefix: "/".into(), target, scheme, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Public, client_ca: None };
    let t = RouteTable { routes: vec![r("shop.example.org", Target::Service { name: "shop-web".into(), port: 80 }, Scheme::Https), r("loc.example.org", Target::Local { port: 1 }, Scheme::Http), r("pass.example.org", Target::Service { name: "p".into(), port: 443 }, Scheme::Passthrough)], ingress_class: None };
    assert_eq!(service_of(&t, "SHOP.example.org"), Ok(("shop-web".to_string(), 80)));
    assert!(service_of(&t, "loc.example.org").is_err() && service_of(&t, "pass.example.org").is_err() && service_of(&t, "nothere.example.org").is_err());
}

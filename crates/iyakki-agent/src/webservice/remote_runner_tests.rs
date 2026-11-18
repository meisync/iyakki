//! The remote runner (`flows/remote.rs`) against the real routes of a second service run in this process: the sign-in is checked
//! there, the flow routes are the real ones, and the answer is read back.

use super::*;
use crate::flows::remote::{run_flow, RemoteTarget, RunOptions};
use iyakki_proxy::{Upstream, UpstreamResponse};
use zeroize::Zeroizing;

/// A transport that hands the request to the other service's own request path, as a network would.
struct Wire<'a>(&'a H);

impl Upstream for Wire<'_> {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        let path = url.strip_prefix("https://other.local:8443").ok_or("wrong host")?;
        let token = headers.iter().find(|h| h.0 == "Authorization").and_then(|h| h.1.strip_prefix("Bearer ")).map(str::to_string);
        let body = if body.is_empty() { None } else { serde_json::from_slice::<Value>(body).ok() };
        let (s, v, _) = self.0.call(method, path, token.as_deref(), body);
        Ok(UpstreamResponse { status: s, body: v.to_string().into_bytes() })
    }
}

#[test]
fn a_flow_runs_on_the_other_service_with_its_own_sign_in_and_its_own_rules() {
    let other = harness("cloud", |_| {});
    let token = other.ann();
    let up = Wire(&other);
    let t = RemoteTarget::new("https://other.local:8443", Zeroizing::new(token)).unwrap();
    let o = run_flow(&up, &t, "f1", &json!({}), &RunOptions::default(), &|| false, &|_| {}).unwrap();
    assert_eq!((o.run.as_str(), o.status.as_str(), o.ended), ("r1", "succeeded", true), "{o:?}");
    // a flow the other person does not have
    let e = run_flow(&up, &t, "nope", &json!({}), &RunOptions::default(), &|| false, &|_| {}).unwrap_err();
    assert!(e.contains("no such flow"), "{e}");
    // a wrong token is refused over there and said so here
    let bad = RemoteTarget::new("https://other.local:8443", Zeroizing::new("not-a-real-token".into())).unwrap();
    let e = run_flow(&up, &bad, "f1", &json!({}), &RunOptions::default(), &|| false, &|_| {}).unwrap_err();
    assert!(e.contains("did not accept the sign-in"), "{e}");
    // the other service's audit says who started it
    assert!(other.accounts.lines().join("\n").contains("flow=f1 run=r1"));
}

#[test]
fn a_service_without_the_flow_routes_says_so() {
    let other = harness("agent-api", |_| {});
    let t = RemoteTarget::new("https://other.local:8443", Zeroizing::new(other.ann())).unwrap();
    let e = run_flow(&Wire(&other), &t, "f1", &json!({}), &RunOptions::default(), &|| false, &|_| {}).unwrap_err();
    assert!(e.contains("answered") || e.contains("no such"), "{e}");
}

#[test]
fn a_flow_runs_over_real_https_on_a_machine_whose_certificate_comes_from_a_private_authority() {
    use iyakki_proxy::https::{HttpOptions, UreqUpstream};
    let other = harness("cloud", |_| {});
    let token = other.ann();
    let pem = std::fs::read_to_string(other.dir.join("tls/cert.pem")).unwrap();
    // the saved machine keeps the authority's certificate, and the host builds the transport from it as `remote_flow` does
    let data = crate::testdir::outside_project("machines", "ca");
    let m = crate::flows::machines::Machines::new(&data)
        .add_with_ca("lab", &format!("https://localhost:{}", other.svc.local_addr().port()), "lab-token", &pem)
        .unwrap();
    let t = RemoteTarget::new(&m.url, Zeroizing::new(token)).unwrap();
    let with_ca = UreqUpstream::with_options(&HttpOptions { timeout: Duration::from_secs(20), ca_pem: Some(m.ca.clone().into_bytes()), ..Default::default() }).unwrap();
    let o = run_flow(&with_ca, &t, "f1", &json!({}), &RunOptions::default(), &|| false, &|_| {}).unwrap();
    assert_eq!((o.status.as_str(), o.ended), ("succeeded", true), "{o:?}");
    // the same address without that authority is not trusted: nothing is sent past the handshake
    let without = UreqUpstream::with_options(&HttpOptions { timeout: Duration::from_secs(20), ..Default::default() }).unwrap();
    let e = run_flow(&without, &t, "f1", &json!({}), &RunOptions::default(), &|| false, &|_| {}).unwrap_err();
    assert!(e.contains("could not reach"), "{e}");
}

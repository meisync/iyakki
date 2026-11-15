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

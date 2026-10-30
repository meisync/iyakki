//! `browser_headers`: the HTTP response headers of the page the person has open, without the engine probe.
//!
//! IYAKKI fetches them itself (one HEAD request, or a GET for one byte when HEAD is refused) through the web tools' client, so the
//! same host policy, local-address refusal, rate limit, size and time caps apply. At most 5 redirects are followed by hand and every
//! hop is listed. No cookies and no credentials are sent, so the headers can differ from what the signed-in browser received; the
//! answer says so. Values of cookie, authentication and token-like headers are replaced by `<redacted>`, long values are cut and the
//! number of headers is capped. A real engine-side list (WebKitURIResponse, `resource-load-started` in the helper) is a later option.

use iyakki_sec::policy::Policy;
use iyakki_web::Url;
use serde_json::Value;

use crate::devtools::{scrub_text, scrub_url};
use crate::services::Services;

/// Headers shown per response.
pub const MAX_HEADERS: usize = 40;
/// Longest header value shown.
pub const MAX_VALUE: usize = 300;

/// Header names whose values are never shown.
const SECRET_NAMES: [&str; 12] = ["set-cookie", "cookie", "authorization", "proxy-authorization", "proxy-authenticate", "www-authenticate", "authentication-info", "x-api-key", "token", "secret", "session", "auth"];

/// The value to show for a header: `<redacted>` for cookies, credentials and token-like values, otherwise the scrubbed, cut text.
pub fn scrub_header(name: &str, value: &str) -> String {
    let n = name.to_ascii_lowercase();
    if SECRET_NAMES.iter().any(|w| n.contains(w)) {
        return "<redacted>".into();
    }
    let v = scrub_text(value);
    if v.chars().count() > MAX_VALUE {
        format!("{}…", v.chars().take(MAX_VALUE).collect::<String>())
    } else {
        v
    }
}

fn find<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// One line about the security-relevant headers of the final response.
pub fn security_summary(h: &[(String, String)], content_type: &str) -> String {
    let show = |name: &str| find(h, name).map(|v| scrub_header(name, v));
    let hsts = show("strict-transport-security").map_or("absent".to_string(), |v| format!("present ({v})"));
    let csp = if find(h, "content-security-policy").is_some() {
        "present".to_string()
    } else if find(h, "content-security-policy-report-only").is_some() {
        "report-only".to_string()
    } else {
        "absent".to_string()
    };
    let ct = show("content-type").unwrap_or_else(|| scrub_text(content_type));
    format!(
        "Security summary: HSTS {hsts}; CSP {csp}; X-Frame-Options {}; X-Content-Type-Options {}; Referrer-Policy {}; Content-Type {}; Server {}; Cache-Control {}.",
        show("x-frame-options").unwrap_or_else(|| "absent".into()),
        show("x-content-type-options").unwrap_or_else(|| "absent".into()),
        show("referrer-policy").unwrap_or_else(|| "absent".into()),
        if ct.is_empty() { "not sent".to_string() } else { ct },
        show("server").unwrap_or_else(|| "not sent".into()),
        show("cache-control").unwrap_or_else(|| "not sent".into()),
    )
}

/// Same site: the same host, or one is a subdomain of the other.
fn same_site(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches("www.").to_ascii_lowercase(), b.trim_start_matches("www.").to_ascii_lowercase());
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// The answer text of the tool (the caller wraps it as untrusted page data).
pub fn report(svc: &Services, p: &Policy, user: &str, args: &Value) -> String {
    let Some(page) = svc.page_snapshot_of(user) else {
        return "Page: (no page is open in the BROWSER tile)\nThere is no open page to read the headers of. Open the page in the BROWSER tile first.".into();
    };
    let page_url = Url::parse(&page.url).ok();
    let mut target = page.url.clone();
    if let Some(u) = args.get("url").and_then(Value::as_str).map(str::trim).filter(|u| !u.is_empty()) {
        let (Ok(want), Some(open)) = (Url::parse(u), page_url.as_ref()) else {
            return format!("Page: {}\nThe address is not usable.", scrub_url(&page.url));
        };
        if !same_site(&want.host, &open.host) {
            return format!("Page: {}\nbrowser_headers only reads addresses on the same site as the open page ({}), not {}.", scrub_url(&page.url), open.host, want.host);
        }
        target = u.to_string();
    }
    let mut out = format!("Page: {}\n", scrub_url(&page.url));
    out.push_str("IYAKKI fetched these headers itself with one extra HEAD request (not read from the browser engine), with no cookies or credentials, so they may differ from what the signed-in browser received. Set-Cookie, Authorization and token-like values are shown as <redacted>.\n");
    let chain = svc.web.header_chain(p, &target);
    for (i, h) in chain.hops.iter().enumerate() {
        let loc = h.location.as_deref().map(|l| format!(", Location: {}", scrub_url(&h.url.join(l).map(|u| u.to_string()).unwrap_or_else(|_| l.to_string())))).unwrap_or_default();
        out.push_str(&format!("Request {}: {} {} -> status {}{}\n", i + 1, h.how, scrub_url(&h.url.to_string()), h.status, loc));
    }
    if let Some(e) = &chain.error {
        out.push_str(&format!("Stopped: {e}\n"));
    }
    if let Some(last) = chain.hops.last().filter(|h| !(300..400).contains(&h.status) || chain.error.is_none()) {
        if last.headers.is_empty() {
            out.push_str("The connection did not report any response headers.\n");
        } else {
            out.push_str(&format!("Headers of the last response (status {}, {} header{}{}):\n", last.status, last.headers.len(), if last.headers.len() == 1 { "" } else { "s" }, if last.headers.len() > MAX_HEADERS { format!(", first {MAX_HEADERS} shown") } else { String::new() }));
            for (k, v) in last.headers.iter().take(MAX_HEADERS) {
                let k: String = k.chars().filter(|c| c.is_ascii_graphic()).take(60).collect();
                out.push_str(&format!("{k}: {}\n", scrub_header(&k, v)));
            }
        }
        out.push_str(&security_summary(&last.headers, &last.content_type));
        out.push('\n');
    }
    out.trim_end().to_string()
}

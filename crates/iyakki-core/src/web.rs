//! Starting the browser front end (`iyakki-remote`) from the command line, the boot entry or the environment.

#[cfg(feature = "web")]
use std::path::Path;

/// Where the server listens and whether it speaks HTTPS.
#[derive(Debug, PartialEq, Eq)]
pub struct WebSpec {
    pub addr: String,
    pub tls: bool,
}

pub const DEFAULT_TLS_PORT: u16 = 8443;
pub const DEFAULT_PLAIN_PORT: u16 = 8080;

/// `1` or a port or `host:port` mean HTTPS; the same after `http:` mean plain HTTP (for a proxy that does the TLS).
/// `0`, `off` and an empty value mean "no".
pub fn parse_spec(v: &str) -> Option<WebSpec> {
    let v = v.trim();
    if v.is_empty() || v == "0" || v.eq_ignore_ascii_case("off") {
        return None;
    }
    let (tls, rest) = match v.strip_prefix("http:") {
        Some(r) => (false, r),
        None => (true, v),
    };
    let default_port = if tls { DEFAULT_TLS_PORT } else { DEFAULT_PLAIN_PORT };
    let addr = if rest == "1" || rest.is_empty() {
        format!("0.0.0.0:{default_port}")
    } else if rest.chars().all(|c| c.is_ascii_digit()) {
        format!("0.0.0.0:{}", rest.parse::<u16>().ok().filter(|p| *p > 0)?)
    } else if rest.contains(':') {
        rest.to_string()
    } else {
        format!("{rest}:{default_port}")
    };
    Some(WebSpec { addr, tls })
}

/// Start the server for the shell `env`; the certificate lives in `<data>/tls`.
#[cfg(feature = "web")]
pub fn start(spec: &WebSpec, env: iyakki_shell::shell::ShellEnv, data_dir: &Path) -> Result<iyakki_remote::server::Server, String> {
    use std::sync::Arc;
    let tls = if spec.tls {
        let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
        let names: Vec<String> = [host].into_iter().filter(|h| !h.is_empty()).collect();
        let id = iyakki_remote::tls::identity(&data_dir.join("tls"), &names)?;
        if id.self_signed {
            log::warn!("web: made a self-signed certificate; the browser will warn once. Fingerprint SHA-256 {}", id.fingerprint);
        } else {
            log::info!("web: certificate fingerprint SHA-256 {}", id.fingerprint);
        }
        Some(id.config)
    } else {
        if !spec.addr.starts_with("127.") && !spec.addr.starts_with("localhost") {
            log::warn!("web: plain HTTP on {}: passwords cross the network unprotected unless a TLS proxy sits in front", spec.addr);
        }
        None
    };
    let server = iyakki_remote::server::Server::start(iyakki_remote::server::Config { addr: spec.addr.clone(), tls }, Arc::new(iyakki_remote::screen::ShellGate::new(env)))
        .map_err(|e| format!("cannot listen on {}: {e}", spec.addr))?;
    // the sign-in relay page is served here while this front end runs (iyakki_agent::extagent::relay)
    iyakki_agent::extagent::relay::note_web_front_end(server.local_addr().port(), spec.tls);
    // Settings -> Remote access shows "on (set at boot)" and does not start a second listener
    iyakki_agent::webaccess::note_boot(server.local_addr().port());
    log::info!("web: browser access on {}://{}", if spec.tls { "https" } else { "http" }, server.local_addr());
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_option_understands_ports_hosts_and_plain_http() {
        let w = |a: &str, tls| Some(WebSpec { addr: a.into(), tls });
        assert_eq!(parse_spec("1"), w("0.0.0.0:8443", true));
        assert_eq!(parse_spec("9443"), w("0.0.0.0:9443", true));
        assert_eq!(parse_spec("127.0.0.1:9000"), w("127.0.0.1:9000", true));
        assert_eq!(parse_spec("myhost"), w("myhost:8443", true));
        assert_eq!(parse_spec("http:1"), w("0.0.0.0:8080", false));
        assert_eq!(parse_spec("http:3000"), w("0.0.0.0:3000", false));
        assert_eq!(parse_spec("http:127.0.0.1:3000"), w("127.0.0.1:3000", false));
        assert_eq!(parse_spec("0"), None);
        assert_eq!(parse_spec("off"), None);
        assert_eq!(parse_spec(""), None);
        assert_eq!(parse_spec("99999"), None, "not a port");
    }
}

use crate::cloudnames::dns_subdomain;

/// A vault address: https only (the keys are wrapped, but the traffic still has a name and a size), a host name, optional port, no path.
pub fn check_url(u: &str) -> Result<(), String> {
    let rest = u.strip_prefix("https://").ok_or_else(|| format!("vault address \"{u}\" must start with https://"))?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, Some(p)),
        _ => (rest, None),
    };
    if !dns_subdomain(host) || host.len() > 253 {
        return Err(format!("vault address \"{u}\": \"{host}\" is not a host name"));
    }
    if port.is_some_and(|p| p.parse::<u32>().map_or(true, |n| n == 0 || n > 65535)) {
        return Err(format!("vault address \"{u}\": the port is not valid"));
    }
    Ok(())
}

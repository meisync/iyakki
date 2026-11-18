//! Percent-encoding and the query-string reader.

/// Encode everything except the unreserved characters (A-Z a-z 0-9 - _ . ~), as a URL component.
pub fn encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// Decode `%XX`; `plus` turns '+' into a space (query strings). Bad escapes are an error.
pub fn decode(s: &str, plus: bool) -> Result<String, String> {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                o.push(h.ok_or_else(|| format!("Bad escape at character {}: '%' must be followed by two hex digits", i + 1))?);
                i += 3;
            }
            b'+' if plus => {
                o.push(b' ');
                i += 1;
            }
            c => {
                o.push(c);
                i += 1;
            }
        }
    }
    Ok(String::from_utf8_lossy(&o).into_owned())
}

pub fn encode_action(input: &str) -> Result<String, String> {
    Ok(encode(input))
}

pub fn decode_action(input: &str) -> Result<String, String> {
    decode(input.trim(), true)
}

/// Split a URL (or a bare query string) into its parts and list the query parameters, decoded.
pub fn parse(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Paste a URL or a query string like a=1&b=two.".into());
    }
    let mut out = String::new();
    let mut rest = t;
    if t.contains("://") {
        let (scheme, r) = t.split_once("://").unwrap_or(("", t));
        out.push_str(&format!("Scheme    {scheme}\n"));
        let (r, frag) = r.split_once('#').map_or((r, ""), |(a, b)| (a, b));
        let (r, query) = r.split_once('?').map_or((r, ""), |(a, b)| (a, b));
        let (auth, path) = r.find('/').map_or((r, ""), |p| (&r[..p], &r[p..]));
        let (user, hostport) = auth.rsplit_once('@').map_or(("", auth), |(u, h)| (u, h));
        if !user.is_empty() {
            out.push_str(&format!("User      {}\n", user.split(':').next().unwrap_or("")));
        }
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.ends_with(']') || (hostport.starts_with('[') && h.ends_with(']')) => (h, p),
            _ => (hostport, ""),
        };
        out.push_str(&format!("Host      {host}\n"));
        if !port.is_empty() {
            out.push_str(&format!("Port      {port}\n"));
        }
        out.push_str(&format!("Path      {}\n", if path.is_empty() { "/" } else { path }));
        if !frag.is_empty() {
            out.push_str(&format!("Fragment  {frag}\n"));
        }
        rest = query;
        if rest.is_empty() {
            out.push_str("Query     (none)\n");
        }
    } else {
        rest = rest.strip_prefix('?').unwrap_or(rest);
    }
    if !rest.is_empty() {
        out.push_str("Query parameters:\n");
        for (n, pair) in rest.split('&').filter(|p| !p.is_empty()).enumerate() {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            out.push_str(&format!("  {}. {} = {}\n", n + 1, decode(k, true)?, decode(v, true)?));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes() {
        assert_eq!(encode("a b&c=d/é~"), "a%20b%26c%3Dd%2F%C3%A9~");
        assert_eq!(decode("a%20b%26c%3Dd%2F%C3%A9~", false).unwrap(), "a b&c=d/é~");
        assert_eq!(decode("a+b", true).unwrap(), "a b");
        assert_eq!(decode("a+b", false).unwrap(), "a+b");
        assert!(decode("100%", true).unwrap_err().contains("Bad escape"));
        assert!(decode("%zz", true).is_err());
    }

    #[test]
    fn a_url_is_split_and_its_query_listed() {
        let t = parse("https://user:pw@example.com:8443/a/b?q=hello+world&x=%C3%A9&flag#top").unwrap();
        for want in ["Scheme    https", "User      user", "Host      example.com", "Port      8443", "Path      /a/b", "Fragment  top", "1. q = hello world", "2. x = é", "3. flag = "] {
            assert!(t.contains(want), "{want} in\n{t}");
        }
        assert!(!t.contains("pw"), "the password is not repeated");
        assert!(parse("?a=1&b=2").unwrap().contains("2. b = 2"));
        assert!(parse("http://h.test").unwrap().contains("Query     (none)"));
        assert!(parse("  ").is_err());
    }
}

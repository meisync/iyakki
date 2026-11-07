//! Model-provider failures in plain words. The providers' own error text (`HTTP 404: {"detail":"Function '…': Not found for
//! account '…'"}`) means nothing to most people and carries ids that should not be on a screen; [`plain_error`] turns the common
//! ones into a sentence and a hint, and keeps the technical text on a `Details:` line with the ids, keys and numbers masked.
//! The run still sees the raw text where it needs it (the "model has no tools" check); only what the person reads is changed.

/// `raw` is the error a provider client returned (`HTTP <status>: <message>`, or a transport error); `provider` is its name from
/// the settings (empty when not known). Text that is not a known failure comes back as it was, with ids masked.
pub fn plain_error(provider: &str, raw: &str) -> String {
    let who = if provider.trim().is_empty() { "The model provider".to_string() } else { provider.trim().to_string() };
    let l = raw.to_ascii_lowercase();
    let status: Option<u16> = raw.trim_start().strip_prefix("HTTP ").and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
    let said = match status {
        Some(401 | 403) => format!("{who} refused the key. Check the key in Settings -> Models."),
        Some(404) if l.contains("not found for account") => format!("{who}: the key works, but its account cannot use this model. Make the key at build.nvidia.com (Get API Key, with the Cloud Functions service), then pick a model from the list in Settings -> Models."),
        Some(404) if l.contains("function") || l.contains("model") => format!("{who}: this model is not available for this account, or its name is wrong. Pick a model from the list in Settings -> Models."),
        Some(404) => format!("{who} did not find that address. Check the server address in Settings -> Models."),
        Some(429) => format!("{who} says there were too many requests or the quota is used up. Wait a minute and try again, or pick another model."),
        Some(408 | 504) => format!("{who} took too long to answer. Try again."),
        Some(s) if (500..600).contains(&s) => format!("{who} is having trouble right now. Try again in a little while."),
        _ if l.contains("timed out") || l.contains("timeout") => format!("{who} took too long to answer. Try again, or check the network."),
        _ if ["connection refused", "connection reset", "could not connect", "failed to connect", "dns", "resolve", "unreachable", "no route"].iter().any(|k| l.contains(k)) => {
            format!("Could not reach {}. Check the network and the server address in Settings -> Models.", if provider.trim().is_empty() { "the model provider" } else { provider.trim() })
        }
        _ => return mask_ids(raw),
    };
    let details: String = mask_ids(raw).chars().take(300).collect();
    format!("{said}\nDetails: {details}")
}

/// Hide what must not be shown: UUIDs (function and request ids), key-like tokens, long numbers (account ids) and long mixed
/// letter-digit strings.
pub fn mask_ids(s: &str) -> String {
    fn word(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }
    let mut out = String::new();
    let mut cur = String::new();
    for c in s.chars().chain(std::iter::once('\0')) {
        if word(c) {
            cur.push(c);
            continue;
        }
        out.push_str(&mask_word(&cur));
        cur.clear();
        if c != '\0' {
            out.push(c);
        }
    }
    out
}

fn mask_word(w: &str) -> String {
    let uuid = {
        let p: Vec<&str> = w.split('-').collect();
        p.len() == 5 && [8, 4, 4, 4, 12].iter().zip(&p).all(|(n, x)| x.len() == *n && x.chars().all(|c| c.is_ascii_hexdigit()))
    };
    let key = ["nvapi-", "sk-", "sk_", "ghp_", "gho_", "xoxb-", "AIza", "Bearer"].iter().any(|k| w.starts_with(k) && w.len() > k.len() + 6);
    let number = w.len() >= 6 && w.chars().all(|c| c.is_ascii_digit());
    let blob = w.len() >= 24 && w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_ascii_alphabetic());
    if uuid {
        "<id>".into()
    } else if key {
        "<key>".into()
    } else if number || blob {
        "<id>".into()
    } else {
        w.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FN_ID: &str = "0b1e4a52-7c3d-4f8e-9a6b-123456789abc";
    const NVIDIA_404: &str = r#"HTTP 404: {"status":404,"title":"Not Found","detail":"Function '0b1e4a52-7c3d-4f8e-9a6b-123456789abc': Not found for account 'Ab12Cd34Ef56Gh78Ij90Kl12Mn34'"}"#;

    fn clean(s: &str) {
        assert!(!s.contains(FN_ID) && !s.contains("Ab12Cd34") && !s.contains("nvapi-abcdef"), "{s}");
    }

    #[test]
    fn the_nvidia_account_error_says_the_key_works_but_the_account_cannot_use_the_model() {
        let e = plain_error("nvidia", NVIDIA_404);
        assert!(e.starts_with("nvidia: the key works, but its account cannot use this model"), "{e}");
        assert!(e.contains("build.nvidia.com") && e.contains("Cloud Functions") && e.contains("pick a model"), "{e}");
        assert!(e.contains("Details: HTTP 404") && e.contains("<id>"), "the technical text stays, masked: {e}");
        clean(&e);
    }

    #[test]
    fn a_refused_key_points_at_settings() {
        for s in [401, 403] {
            let e = plain_error("openai", &format!("HTTP {s}: Incorrect API key provided: sk-abcdef1234567890"));
            assert!(e.starts_with("openai refused the key") && e.contains("Settings -> Models"), "{e}");
            assert!(!e.contains("sk-abcdef"), "{e}");
        }
    }

    #[test]
    fn a_missing_model_and_a_wrong_address_are_told_apart() {
        let m = plain_error("gemini", "HTTP 404: models/gemin-9 is not found for API version v1beta (model)");
        assert!(m.contains("model is not available for this account, or its name is wrong"), "{m}");
        let u = plain_error("my-server", "HTTP 404: 404 page not found");
        assert!(u.contains("did not find that address") && u.contains("server address"), "{u}");
    }

    #[test]
    fn rate_limits_and_server_trouble_and_timeouts_and_connections() {
        assert!(plain_error("openrouter", "HTTP 429: slow down").contains("too many requests or the quota"));
        assert!(plain_error("x", "HTTP 503: overloaded").contains("having trouble right now"));
        assert!(plain_error("x", "HTTP 500").contains("having trouble right now"));
        assert!(plain_error("x", "HTTP 504: gateway").contains("took too long"));
        assert!(plain_error("x", "request timed out after 120 s").contains("took too long"));
        let c = plain_error("ollama", "connection refused (os error 111)");
        assert!(c.starts_with("Could not reach ollama") && c.contains("server address"), "{c}");
        assert!(plain_error("", "HTTP 503").starts_with("The model provider is having trouble"));
    }

    #[test]
    fn unknown_text_is_kept_but_ids_and_keys_are_masked() {
        assert_eq!(plain_error("x", "model crashed"), "model crashed");
        assert_eq!(plain_error("x", "HTTP 400: bad request"), "HTTP 400: bad request");
        let m = plain_error("x", &format!("HTTP 400: request {FN_ID} used nvapi-abcdef0123456789 for account 123456789012"));
        assert_eq!(m, "HTTP 400: request <id> used <key> for account <id>");
        assert_eq!(mask_ids("HTTP 404 in 12 ms"), "HTTP 404 in 12 ms", "short numbers stay");
    }
}

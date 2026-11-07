//! Spotting a credential in text a person is about to send to the assistant (an API key, a token, a private key, a
//! password given after `password:`). The chat does not send such a message as written: it offers to put the value in the
//! person's vault and sends a placeholder instead. The detector errs on the side of ordinary words: "my password is hard to
//! remember" is not a credential.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// What it looks like: `API key`, `GitHub token`, `private key`, `password`, …
    pub what: &'static str,
    /// The secret value (kept only in memory, to offer it to the vault form).
    pub value: String,
    /// The text with every credential replaced by [`PLACEHOLDER`].
    pub redacted: String,
}

pub const PLACEHOLDER: &str = "[credential removed - to be stored in the vault]";

fn is_tokch(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '/' | '=')
}

/// Spans (byte start, end) of whitespace-separated words, with surrounding quotes and brackets cut off.
fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut v = vec![];
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                let w = &text[s..i];
                let lead = w.len() - w.trim_start_matches(['"', '\'', '(', '<', '[', '`']).len();
                let trail = w.len() - w.trim_end_matches(['"', '\'', ')', '>', ']', '`', ',', ';', '.']).len();
                if s + lead < i - trail {
                    v.push((s + lead, i - trail));
                }
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    v
}

fn run_len(s: &str, ok: impl Fn(char) -> bool) -> usize {
    s.chars().take_while(|c| ok(*c)).count()
}

/// A prefixed key format: (prefix, what, minimum characters after the prefix, allowed characters).
fn prefixed(w: &str) -> Option<&'static str> {
    let alnum_ = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let rules: &[(&str, &str, usize, fn(char) -> bool)] = &[
        ("sk-", "API key", 20, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        ("ghp_", "GitHub token", 20, |c| c.is_ascii_alphanumeric() || c == '_'),
        ("gho_", "GitHub token", 20, |c| c.is_ascii_alphanumeric() || c == '_'),
        ("ghu_", "GitHub token", 20, |c| c.is_ascii_alphanumeric() || c == '_'),
        ("ghs_", "GitHub token", 20, |c| c.is_ascii_alphanumeric() || c == '_'),
        ("github_pat_", "GitHub token", 20, |c| c.is_ascii_alphanumeric() || c == '_'),
        ("AKIA", "AWS access key", 16, |c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        ("ASIA", "AWS access key", 16, |c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        ("AIza", "Google API key", 30, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        ("xapp-", "Slack token", 10, |c| c.is_ascii_alphanumeric() || c == '-'),
        ("eyJ", "JSON web token", 30, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'),
        // model providers and common services (their own prefixes; the random part must be long, so a word such as "hf_model" is not a key)
        ("nvapi-", "NVIDIA API key", 30, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        ("gsk_", "Groq API key", 30, |c| c.is_ascii_alphanumeric()),
        ("hf_", "Hugging Face token", 30, |c| c.is_ascii_alphanumeric()),
        ("pplx-", "Perplexity API key", 30, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        ("xai-", "xAI API key", 30, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        ("r8_", "Replicate token", 30, |c| c.is_ascii_alphanumeric()),
        ("glpat-", "GitLab token", 20, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        ("ghr_", "GitHub token", 20, |c| c.is_ascii_alphanumeric() || c == '_'),
        ("npm_", "npm token", 30, |c| c.is_ascii_alphanumeric()),
        ("sk_live_", "Stripe key", 16, |c| c.is_ascii_alphanumeric()),
        ("rk_live_", "Stripe key", 16, |c| c.is_ascii_alphanumeric()),
        ("SG.", "SendGrid key", 40, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'),
        ("dop_v1_", "DigitalOcean token", 40, |c| c.is_ascii_hexdigit()),
        ("shpat_", "Shopify token", 30, |c| c.is_ascii_hexdigit()),
        ("ya29.", "Google OAuth token", 40, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'),
    ];
    let _ = alnum_;
    for &(p, what, min, ok) in rules {
        if let Some(rest) = w.strip_prefix(p) {
            if run_len(rest, ok) >= min && (p != "eyJ" || rest.matches('.').count() >= 2) {
                return Some(what);
            }
        }
    }
    if let Some(rest) = ["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-"].iter().find_map(|p| w.strip_prefix(p)) {
        if run_len(rest, |c| c.is_ascii_alphanumeric() || c == '-') >= 10 {
            return Some("Slack token");
        }
    }
    None
}

fn has_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

/// Random-looking: long, one run of token characters, letters and digits mixed (or hex).
fn random_like(v: &str, min: usize) -> bool {
    if v.chars().count() < min || !v.chars().all(is_tokch) || v.starts_with('/') || v.contains("://") {
        return false;
    }
    let hex = v.len() >= 32 && v.chars().all(|c| c.is_ascii_hexdigit());
    let up = v.chars().any(|c| c.is_ascii_uppercase());
    let lo = v.chars().any(|c| c.is_ascii_lowercase());
    hex || (has_digit(v) && (up || lo) && v.chars().filter(|c| c.is_ascii_digit()).count() >= 2)
}

/// A value given explicitly (`password: x`, `token=x`).
fn explicit_value(v: &str) -> bool {
    let n = v.chars().count();
    if n < 6 || v.chars().any(char::is_whitespace) {
        return false;
    }
    let digit = has_digit(v);
    let sym = v.chars().any(|c| !c.is_alphanumeric());
    let mixed = v.chars().any(|c| c.is_uppercase()) && v.chars().any(|c| c.is_lowercase());
    digit || sym || (mixed && n >= 8)
}

const KEYWORDS: &[&str] = &["password", "passwd", "pwd", "passphrase", "secret", "token", "apikey", "api_key", "api-key", "credential", "credentials", "bearer"];

fn keyword_of(w: &str, prev: Option<&str>) -> bool {
    let w = w.trim_end_matches([':', '=']).to_lowercase();
    if KEYWORDS.contains(&w.as_str()) {
        return true;
    }
    w == "key" && matches!(prev.map(str::to_lowercase).as_deref(), Some("api" | "secret" | "access" | "private"))
}

/// Look for a credential in `text`. `None` when it looks like ordinary writing.
pub fn detect(text: &str) -> Option<Found> {
    // a private key block takes everything from its header to its footer
    if let Some(a) = text.find("-----BEGIN ").filter(|a| text[*a..].lines().next().is_some_and(|l| l.contains("PRIVATE KEY"))) {
        let end = text[a..].find("-----END ").and_then(|e| text[a + e + 9..].find("-----").map(|f| a + e + 9 + f + 5)).unwrap_or(text.len());
        let value = text[a..end].to_string();
        let redacted = format!("{}{}{}", &text[..a], PLACEHOLDER, &text[end..]);
        return Some(Found { what: "private key", value, redacted });
    }
    let sp = spans(text);
    let mut hits: Vec<(usize, usize, &'static str)> = vec![];
    for (k, &(s, e)) in sp.iter().enumerate() {
        let w = &text[s..e];
        if let Some(what) = prefixed(w) {
            hits.push((s, e, what));
            continue;
        }
        // user:password@host in an address
        if let Some(i) = w.find("://") {
            let rest = &w[i + 3..];
            if let Some((cred, _)) = rest.split_once('@') {
                if let Some((_, pw)) = cred.split_once(':') {
                    if pw.chars().count() >= 4 {
                        let ps = s + i + 3 + cred.find(':').unwrap_or(0) + 1;
                        hits.push((ps, ps + pw.len(), "password"));
                        continue;
                    }
                }
            }
            continue;
        }
        // a keyword followed by a value: `password: x`, `token = x`, `api key is x`
        let prev = k.checked_sub(1).map(|p| &text[sp[p].0..sp[p].1]);
        if keyword_of(w, prev) || (w.contains('=') || w.contains(':')) && {
            let (name, _) = w.split_once(['=', ':']).unwrap_or((w, ""));
            keyword_of(name, prev)
        } {
            let (mut sep, mut inline) = (w.ends_with(':') || w.ends_with('='), None);
            if let Some((name, val)) = w.split_once(['=', ':']) {
                if !val.is_empty() && KEYWORDS.contains(&name.to_lowercase().as_str()) {
                    inline = Some((s + name.len() + 1, e));
                    sep = true;
                }
            }
            let mut j = k + 1;
            let mut natural = false;
            if inline.is_none() {
                // skip a lone `:`/`=` or the word `is`
                while let Some(&(a, b)) = sp.get(j) {
                    match &text[a..b] {
                        ":" | "=" => sep = true,
                        x if x.eq_ignore_ascii_case("is") => natural = true,
                        _ => break,
                    }
                    j += 1;
                }
            }
            let cand = inline.or_else(|| sp.get(j).copied());
            if let Some((a, b)) = cand {
                let v = &text[a..b];
                if let Some(known) = prefixed(v) {
                    hits.push((a, b, known));
                    continue;
                }
                let ok = if sep && !natural { explicit_value(v) || random_like(v, 16) } else { random_like(v, 16) };
                if ok && !KEYWORDS.contains(&v.to_lowercase().as_str()) {
                    let what = if w.to_lowercase().starts_with("pass") || w.to_lowercase().starts_with("pwd") { "password" } else if w.to_lowercase().contains("key") || prev.is_some_and(|p| p.eq_ignore_ascii_case("api")) { "API key" } else { "token" };
                    hits.push((a, b, what));
                }
            }
            continue;
        }
        // a long hex string or a long mixed-case run is a key even without a label
        if w.len() >= 40 && random_like(w, 40) && w.chars().any(|c| c.is_ascii_uppercase()) == w.chars().any(|c| c.is_ascii_lowercase()) || (w.len() >= 40 && w.chars().all(|c| c.is_ascii_hexdigit())) {
            hits.push((s, e, "secret key"));
        }
    }
    let &(s, e, what) = hits.first()?;
    let mut redacted = String::new();
    let mut at = 0;
    hits.sort();
    hits.dedup();
    for (a, b, _) in &hits {
        if *a >= at {
            redacted.push_str(&text[at..*a]);
            redacted.push_str(PLACEHOLDER);
            at = *b;
        }
    }
    redacted.push_str(&text[at..]);
    Some(Found { what, value: text[s..e].to_string(), redacted })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_key_formats_are_caught_and_redacted() {
        for (t, what) in [
            ("use sk-proj-abcDEF1234567890abcdefGHIJ for the api", "API key"),
            ("here: ghp_abcdefghijklmnopqrstuvwxyz0123456789", "GitHub token"),
            ("token xoxb-1234567890-abcdefghij", "Slack token"),
            ("my aws key AKIAIOSFODNN7EXAMPLE ok", "AWS access key"),
            ("AIzaSyA1234567890abcdefghijklmnopqrstuv", "Google API key"),
            ("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop", "JSON web token"),
        ] {
            let f = detect(t).unwrap_or_else(|| panic!("{t}"));
            assert_eq!(f.what, what, "{t}");
            assert!(f.redacted.contains(PLACEHOLDER) && !f.redacted.contains(&f.value), "{t}: {}", f.redacted);
            assert!(t.contains(&f.value));
        }
        let f = detect("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\nAAAA\n-----END OPENSSH PRIVATE KEY----- thanks").unwrap();
        assert_eq!(f.what, "private key");
        assert_eq!(f.redacted, format!("{PLACEHOLDER} thanks"));
    }

    #[test]
    fn labelled_values_are_caught() {
        let f = detect("password: hunter2!").unwrap();
        assert_eq!((f.what, f.value.as_str()), ("password", "hunter2!"));
        assert_eq!(detect("my login is bob and password=Tr0ub4dor&3").unwrap().value, "Tr0ub4dor&3");
        assert_eq!(detect("api key: 9f8e7d6c5b4a39281706f5e4d3c2b1a0").unwrap().what, "API key");
        assert_eq!(detect("the token is a1b2c3d4e5f6a7b8c9d0").unwrap().what, "token");
        assert_eq!(detect("connect to https://bob:s3cretPw@nas.local/share").unwrap().what, "password");
        assert!(detect("Authorization: Bearer abc123def456ghi789jkl").is_some());
        assert_eq!(detect("0123456789abcdef0123456789abcdef01234567").unwrap().what, "secret key");
    }

    #[test]
    fn ordinary_writing_is_left_alone() {
        for t in [
            "my password is hard to remember",
            "I forgot my password again",
            "how do I reset a password?",
            "the token is valid for an hour",
            "password: secret",
            "what is an api key?",
            "play a kutthu song",
            "set a secret santa reminder",
            "the key to success is practice",
            "open /home/me/Music/Albums/The_Best_Of_The_Year_2024_Collection_Volume_3.mp3",
            "see https://example.com/a/very/long/path/with/many/parts/and/more/parts/here",
            "sk-",
            "bearer of bad news",
            "version 2.4.1-beta3 is out",
            "",
        ] {
            assert_eq!(detect(t), None, "{t}");
        }
    }

    /// Made-up values with the shape of each provider's key (never a real one).
    #[test]
    fn model_provider_and_service_keys_are_found_by_their_prefix_and_their_names_are_not() {
        let rnd = "aB3dE5fG7hJ9kL2mN4pQ6rS8tU0vW1xY3zA5bC7dE9fG1hJ3kL5";
        for (prefix, what) in [
            ("nvapi-", "NVIDIA API key"),
            ("gsk_", "Groq API key"),
            ("hf_", "Hugging Face token"),
            ("pplx-", "Perplexity API key"),
            ("xai-", "xAI API key"),
            ("r8_", "Replicate token"),
            ("glpat-", "GitLab token"),
            ("npm_", "npm token"),
            ("sk_live_", "Stripe key"),
            ("ya29.", "Google OAuth token"),
        ] {
            let key = format!("{prefix}{rnd}");
            let f = detect(&format!("use {key} for the provider")).unwrap_or_else(|| panic!("{prefix} is not found"));
            assert_eq!(f.what, what, "{prefix}");
            assert!(!f.redacted.contains(&key) && f.redacted.contains(PLACEHOLDER), "{prefix}: {}", f.redacted);
            assert_eq!(f.value, key);
        }
        // a key inside the Python snippet people paste
        let snippet = format!("client = OpenAI(\n  base_url = \"https://integrate.api.nvidia.com/v1\",\n  api_key = \"nvapi-{rnd}\"\n)");
        let f = detect(&snippet).expect("the key in a pasted snippet");
        assert!(!f.redacted.contains(rnd), "{}", f.redacted);
        // the prefixes alone, short words and model names are ordinary writing
        for t in ["the hf_model folder", "use nvapi- keys from build.nvidia.com", "gsk_ is the Groq prefix", "xai-grok is a model name", "npm_config", "pick glpat- for GitLab", "sk_live_ keys"] {
            assert_eq!(detect(t), None, "{t}");
        }
    }
}

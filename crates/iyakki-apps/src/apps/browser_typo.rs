//! A typed address whose host does not exist (`youtub.com`): say so plainly and offer the two likely ways on, instead of
//! a raw resolver error. No network is used here; the suggestions come from a short list of popular sites.

use iyakki_web::SearchEngine;

/// Sites people mistype most. Only used to suggest, never to redirect.
const POPULAR: &[&str] = &[
    "youtube.com", "google.com", "gmail.com", "facebook.com", "instagram.com", "twitter.com", "x.com", "wikipedia.org", "amazon.com", "netflix.com", "reddit.com", "linkedin.com", "github.com", "stackoverflow.com", "microsoft.com", "apple.com",
    "bing.com", "yahoo.com", "whatsapp.com", "spotify.com", "twitch.tv", "bbc.com", "cnn.com", "nytimes.com", "duckduckgo.com", "paypal.com", "ebay.com", "flipkart.com", "hotstar.com", "thehindu.com",
];

/// The resolver could not find the host (as opposed to a refused connection, a bad certificate or a timeout).
pub fn is_lookup_failure(e: &str) -> bool {
    let l = e.to_lowercase();
    l.contains("failed to lookup") || l.contains("no usable address") || l.contains("name or service not known") || l.contains("no address associated") || l.contains("nxdomain") || l.contains("could not resolve") || l.contains("temporary failure in name resolution")
}

fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// A popular site the host is a small slip away from (one or two letters; short names allow only one).
pub fn did_you_mean(host: &str) -> Option<&'static str> {
    let h = host.strip_prefix("www.").unwrap_or(host);
    if POPULAR.contains(&h) {
        return None;
    }
    POPULAR.iter().map(|p| (distance(h, p), *p)).filter(|(d, p)| *d <= if p.len() >= 10 { 2 } else { 1 }).min_by_key(|(d, _)| *d).map(|(_, p)| p)
}

/// The page shown in place of the failed one.
pub fn not_found_html(host: &str, engine: SearchEngine) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut h = format!("<title>Couldn't find {0}</title><h1>Couldn't find {0}</h1><p>There is no web site with that name, or this computer cannot reach the name service. Check the spelling and the network.</p>", esc(host));
    if let Some(p) = did_you_mean(host) {
        h.push_str(&format!("<p>Did you mean <a href=\"https://{0}/\">{0}</a>?</p>", p));
    }
    h.push_str(&format!("<p><a href=\"{}\">Search for '{}' instead</a> (on {})</p>", esc(&engine.url(host).to_string()), esc(host), engine.name()));
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_errors_are_told_from_other_failures() {
        assert!(is_lookup_failure("io: failed to lookup address information: Name has no usable address"));
        assert!(is_lookup_failure("dns error: Name or service not known"));
        assert!(!is_lookup_failure("connection refused"));
        assert!(!is_lookup_failure("tls: invalid peer certificate: UnknownIssuer"));
    }

    #[test]
    fn near_misses_of_popular_sites_are_suggested_and_others_are_not() {
        assert_eq!(did_you_mean("youtub.com"), Some("youtube.com"));
        assert_eq!(did_you_mean("www.yutube.com"), Some("youtube.com"));
        assert_eq!(did_you_mean("gogle.com"), Some("google.com"));
        assert_eq!(did_you_mean("wikipedia.og"), Some("wikipedia.org"));
        assert_eq!(did_you_mean("youtube.com"), None, "a real name is never 'corrected'");
        assert_eq!(did_you_mean("my-own-box.local"), None);
        assert_eq!(did_you_mean("bbd.com"), Some("bbc.com"));
        assert_eq!(did_you_mean("xyz.com"), None, "short names tolerate one slip only and this is not one");
    }

    #[test]
    fn the_page_offers_a_search_and_the_suggestion() {
        let h = not_found_html("youtub.com", SearchEngine::Bing);
        assert!(h.contains("Couldn't find youtub.com"));
        assert!(h.contains("Search for 'youtub.com' instead") && h.contains("www.bing.com/search?q=youtub.com"), "{h}");
        assert!(h.contains("Did you mean <a href=\"https://youtube.com/\">youtube.com"));
        let plain = not_found_html("nothing-here.example", SearchEngine::DuckDuckGo);
        assert!(!plain.contains("Did you mean") && plain.contains("html.duckduckgo.com"));
        assert!(!not_found_html("<b>", SearchEngine::Bing).contains("<b>"));
    }
}

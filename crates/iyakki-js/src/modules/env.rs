//! `window`, `location`, `navigator`, `history`, `screen`, `crypto.getRandomValues`, `AbortController`: what a page
//! learns about its surroundings. It learns very little: a fixed browser name, a fixed screen, its own address.

use std::any::Any;

use iyakki_web::Url;

use crate::engine::WebModule;

pub struct EnvModule {
    url: Url,
    /// Where a script asked to go (`location.href = ...`); the browser decides whether to follow.
    pub navigation: Option<String>,
}

impl EnvModule {
    pub fn new(url: Url) -> EnvModule {
        EnvModule { url, navigation: None }
    }
}

/// `n` random bytes from the system.
fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        // no system source: fall back to a time-seeded generator (not suitable for secrets, but never silent zeros)
        let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        for b in buf.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x >> 24) as u8;
        }
    }
    buf
}

impl WebModule for EnvModule {
    fn name(&self) -> &'static str {
        "window"
    }

    fn prelude(&self) -> &'static str {
        include_str!("env.js")
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        match op {
            "location" => {
                let u = &self.url;
                let default = if u.https { 443 } else { 80 };
                let port = u.port.filter(|p| *p != default).map(|p| p.to_string()).unwrap_or_default();
                let host = if port.is_empty() { u.host.clone() } else { format!("{}:{port}", u.host) };
                Ok(serde_json::json!({
                    "href": u.to_string(),
                    "protocol": if u.https { "https:" } else { "http:" },
                    "host": host,
                    "hostname": u.host,
                    "port": port,
                    "pathname": u.path,
                    "search": u.query.as_ref().map_or(String::new(), |q| if q.is_empty() { String::new() } else { format!("?{q}") }),
                    "hash": u.fragment.as_ref().map_or(String::new(), |f| if f.is_empty() { String::new() } else { format!("#{f}") }),
                    "origin": format!("{}://{host}", if u.https { "https" } else { "http" }),
                }))
            }
            "navigate" => {
                let target = self.url.join(args["url"].as_str().unwrap_or(""))?;
                self.navigation = Some(target.to_string());
                Ok(serde_json::Value::Null)
            }
            "random" => {
                let n = args["n"].as_u64().unwrap_or(0).min(65_536) as usize;
                Ok(serde_json::json!(random_bytes(n)))
            }
            other => Err(format!("window has no operation {other}")),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{ConsoleModule, TimersModule, UrlModule};
    use crate::{JsEngine, Limits};

    fn engine() -> JsEngine {
        JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new()), Box::new(TimersModule), Box::new(UrlModule), Box::new(EnvModule::new(Url::parse("https://example.org:8443/dir/page.html?x=1&y=2#sec").unwrap()))]).unwrap()
    }

    #[test]
    fn the_page_knows_its_own_address_and_little_else() {
        let mut e = engine();
        assert_eq!(e.eval("t", "[location.href, location.origin, location.host, location.hostname, location.port, location.pathname, location.search, location.hash, location.protocol].join(' ')").unwrap(), "https://example.org:8443/dir/page.html?x=1&y=2#sec https://example.org:8443 example.org:8443 example.org 8443 /dir/page.html ?x=1&y=2 #sec https:");
        assert_eq!(e.eval("t", "[window === globalThis, self === window, window.top === window, typeof navigator.userAgent, navigator.webdriver, innerWidth, isSecureContext].join()").unwrap(), "true,true,true,string,false,1280,true");
        assert!(e.eval("t", "navigator.userAgent").unwrap().contains("Iyakki"));
    }

    #[test]
    fn navigation_requests_are_recorded_for_the_browser_to_decide() {
        let mut e = engine();
        e.eval("t", "location.href = '/next'; ").unwrap();
        assert_eq!(e.module::<EnvModule>("window").unwrap().navigation.as_deref(), Some("https://example.org:8443/next"));
        e.eval("t", "location.assign('https://other.org/x')").unwrap();
        assert_eq!(e.module::<EnvModule>("window").unwrap().navigation.as_deref(), Some("https://other.org/x"));
        assert!(e.eval("t", "location.assign('javascript:alert(1)')").is_err(), "unusable targets are errors");
        e.eval("t", "history.pushState({ a: 1 }, '', '/pushed?z=9')").unwrap();
        assert_eq!(e.eval("t", "location.pathname + location.search + JSON.stringify(history.state)").unwrap(), "/pushed?z=9{\"a\":1}", "pushState changes what the page sees without navigating");
    }

    #[test]
    fn random_values_are_filled_and_uuids_are_well_formed() {
        let mut e = engine();
        assert_eq!(e.eval("t", "const a = crypto.getRandomValues(new Uint8Array(32)); a.length + ',' + (a.some((x) => x !== 0))").unwrap(), "32,true");
        let u = e.eval("t", "crypto.randomUUID()").unwrap();
        assert_eq!(u.len(), 36);
        assert_eq!(u.as_bytes()[14], b'4', "version 4");
        assert_ne!(u, e.eval("t", "crypto.randomUUID()").unwrap());
    }

    #[test]
    fn abort_controllers_work_and_match_media_says_no() {
        let mut e = engine();
        assert_eq!(e.eval("t", "const c = new AbortController(); let n = 0; c.signal.addEventListener('abort', () => n++); c.abort(); c.abort(); [c.signal.aborted, n, matchMedia('(min-width: 1px)').matches, typeof structuredClone({ a: [1] }).a].join()").unwrap(), "true,1,false,object");
    }
}

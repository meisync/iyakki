//! A transport wrapper that adds the person's custom request headers to the requests they match. Which headers go to which address
//! is decided by a function the caller gives (the rules live in the agent crate); this wrapper only asks it for each request, so
//! every redirect hop is judged again and a header never follows a redirect to a site it was not meant for.

use std::sync::Arc;

use crate::fetch::{ExtraHeaders, Fetch, Request, Response};
use crate::url::Url;

/// Given an address, the (name, value) pairs to send to it (empty for none).
pub type Provider = Arc<dyn Fn(&Url) -> Vec<(String, String)> + Send + Sync>;

pub struct HeaderFetch {
    inner: Arc<dyn Fetch>,
    provider: Provider,
}

impl HeaderFetch {
    pub fn new(inner: Arc<dyn Fetch>, provider: Provider) -> HeaderFetch {
        HeaderFetch { inner, provider }
    }
}

impl Fetch for HeaderFetch {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn for_user(&self, user: &str) -> Option<Arc<dyn Fetch>> {
        self.inner.for_user(user).map(|i| Arc::new(HeaderFetch { inner: i, provider: self.provider.clone() }) as Arc<dyn Fetch>)
    }

    fn cache_stats(&self, user: &str) -> Option<crate::cache::CacheStats> {
        self.inner.cache_stats(user)
    }

    fn clear_cache(&self, user: &str) {
        self.inner.clear_cache(user)
    }

    fn set_cache_budget(&self, b: crate::net::Budget) {
        self.inner.set_cache_budget(b)
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        let extra = (self.provider)(&req.url);
        if extra.is_empty() {
            return self.inner.send(req);
        }
        let mut r = req.clone();
        r.extra = ExtraHeaders(extra);
        self.inner.send(&r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Rec(Mutex<Vec<(String, Vec<(String, String)>)>>);
    impl Fetch for Rec {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.send(&Request::get(url))
        }
        fn send(&self, req: &Request) -> Result<Response, String> {
            self.0.lock().unwrap().push((req.url.to_string(), req.extra.0.clone()));
            Ok(Response { status: 200, ..Default::default() })
        }
    }

    #[test]
    fn matching_requests_get_the_headers_and_others_do_not() {
        let rec = Arc::new(Rec(Mutex::new(vec![])));
        let p: Provider = Arc::new(|u: &Url| if u.host == "api.example.test" { vec![("X-Test".into(), "v1".into())] } else { vec![] });
        let f = HeaderFetch::new(rec.clone(), p);
        f.get(&Url::parse("https://api.example.test/a").unwrap()).unwrap();
        f.get(&Url::parse("https://other.example.test/a").unwrap()).unwrap();
        let seen = rec.0.lock().unwrap();
        assert_eq!(seen[0].1, vec![("X-Test".to_string(), "v1".to_string())]);
        assert!(seen[1].1.is_empty());
    }

    #[test]
    fn debug_text_never_shows_a_value() {
        let r = Request { extra: ExtraHeaders(vec![("Authorization".into(), "Bearer not-a-real-token".into())]), ..Default::default() };
        let text = format!("{r:?}");
        assert!(text.contains("Authorization") && !text.contains("not-a-real-token"), "{text}");
    }
}

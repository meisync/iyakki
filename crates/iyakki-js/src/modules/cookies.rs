//! `document.cookie`. The jar is the browser's own ([`iyakki_web::CookieStore`]), shared with the requests the browser
//! makes, so a cookie a server set is visible to scripts (unless `HttpOnly`) and a cookie a script sets goes out with
//! the next request. Where the jar is kept (memory, the user's vault) is the embedder's business.

use std::any::Any;
use std::sync::{Arc, Mutex};

use iyakki_web::{CookieStore, Url};

use crate::engine::WebModule;

pub struct CookiesModule {
    store: Arc<Mutex<CookieStore>>,
    url: Url,
    /// Seconds since the epoch, fixed at the start of the page so a script's view is stable.
    now: i64,
}

impl CookiesModule {
    pub fn new(store: Arc<Mutex<CookieStore>>, url: Url, now: i64) -> CookiesModule {
        CookiesModule { store, url, now }
    }
}

impl WebModule for CookiesModule {
    fn name(&self) -> &'static str {
        "cookies"
    }

    fn prelude(&self) -> &'static str {
        r#"
globalThis.__cookies = {
  get: () => JSON.parse(__host('cookies', 'get', '{}')),
  set: (s) => { __host('cookies', 'set', JSON.stringify({ s: String(s) })); },
};
"#
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut store = self.store.lock().map_err(|_| "cookies are unavailable".to_string())?;
        match op {
            "get" => Ok(serde_json::Value::String(store.script_view(&self.url, self.now))),
            "set" => {
                store.store_from_script(&self.url, args["s"].as_str().unwrap_or(""), self.now);
                Ok(serde_json::Value::Null)
            }
            other => Err(format!("cookies has no operation {other}")),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsEngine, Limits};

    fn engine(store: Arc<Mutex<CookieStore>>, url: &str) -> JsEngine {
        JsEngine::new(Limits::default(), vec![Box::new(CookiesModule::new(store, Url::parse(url).unwrap(), 5000))]).unwrap()
    }

    #[test]
    fn scripts_read_and_write_the_browsers_jar() {
        let store = Arc::new(Mutex::new(CookieStore::new()));
        let page = Url::parse("https://a.org/app/").unwrap();
        store.lock().unwrap().store_from_header(&page, "session=secret; HttpOnly", 5000);
        store.lock().unwrap().store_from_header(&page, "theme=dark", 5000);
        let mut e = engine(store.clone(), "https://a.org/app/");
        assert_eq!(e.eval("t", "__cookies.get()").unwrap(), "theme=dark", "HttpOnly cookies are invisible to scripts");
        e.eval("t", "__cookies.set('visit=1; Max-Age=3600'); __cookies.set('theme=light')").unwrap();
        assert_eq!(e.eval("t", "__cookies.get()").unwrap(), "theme=light; visit=1");
        // the server-facing view has everything, including what the script set
        assert_eq!(store.lock().unwrap().header_for(&page, 5000), "session=secret; theme=light; visit=1");
        e.eval("t", "__cookies.set('session=hijack; HttpOnly')").unwrap();
        assert_eq!(store.lock().unwrap().header_for(&page, 5000), "session=secret; theme=light; visit=1", "a script cannot overwrite an HttpOnly cookie by setting one");
    }

    #[test]
    fn a_page_sees_only_its_own_sites_cookies() {
        let store = Arc::new(Mutex::new(CookieStore::new()));
        store.lock().unwrap().store_from_header(&Url::parse("https://other.org/").unwrap(), "o=1", 5000);
        let mut e = engine(store, "https://a.org/");
        e.eval("t", "__cookies.set('mine=1')").unwrap();
        assert_eq!(e.eval("t", "__cookies.get()").unwrap(), "mine=1");
    }
}

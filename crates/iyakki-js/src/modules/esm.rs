//! ES modules: the engine asks this plug-in to turn an import specifier into an address and to fetch a module's source.
//! Every address is checked against the host policy of the account that opened the page, the number and size of modules
//! is capped, and a module is fetched once. A page's `<script type="importmap">` maps bare names.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use iyakki_web::{Fetch, Url};

use crate::engine::WebModule;

const MAX_MODULES: usize = 1200;
const MAX_MODULE_BYTES: usize = 2 << 20;
const MAX_TOTAL: usize = 24 << 20;

pub struct EsmModule {
    web: Arc<dyn Fetch>,
    allow: Box<dyn Fn(&str) -> bool + Send>,
    /// `imports` of the page's import map, longest key first.
    imports: Vec<(String, String)>,
    base: Url,
    cache: HashMap<String, String>,
    total: usize,
    /// Every module address fetched, for the developer view.
    pub log: Vec<String>,
}

impl EsmModule {
    pub fn new(base: Url, web: Arc<dyn Fetch>, allow: Box<dyn Fn(&str) -> bool + Send>, import_map: Option<&str>) -> EsmModule {
        let mut imports: Vec<(String, String)> = Vec::new();
        if let Some(text) = import_map.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) {
            if let Some(m) = text["imports"].as_object() {
                for (k, v) in m {
                    if let Some(v) = v.as_str() {
                        // map values are addresses relative to the page
                        if let Ok(u) = base.join(v) {
                            imports.push((k.clone(), u.to_string()));
                        }
                    }
                }
            }
        }
        imports.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        EsmModule { web, allow, imports, base, cache: HashMap::new(), total: 0, log: Vec::new() }
    }

    fn resolve(&self, base: &str, name: &str) -> Result<String, String> {
        let name = name.trim();
        let relative = name.starts_with("./") || name.starts_with("../") || name.starts_with('/');
        if !relative && !name.contains("://") {
            // a bare name: the import map decides
            for (k, v) in &self.imports {
                if name == k {
                    return Ok(v.clone());
                }
                if k.ends_with('/') && name.starts_with(k.as_str()) {
                    return Ok(format!("{}{}", v, &name[k.len()..]));
                }
            }
            return Err(format!("Cannot resolve the module '{name}': there is no import map entry for it"));
        }
        // the importer's address, without the marker that tells inline modules apart
        let base_url = if base.is_empty() { self.base.clone() } else { Url::parse(base.split('#').next().unwrap_or(base)).unwrap_or_else(|_| self.base.clone()) };
        let u = base_url.join(name).map_err(|e| format!("Cannot resolve the module '{name}': {e}"))?;
        Ok(u.without_fragment().to_string())
    }

    fn load(&mut self, name: &str) -> Result<String, String> {
        if let Some(s) = self.cache.get(name) {
            return Ok(s.clone());
        }
        if self.cache.len() >= MAX_MODULES {
            return Err("this page loads too many modules".into());
        }
        let mut url = Url::parse(name.split('#').next().unwrap_or(name))?;
        for _ in 0..5 {
            if !(self.allow)(&url.host) {
                return Err(format!("your account may not open pages from {}", url.host));
            }
            self.log.push(url.to_string());
            let r = self.web.get(&url)?;
            if (300..400).contains(&r.status) {
                url = url.join(r.location.as_deref().ok_or("a redirect with nowhere to go")?)?.without_fragment();
                continue;
            }
            if r.status >= 400 {
                return Err(format!("the server answered {} for {}", r.status, url));
            }
            if r.body.len() > MAX_MODULE_BYTES || self.total + r.body.len() > MAX_TOTAL {
                return Err(format!("the module {url} is too large"));
            }
            let text = String::from_utf8_lossy(&r.body).into_owned();
            self.total += text.len();
            self.cache.insert(name.to_string(), text.clone());
            return Ok(text);
        }
        Err("too many redirects".into())
    }
}

impl WebModule for EsmModule {
    fn name(&self) -> &'static str {
        "module"
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        match op {
            "resolve" => Ok(serde_json::Value::String(self.resolve(args["base"].as_str().unwrap_or(""), args["name"].as_str().ok_or("no name")?)?)),
            "load" => Ok(serde_json::Value::String(self.load(args["name"].as_str().ok_or("no name")?)?)),
            _ => Err(format!("module: unknown operation {op}")),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

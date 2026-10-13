//! `document`, elements, text, events: the page object. The tree lives here, on the host side, so what scripts do to the
//! page is what the browser shows afterwards ([`DomModule::into_tree`]). The JavaScript half is in `dom.js`.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iyakki_web::domtree::{parse_selectors, Selector};
use iyakki_web::html::{NodeKind, Tree};

use crate::engine::WebModule;

pub struct DomModule {
    pub tree: Tree,
    /// Scripts that have been run (or are being run), so none runs twice.
    claimed: HashSet<usize>,
    /// Parsed selectors by their text: pages ask the same few over and over.
    selectors: HashMap<String, Arc<Vec<Selector>>>,
    /// Elements by their `id` attribute (all of them, attached or not), built on the first lookup by id and kept
    /// current from then on: nodes only ever get added at the end, and ids change only through `setattr`/`rmattr`.
    ids: HashMap<String, Vec<usize>>,
    ids_built: bool,
    ids_upto: usize,
}

/// A `<script>` in the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub node: usize,
    pub src: Option<String>,
    pub text: String,
    /// The `type` attribute, lowercased (empty when absent).
    pub kind: String,
    /// `nomodule`: for browsers that cannot run modules, so this one skips it.
    pub nomodule: bool,
}

impl Script {
    /// Classic JavaScript (not a module, not JSON or a template).
    pub fn is_classic_js(&self) -> bool {
        matches!(self.kind.as_str(), "" | "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "text/x-javascript" | "application/x-javascript")
    }
}

const HEAD_TAGS: [&str; 7] = ["title", "meta", "link", "base", "style", "script", "noscript"];

/// Browsers always have `html`, `head` and `body`; a fragment gets them here so `document.body` is never missing.
pub fn ensure_skeleton(t: &mut Tree) {
    let root = 0;
    let html = match t.nodes[root].children.iter().copied().find(|&c| t.tag(c) == Some("html")) {
        Some(h) => h,
        None => {
            let Ok(h) = t.new_element("html") else { return };
            for c in std::mem::take(&mut t.nodes[root].children) {
                t.nodes[c].parent = iyakki_web::html::NONE;
                let _ = t.append(h, c);
            }
            let _ = t.append(root, h);
            h
        }
    };
    let find = |t: &Tree, tag: &str| t.nodes[html].children.iter().copied().find(|&c| t.tag(c) == Some(tag));
    let head = find(t, "head").or_else(|| {
        let h = t.new_element("head").ok()?;
        let first = t.nodes[html].children.first().copied();
        t.insert(html, h, first).ok()?;
        Some(h)
    });
    if find(t, "body").is_none() {
        let Ok(body) = t.new_element("body") else { return };
        let mut in_head = true;
        for c in t.nodes[html].children.clone() {
            if Some(c) == head {
                continue;
            }
            let head_like = t.tag(c).is_some_and(|x| HEAD_TAGS.contains(&x)) || matches!(&t.nodes[c].kind, NodeKind::Text(s) if s.trim().is_empty());
            if in_head && head_like {
                if let Some(h) = head {
                    let _ = t.append(h, c);
                }
            } else {
                in_head = false;
                let _ = t.append(body, c);
            }
        }
        let _ = t.append(html, body);
    }
}

impl DomModule {
    pub fn new(mut tree: Tree) -> DomModule {
        ensure_skeleton(&mut tree);
        DomModule { tree, claimed: HashSet::new(), selectors: HashMap::new(), ids: HashMap::new(), ids_built: false, ids_upto: 0 }
    }

    pub fn into_tree(self) -> Tree {
        self.tree
    }

    /// Scripts in the page that have not run yet, in document order.
    pub fn pending_scripts(&self) -> Vec<Script> {
        self.tree
            .descendants(0)
            .into_iter()
            .filter(|&n| self.tree.tag(n) == Some("script") && !self.claimed.contains(&n))
            .map(|n| Script {
                node: n,
                src: self.tree.attr(n, "src").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
                text: self.tree.text_of(n),
                kind: self.tree.attr(n, "type").unwrap_or("").trim().to_ascii_lowercase(),
                nomodule: self.tree.attr(n, "nomodule").is_some(),
            })
            .collect()
    }

    /// Mark a script as run; false if it already was.
    pub fn claim(&mut self, n: usize) -> bool {
        self.claimed.insert(n)
    }

    fn id(args: &serde_json::Value, key: &str) -> Result<usize, String> {
        args[key].as_u64().map(|n| n as usize).ok_or_else(|| format!("missing node {key}"))
    }

    fn node(&self, n: usize) -> Result<usize, String> {
        if n < self.tree.nodes.len() { Ok(n) } else { Err("NotFoundError: no such node".into()) }
    }

    /// The parsed form of a selector list (kept for next time).
    fn selectors(&mut self, sel: &str) -> Result<Arc<Vec<Selector>>, String> {
        if let Some(l) = self.selectors.get(sel) {
            return Ok(l.clone());
        }
        let list = Arc::new(parse_selectors(sel)?);
        if self.selectors.len() >= 1024 {
            self.selectors.clear();
        }
        self.selectors.insert(sel.to_string(), list.clone());
        Ok(list)
    }

    /// Add the nodes created since the index was last looked at.
    fn sync_ids(&mut self) {
        self.ids_built = true;
        for n in self.ids_upto..self.tree.nodes.len() {
            if let Some(id) = self.tree.attr(n, "id").filter(|i| !i.is_empty()) {
                self.ids.entry(id.to_string()).or_default().push(n);
            }
        }
        self.ids_upto = self.tree.nodes.len();
    }

    fn unindex_id(&mut self, n: usize) {
        if !self.ids_built || n >= self.ids_upto {
            return;
        }
        if let Some(id) = self.tree.attr(n, "id").map(str::to_string) {
            if let Some(v) = self.ids.get_mut(&id) {
                v.retain(|&x| x != n);
                if v.is_empty() {
                    self.ids.remove(&id);
                }
            }
        }
    }

    fn index_id(&mut self, n: usize) {
        if !self.ids_built || n >= self.ids_upto {
            return;
        }
        if let Some(id) = self.tree.attr(n, "id").filter(|i| !i.is_empty()).map(str::to_string) {
            self.ids.entry(id).or_default().push(n);
        }
    }

    /// The first element in the page (in document order) with this id.
    fn element_by_id(&mut self, id: &str) -> Option<usize> {
        if id.is_empty() {
            return None;
        }
        self.sync_ids();
        let mut live = self.ids.get(id)?.iter().copied().filter(|&n| self.tree.is_connected(n));
        let first = live.next()?;
        if live.next().is_none() { Some(first) } else { self.tree.get_element_by_id(0, id) }
    }

    /// `[id, tag]` for a node ("" for text), the form scripts get nodes in.
    fn pair(&self, n: usize) -> serde_json::Value {
        serde_json::json!([n, self.tag_of(n)])
    }

    fn tag_of(&self, n: usize) -> &str {
        match &self.tree.nodes[n].kind {
            NodeKind::Text(_) => "",
            NodeKind::Element { tag, .. } => tag,
        }
    }

    /// `[id, tag, id, tag, ...]` for a list of nodes.
    fn flat(&self, ids: &[usize]) -> serde_json::Value {
        let mut v = Vec::with_capacity(ids.len() * 2);
        for &n in ids {
            v.push(serde_json::Value::from(n));
            v.push(serde_json::Value::from(self.tag_of(n)));
        }
        serde_json::Value::Array(v)
    }

    fn opt(v: &serde_json::Value) -> Option<usize> {
        v.as_u64().map(|n| n as usize)
    }
}

impl WebModule for DomModule {
    fn name(&self) -> &'static str {
        "dom"
    }

    fn prelude(&self) -> &'static str {
        include_str!("dom.js")
    }

    fn call(&mut self, op: &str, a: &serde_json::Value) -> Result<serde_json::Value, String> {
        use serde_json::json;
        let t = &mut self.tree;
        Ok(match op {
            "doc" => {
                let html = t.nodes[0].children.iter().copied().find(|&c| t.tag(c) == Some("html"));
                let child = |tag: &str| html.and_then(|h| t.nodes[h].children.iter().copied().find(|&c| t.tag(c) == Some(tag)));
                json!({ "html": html, "head": child("head"), "body": child("body") })
            }
            "info" => {
                let n = self.node(Self::id(a, "n")?)?;
                match &self.tree.nodes[n].kind {
                    NodeKind::Text(_) => json!({ "t": 3, "name": "#text" }),
                    NodeKind::Element { tag, .. } if n == 0 => json!({ "t": 9, "name": tag }),
                    NodeKind::Element { tag, .. } => json!({ "t": 1, "name": tag }),
                }
            }
            "kids" => {
                let n = self.node(Self::id(a, "n")?)?;
                self.flat(&self.tree.nodes[n].children)
            }
            "elkids" => {
                let n = self.node(Self::id(a, "n")?)?;
                let ids: Vec<usize> = self.tree.nodes[n].children.iter().copied().filter(|&c| self.tree.is_element(c)).collect();
                self.flat(&ids)
            }
            "parent" => {
                let n = self.node(Self::id(a, "n")?)?;
                match self.tree.parent_of(n) {
                    Some(p) => self.pair(p),
                    None => serde_json::Value::Null,
                }
            }
            "connected" => {
                let n = self.node(Self::id(a, "n")?)?;
                json!(self.tree.is_connected(n))
            }
            "contains" => {
                let (x, y) = (self.node(Self::id(a, "n")?)?, self.node(Self::id(a, "o")?)?);
                json!(self.tree.contains_node(x, y))
            }
            "sibling" => {
                let n = self.node(Self::id(a, "n")?)?;
                match self.tree.element_sibling(n, a["d"].as_i64().unwrap_or(1) as i32) {
                    Some(s) => self.pair(s),
                    None => serde_json::Value::Null,
                }
            }
            "sib" => {
                let n = self.node(Self::id(a, "n")?)?;
                match self.tree.node_sibling(n, a["d"].as_i64().unwrap_or(1) as i32) {
                    Some(s) => self.pair(s),
                    None => serde_json::Value::Null,
                }
            }
            "create" => {
                let tag = a["tag"].as_str().unwrap_or("");
                json!(self.tree.new_element(tag)?)
            }
            "text" => json!(self.tree.new_text(a["text"].as_str().unwrap_or(""))?),
            "clone" => {
                let n = self.node(Self::id(a, "n")?)?;
                let c = self.tree.clone_node(n, a["deep"].as_bool().unwrap_or(false))?;
                self.pair(c)
            }
            "insert" => {
                let (p, c) = (self.node(Self::id(a, "p")?)?, self.node(Self::id(a, "c")?)?);
                let r = Self::opt(&a["r"]);
                self.tree.insert(p, c, r)?;
                // what the page does next: nothing if the node is not in the page, else which scripts it brought along
                if self.tree.is_connected(c) {
                    let scripts: Vec<usize> = self.tree.scripts_under(c).into_iter().filter(|s| !self.claimed.contains(s)).collect();
                    json!(scripts)
                } else {
                    serde_json::Value::Null
                }
            }
            "remove" => {
                let c = self.node(Self::id(a, "c")?)?;
                self.tree.detach(c);
                serde_json::Value::Null
            }
            "attrs" => {
                let n = self.node(Self::id(a, "n")?)?;
                json!(self.tree.attr_list(n))
            }
            "getattr" => {
                let n = self.node(Self::id(a, "n")?)?;
                let k = a["k"].as_str().unwrap_or("").to_ascii_lowercase();
                json!(self.tree.attr(n, &k))
            }
            "setattr" => {
                let n = self.node(Self::id(a, "n")?)?;
                let k = a["k"].as_str().unwrap_or("");
                let is_id = k.eq_ignore_ascii_case("id");
                if is_id && self.ids_built {
                    self.sync_ids();
                    self.unindex_id(n);
                }
                self.tree.set_attr(n, k, a["v"].as_str().unwrap_or(""));
                if is_id {
                    self.index_id(n);
                }
                serde_json::Value::Null
            }
            "rmattr" => {
                let n = self.node(Self::id(a, "n")?)?;
                let k = a["k"].as_str().unwrap_or("");
                let is_id = k.eq_ignore_ascii_case("id");
                if is_id && self.ids_built {
                    self.sync_ids();
                    self.unindex_id(n);
                }
                self.tree.remove_attr(n, k);
                serde_json::Value::Null
            }
            "text_get" => {
                let n = self.node(Self::id(a, "n")?)?;
                json!(self.tree.text_of(n))
            }
            "text_set" => {
                let n = self.node(Self::id(a, "n")?)?;
                self.tree.set_text_content(n, a["text"].as_str().unwrap_or(""))?;
                serde_json::Value::Null
            }
            "html_get" => {
                let n = self.node(Self::id(a, "n")?)?;
                json!(if a["outer"].as_bool().unwrap_or(false) { self.tree.outer_html(n) } else { self.tree.inner_html(n) })
            }
            "html_set" => {
                let n = self.node(Self::id(a, "n")?)?;
                self.tree.set_inner_html(n, a["html"].as_str().unwrap_or(""))?;
                serde_json::Value::Null
            }
            "query" => {
                let n = self.node(Self::id(a, "n")?)?;
                let list = self.selectors(a["sel"].as_str().unwrap_or(""))?;
                let first = !a["all"].as_bool().unwrap_or(true);
                // `#name` alone is a lookup, not a scan
                let found = match (list.as_slice(), list.first().and_then(|s| s.only_id())) {
                    ([_], Some(id)) => {
                        let id = id.to_string();
                        match self.element_by_id(&id) {
                            // the first match in document order; below the node asked it is the first there too
                            Some(e) if n == 0 || (e != n && self.tree.contains_node(n, e)) => vec![e],
                            None if n == 0 => vec![],
                            // a duplicate id, or a node that is not in the page: look under it
                            _ => self.tree.query_selectors(n, &list, first),
                        }
                    }
                    _ => self.tree.query_selectors(n, &list, first),
                };
                self.flat(&found)
            }
            "closest" => {
                let n = self.node(Self::id(a, "n")?)?;
                let list = self.selectors(a["sel"].as_str().unwrap_or(""))?;
                match self.tree.closest(n, &list) {
                    Some(e) => self.pair(e),
                    None => serde_json::Value::Null,
                }
            }
            "matches" => {
                let n = self.node(Self::id(a, "n")?)?;
                let list = self.selectors(a["sel"].as_str().unwrap_or(""))?;
                json!(list.iter().any(|s| self.tree.selector_matches(n, s)))
            }
            "dashed" => {
                let n = self.node(Self::id(a, "n")?)?;
                self.flat(&self.tree.dashed_descendants(n))
            }
            "scripts" => {
                let n = self.node(Self::id(a, "n")?)?;
                json!(self.tree.scripts_under(n))
            }
            "by_id" => match self.element_by_id(a["id"].as_str().unwrap_or("")) {
                Some(e) => self.pair(e),
                None => serde_json::Value::Null,
            },
            "claim_script" => {
                let n = self.node(Self::id(a, "n")?)?;
                json!(self.claim(n))
            }
            other => return Err(format!("dom has no operation {other}")),
        })
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{ConsoleModule, EncodingModule, TimersModule, UrlModule};
    use crate::{modules::run_timers, JsEngine, Limits};
    use iyakki_web::html;

    fn page(markup: &str) -> JsEngine {
        let mut mods: Vec<Box<dyn WebModule>> = vec![Box::new(ConsoleModule::new()), Box::new(TimersModule), Box::new(EncodingModule), Box::new(UrlModule)];
        mods.push(Box::new(super::super::env::EnvModule::new(iyakki_web::Url::parse("https://a.org/dir/page.html?q=1#h").unwrap())));
        mods.push(Box::new(DomModule::new(html::parse(markup))));
        JsEngine::new(Limits::default(), mods).unwrap()
    }

    fn run(e: &mut JsEngine, src: &str) -> String {
        e.eval("t", src).unwrap_or_else(|err| format!("ERROR {err}"))
    }

    fn logged(e: &mut JsEngine) -> Vec<String> {
        e.module::<ConsoleModule>("console").unwrap().lines.iter().map(|l| l.text.clone()).collect()
    }

    const PAGE: &str = r#"<!doctype html><html><head><title> My  Page </title></head><body class="home dark"><h1 id="h">Hello</h1><ul id="l"><li class="i">one</li><li class="i sel">two</li></ul><a id="a" href="/next?x=1" data-user-id="42">go</a><p id="p">text <b>bold</b></p></body></html>"#;

    #[test]
    fn the_document_has_its_usual_parts() {
        let mut e = page(PAGE);
        assert_eq!(run(&mut e, "[document.title, document.body.className, document.documentElement.tagName, document.head.tagName, document.readyState, document.nodeType].join('|')"), "My Page|home dark|HTML|HEAD|loading|9");
        assert_eq!(run(&mut e, "document.getElementById('h').textContent"), "Hello");
        assert_eq!(run(&mut e, "document.getElementById('nope') === null"), "true");
        assert_eq!(run(&mut e, "document.querySelectorAll('li.i').length + ',' + document.querySelector('li.sel').textContent + ',' + document.getElementsByTagName('li').length + ',' + document.getElementsByClassName('i sel').length"), "2,two,2,1");
        assert_eq!(run(&mut e, "document.getElementById('h') === document.querySelector('h1')"), "true", "the same node is the same object");
        assert_eq!(run(&mut e, "document.title = 'New'; document.title"), "New");
    }

    #[test]
    fn a_fragment_gets_the_parts_a_browser_would_give_it() {
        let mut e = page("<p id=x>just a paragraph</p>");
        assert_eq!(run(&mut e, "document.body.firstElementChild.id + ',' + document.head.tagName + ',' + document.documentElement.children.length"), "x,HEAD,2");
        let mut e = page("<title>T</title><meta charset=utf-8><div id=y>d</div>");
        assert_eq!(run(&mut e, "document.head.children.length + ',' + document.body.children.length + ',' + document.title"), "2,1,T");
    }

    #[test]
    fn elements_are_read_and_changed() {
        let mut e = page(PAGE);
        assert_eq!(run(&mut e, "const a = document.getElementById('a'); [a.tagName, a.getAttribute('href'), a.href, a.dataset.userId, a.hasAttribute('x'), a.getAttribute('x')].join('|')"), "A|/next?x=1|https://a.org/next?x=1|42|false|");
        assert_eq!(run(&mut e, "a.setAttribute('target', '_blank'); a.dataset.fooBar = 'z'; a.className = 'x y'; [a.target, a.getAttribute('data-foo-bar'), a.classList.contains('y'), a.id].join('|')"), "_blank|z|true|a");
        assert_eq!(run(&mut e, "a.classList.add('z'); a.classList.remove('x'); a.classList.toggle('y'); a.className"), "z");
        assert_eq!(run(&mut e, "a.textContent = 'went'; a.outerHTML"), "<a id=\"a\" href=\"/next?x=1\" data-user-id=\"42\" target=\"_blank\" data-foo-bar=\"z\" class=\"z\">went</a>");
        assert_eq!(run(&mut e, "const p = document.getElementById('p'); p.innerHTML = '<i>new</i> &amp; <b>old</b>'; [p.innerHTML, p.textContent, p.children.length, p.firstElementChild.tagName].join('|')"), "<i>new</i> &amp; <b>old</b>|new & old|2|I");
        assert_eq!(run(&mut e, "a.style.color = 'red'; a.style.fontSize = '12px'; a.style.cssText + '|' + a.style.fontSize + '|' + a.getAttribute('style')"), "color: red; font-size: 12px|12px|color: red; font-size: 12px");
        assert_eq!(run(&mut e, "a.removeAttribute('target'); a.target + a.getAttributeNames().join()"), "id,href,data-user-id,data-foo-bar,class,style");
    }

    #[test]
    fn the_tree_can_be_rearranged_and_the_browser_sees_the_result() {
        let mut e = page(PAGE);
        run(&mut e, "const li = document.createElement('li'); li.textContent = 'three'; li.className = 'i'; const l = document.getElementById('l'); l.appendChild(li); l.insertBefore(document.createElement('hr'), l.firstChild); l.removeChild(l.children[1])");
        assert_eq!(run(&mut e, "document.getElementById('l').innerHTML"), "<hr><li class=\"i sel\">two</li><li class=\"i\">three</li>");
        assert_eq!(run(&mut e, "[li.parentNode === l, li.previousSibling.tagName, li.nextSibling === null, l.childNodes.length, l.contains(li), li.isConnected].join()"), "true,LI,true,3,true,true");
        run(&mut e, "const d = document.createElement('div'); d.append('hi ', document.createElement('b')); document.body.prepend(d); document.querySelector('h1').after(document.createElement('hr')); document.querySelector('h1').replaceWith('Title')");
        assert_eq!(run(&mut e, "document.body.innerHTML.slice(0, 45)"), "<div>hi <b></b></div>Title<hr><ul id=\"l\"><hr>");
        assert_eq!(run(&mut e, "const f = document.createDocumentFragment(); f.appendChild(document.createElement('i')); f.appendChild(document.createElement('u')); document.body.appendChild(f); document.body.lastElementChild.tagName + f.childNodes.length"), "U0", "a fragment's children move out when it is attached");
        assert_eq!(run(&mut e, "const c = document.getElementById('l').cloneNode(true); [c.parentNode, c.children.length, c.isConnected, document.querySelectorAll('li').length].join()"), ",3,false,2");
        // a node cannot be put inside itself; a missing reference is an error
        assert!(run(&mut e, "l.appendChild(l)").starts_with("ERROR"));
        assert!(run(&mut e, "l.insertBefore(document.createElement('p'), document.body)").starts_with("ERROR"));
        // what the page now looks like is what the host holds
        let dom = e.module::<DomModule>("dom").unwrap();
        assert!(dom.tree.outer_html(0).contains("<div>hi <b></b></div>Title"));
    }

    #[test]
    fn events_capture_bubble_stop_and_prevent() {
        let mut e = page(PAGE);
        run(&mut e, "const log = []; const l = document.getElementById('l'); const li = l.firstElementChild;\
            document.addEventListener('click', (ev) => log.push('doc'));\
            window.addEventListener('click', (ev) => log.push('win'));\
            l.addEventListener('click', (ev) => log.push('ul-capture'), true);\
            l.addEventListener('click', (ev) => log.push('ul'));\
            li.addEventListener('click', (ev) => { log.push('li ' + ev.target.tagName + ' ' + ev.currentTarget.tagName); ev.preventDefault(); });\
            li.onclick = () => log.push('li-onclick');\
            li.click();");
        assert_eq!(run(&mut e, "log.join(',')"), "ul-capture,li LI LI,li-onclick,ul,doc,win");
        assert_eq!(run(&mut e, "log.length = 0; l.addEventListener('click', (ev) => ev.stopPropagation()); li.click(); log.join(',')"), "ul-capture,li LI LI,li-onclick,ul");
        assert_eq!(run(&mut e, "const ev = new Event('x', { cancelable: true }); li.addEventListener('x', (e) => e.preventDefault()); li.dispatchEvent(ev)"), "false");
        assert_eq!(run(&mut e, "let n = 0; const h = () => n++; li.addEventListener('y', h); li.addEventListener('y', h); li.dispatchEvent(new Event('y')); li.removeEventListener('y', h); li.dispatchEvent(new Event('y')); n"), "1", "the same listener is added once and can be removed");
        assert_eq!(run(&mut e, "let m = 0; li.addEventListener('z', () => m++, { once: true }); li.dispatchEvent(new Event('z')); li.dispatchEvent(new Event('z')); m"), "1");
        assert_eq!(run(&mut e, "let d; li.addEventListener('c', (e) => d = e.detail); li.dispatchEvent(new CustomEvent('c', { detail: { k: 1 } })); d.k"), "1");
        // a listener that throws does not stop the others
        run(&mut e, "const t = document.createElement('div'); t.addEventListener('q', () => { throw new Error('bad'); }); t.addEventListener('q', () => log.push('after throw')); t.dispatchEvent(new Event('q'))");
        assert!(run(&mut e, "log.slice(-1)[0]") == "after throw");
        assert!(logged(&mut e).iter().any(|l| l.starts_with("Uncaught Error: bad")));
    }

    #[test]
    fn page_loading_events_fire_in_order() {
        let mut e = page(PAGE);
        run(&mut e, "const order = []; document.addEventListener('readystatechange', () => order.push('rs:' + document.readyState)); document.addEventListener('DOMContentLoaded', () => order.push('dcl')); window.addEventListener('load', () => order.push('load')); window.onload = () => order.push('onload');");
        e.eval("t", "__dom.fireLoad()").unwrap();
        assert_eq!(run(&mut e, "order.join(',')"), "rs:interactive,dcl,rs:complete,load,onload");
    }

    #[test]
    fn scripts_added_by_scripts_run_but_markup_set_as_innerhtml_does_not() {
        let mut e = page(PAGE);
        run(&mut e, "var ran = []; const s = document.createElement('script'); s.textContent = 'ran.push(\"dynamic\")'; document.body.appendChild(s); document.body.appendChild(s);");
        assert_eq!(run(&mut e, "ran.join()"), "dynamic", "a script runs once, when it is attached");
        run(&mut e, "document.body.innerHTML = '<script>ran.push(\"from innerHTML\")</script>'");
        assert_eq!(run(&mut e, "ran.join()"), "dynamic", "scripts in innerHTML never run");
        run(&mut e, "const j = document.createElement('script'); j.type = 'application/json'; j.textContent = 'ran.push(1)'; document.body.appendChild(j); const m = document.createElement('script'); m.type = 'module'; m.textContent = 'ran.push(2)'; document.body.appendChild(m);");
        assert_eq!(run(&mut e, "ran.join()"), "dynamic", "only classic JavaScript runs");
    }

    #[test]
    fn the_host_lists_the_scripts_a_page_wants_to_run() {
        let tree = html::parse("<script>one()</script><p>x</p><script src=\"/a.js\"></script><script type=\"module\">m()</script><script type=\"application/ld+json\">{}</script>");
        let dom = DomModule::new(tree);
        let s = dom.pending_scripts();
        assert_eq!(s.iter().map(|x| (x.src.clone(), x.text.clone(), x.is_classic_js())).collect::<Vec<_>>(), vec![(None, "one()".into(), true), (Some("/a.js".into()), String::new(), true), (None, "m()".into(), false), (None, "{}".into(), false)]);
    }

    #[test]
    fn the_platform_interfaces_pages_probe_for_exist_and_the_common_ones_work() {
        let mut e = page("<div id=a><p>one</p><!--c--><span>two</span></div>");
        e.eval("t", r#"
          const ev = document.createEvent('Event'); ev.initEvent('foo', true, true); ev.preventDefault();
          const ce = document.createEvent('CustomEvent'); ce.initCustomEvent('bar', false, false, { x: 1 });
          const w = document.createTreeWalker(document.getElementById('a'), NodeFilter.SHOW_ELEMENT, null, false);
          const tags = []; let n; while ((n = w.nextNode())) tags.push(n.localName);
          const ts = []; const it = document.createNodeIterator(document.getElementById('a'), NodeFilter.SHOW_TEXT); let t; while ((t = it.nextNode())) ts.push(t.data);
          let err = ''; try { customElements.define('x-a', class extends HTMLElement {}); customElements.define('x-a', class extends HTMLElement {}); } catch (e) { err = e.name + ':' + (e instanceof DOMException); }
          const c = document.createTextNode('ab'); c.appendData('cd');
          console.log([ev.defaultPrevented, ce.detail.x, tags.join(','), ts.join('|'), err, c.data, c instanceof CharacterData, typeof Range, typeof Blob, new Intl.NumberFormat('en-US').format(1234567.891), typeof XMLSerializer, typeof ShadowRoot, typeof CDATASection].join(' '));
        "#).unwrap();
        e.run_jobs().unwrap();
        let out = logged(&mut e);
        assert_eq!(out, vec!["true 1 p,span one|two NotSupportedError:true abcd true function function 1,234,567.891 function function function"]);
    }

    #[test]
    fn the_window_has_a_prototype_chain_elements_have_their_own_classes_and_streams_and_digests_work() {
        let mut e = page("<a id=l href='/x/y?q=1#h'>l</a><svg id=s><path id=p></path></svg><input id=i>");
        e.eval("t", r#"
          const l = document.getElementById('l');
          const out = [
            window instanceof Window, Object.getPrototypeOf(Window.prototype) === EventTarget.prototype, typeof Window.prototype.addEventListener,
            l instanceof HTMLAnchorElement, document.getElementById('s') instanceof SVGSVGElement, document.getElementById('p') instanceof SVGPathElement, document.getElementById('i') instanceof HTMLInputElement, document.createElement('div') instanceof HTMLDivElement,
            l.pathname, l.search, l.hash, l.host,
            navigator instanceof Navigator,
          ];
          console.log(out.join(' '));
          let got = [];
          window.addEventListener('ping', (ev) => got.push(ev.type + ':' + (ev.target === window)));
          window.dispatchEvent(new Event('ping'));
          const rs = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } });
          (async () => {
            const r = rs.getReader(); const chunks = []; for (;;) { const x = await r.read(); if (x.done) break; chunks.push(x.value); }
            const ts = new TransformStream({ transform(c, ctl) { ctl.enqueue(String(c).toUpperCase()); } });
            const w = ts.writable.getWriter(); w.write('x'); w.write('y'); w.close();
            const rr = ts.readable.getReader(); const up = []; for (;;) { const x = await rr.read(); if (x.done) break; up.push(x.value); }
            const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc'));
            const hex = Array.from(new Uint8Array(d)).map((b) => b.toString(16).padStart(2, '0')).join('');
            const d1 = await crypto.subtle.digest('SHA-1', new TextEncoder().encode('abc'));
            console.log(got.join(',') + ' ' + chunks.join('') + ' ' + up.join('') + ' ' + hex + ' ' + Array.from(new Uint8Array(d1)).map((b) => b.toString(16).padStart(2, '0')).join(''));
          })();
        "#).unwrap();
        e.run_jobs().unwrap();
        let out = logged(&mut e);
        assert_eq!(out[0], "true true function true true true true true /x/y ?q=1 #h  true".replace("#h  true", "#h  true").replace("/x/y ?q=1 #h  true", "/x/y ?q=1 #h a.org true").replace("true true function true true true true true /x/y ?q=1 #h a.org true", "true true function true true true true true /x/y ?q=1 #h a.org true"), "{out:?}");
        assert_eq!(out[1], "ping:true ab XY ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn the_things_pages_poke_at_exist_and_are_harmless() {
        let mut e = page(PAGE);
        assert_eq!(run(&mut e, "const r = document.body.getBoundingClientRect(); [r.width, document.body.offsetHeight, typeof requestAnimationFrame, typeof MutationObserver, typeof customElements.define, document.body.matches('body.home'), document.getElementById('a').closest('body') === document.body, document.querySelector('p b').closest('p').id].join()"), "0,0,function,function,function,true,true,p");
        assert_eq!(run(&mut e, "typeof Node + typeof HTMLElement + (document.body instanceof HTMLElement) + (document.body instanceof Node) + (document.body instanceof EventTarget)"), "functionfunctiontruetruetrue");
        run(&mut e, "const o = new IntersectionObserver((es) => console.log('seen ' + es[0].isIntersecting)); o.observe(document.body);");
        run_timers(&mut e, 10, 10).unwrap();
        assert_eq!(logged(&mut e), vec!["seen true"], "lazy-loading code is told everything is visible");
        assert_eq!(run(&mut e, "document.body.insertAdjacentHTML('beforeend', '<em>end</em>'); document.body.lastElementChild.outerHTML"), "<em>end</em>");
        let bad = run(&mut e, "document.querySelector('nonsense[')");
        assert!(bad.starts_with("ERROR") && bad.contains("not a valid selector"), "{bad}");
    }


    #[test]
    fn lookups_by_id_stay_right_as_the_page_changes() {
        let mut e = page("<div id=a><p id=dup>first</p></div><p id=dup>second</p>");
        assert_eq!(run(&mut e, "document.getElementById('dup').textContent + ',' + document.querySelector('#dup').textContent + ',' + document.querySelectorAll('#dup').length"), "first,first,1");
        // a new element is found once it is in the page, and not before
        assert_eq!(run(&mut e, "const n = document.createElement('i'); n.id = 'new'; [document.getElementById('new'), document.querySelector('#new')].join()"), ",");
        assert_eq!(run(&mut e, "document.body.appendChild(n); [document.getElementById('new') === n, document.querySelector('#new') === n].join()"), "true,true");
        // the id changes, goes away, comes back
        assert_eq!(run(&mut e, "n.id = 'renamed'; [document.getElementById('new'), document.getElementById('renamed') === n].join()"), ",true");
        assert_eq!(run(&mut e, "n.removeAttribute('id'); document.getElementById('renamed')"), "null");
        assert_eq!(run(&mut e, "n.setAttribute('ID', 'back'); document.getElementById('back') === n"), "true");
        // taken out of the page, it cannot be found; put back, it can
        assert_eq!(run(&mut e, "n.remove(); document.getElementById('back') === null"), "true");
        assert_eq!(run(&mut e, "document.getElementById('a').appendChild(n); document.getElementById('back') === n"), "true");
        // the first of two with the same id, in document order, also after the first one moves to the end
        assert_eq!(run(&mut e, "const first = document.getElementById('dup'); document.body.appendChild(first); document.getElementById('dup').textContent"), "second");
        // a detached tree is searched under its own root; markup that arrives with innerHTML is found too
        assert_eq!(run(&mut e, "const t = document.createElement('div'); t.innerHTML = '<span id=deep>x</span>'; [t.querySelector('#deep').textContent, document.getElementById('deep')].join()"), "x,");
        assert_eq!(run(&mut e, "document.body.appendChild(t); document.getElementById('deep').textContent + ',' + t.querySelector('#deep').textContent + ',' + document.getElementById('a').querySelector('#deep')"), "x,x,null");
        assert_eq!(run(&mut e, "const c = t.cloneNode(true); document.body.appendChild(c); document.querySelectorAll('#deep').length + ',' + document.getElementById('deep').parentNode.isSameNode(t)"), "1,true");
        assert_eq!(run(&mut e, "document.getElementById('')"), "null");
    }

    #[test]
    fn attributes_read_back_what_was_written_through_every_door() {
        let mut e = page("<a id=l href='/x' CLASS='one two' data-k=v>link</a>");
        assert_eq!(run(&mut e, "const a = document.getElementById('l'); [a.getAttribute('class'), a.getAttribute('CLASS'), a.hasAttribute('href'), a.hasAttribute('nope'), a.getAttribute('nope'), a.getAttributeNames().join()].join('|')"), "one two|one two|true|false||id,href,class,data-k");
        assert_eq!(run(&mut e, "a.setAttribute('Title', 'T'); a.setAttribute('href', '/y'); [a.getAttribute('title'), a.href, a.attributes.length, a.attributes.getNamedItem('title').value, a.attributes.getNamedItem('zzz')].join('|')"), "T|https://a.org/y|5|T|");
        assert_eq!(run(&mut e, "a.removeAttribute('TITLE'); a.className = 'z'; a.dataset.more = 'm'; a.style.color = 'red'; [a.hasAttribute('title'), a.getAttribute('class'), a.getAttribute('data-more'), a.getAttribute('style'), a.hasAttributes()].join('|')"), "false|z|m|color: red|true");
        assert_eq!(run(&mut e, "a.toggleAttribute('hidden'); a.toggleAttribute('hidden'); a.toggleAttribute('open', true); [a.hasAttribute('hidden'), a.hasAttribute('open')].join()"), "false,true");
        // copies and parsed markup bring their attributes along, and are independent of the original
        assert_eq!(run(&mut e, "const c = a.cloneNode(false); c.setAttribute('href', '/copy'); [c.getAttribute('data-k'), c.getAttribute('href'), a.getAttribute('href')].join()"), "v,/copy,/y");
        assert_eq!(run(&mut e, "const d = document.createElement('div'); d.innerHTML = '<b x=1 y=2>t</b>'; const b = d.firstChild; b.setAttribute('x', '9'); [b.getAttribute('x'), b.getAttribute('y'), b.getAttributeNames().join()].join()"), "9,2,x,y");
        // a fresh element starts empty, an attribute written before the first read is not lost
        assert_eq!(run(&mut e, "const f = document.createElement('p'); f.setAttribute('a', '1'); f.setAttribute('b', '2'); f.setAttribute('a', '3'); [f.getAttributeNames().join(), f.getAttribute('a'), f.outerHTML].join('|')"), "a,b|3|<p a=\"3\" b=\"2\"></p>");
        // the host and the script agree on the 128-attribute limit
        assert_eq!(run(&mut e, "const g = document.createElement('p'); for (let i = 0; i < 200; i++) g.setAttribute('k' + i, i); [g.getAttributeNames().length, g.getAttribute('k127'), g.getAttribute('k128'), g.outerHTML.split('=').length - 1].join()"), "128,127,,128");
    }

    #[test]
    fn what_is_remembered_about_the_tree_is_dropped_when_the_tree_changes() {
        let mut e = page("<ul id=l><li id=a>1</li><li id=b>2</li><li id=c>3</li></ul><p id=p></p>");
        assert_eq!(run(&mut e, "const l = document.getElementById('l'), a = document.getElementById('a'), b = document.getElementById('b'), c = document.getElementById('c'), p = document.getElementById('p'); [a.parentNode.id, l.firstChild.id, l.lastChild.id, l.children.length, l.childNodes.length, l.childElementCount, a.nextSibling.id, a.nextElementSibling.id, c.nextSibling, b.previousSibling.id].join()"), "l,a,c,3,3,3,b,b,,a");
        // moving a node: its old parent, its new parent and the siblings all notice
        assert_eq!(run(&mut e, "p.appendChild(a); [a.parentNode.id, l.firstChild.id, l.children.length, p.firstChild.id, p.childElementCount, b.previousSibling, a.isConnected, l.contains(a), p.contains(a)].join()"), "p,b,2,a,1,,true,false,true");
        assert_eq!(run(&mut e, "l.insertBefore(a, c); [l.children.length, b.nextSibling.id, a.previousSibling.id, a.parentNode.id, p.firstChild, p.hasChildNodes()].join()"), "3,a,b,l,,false");
        assert_eq!(run(&mut e, "b.remove(); [b.parentNode, b.isConnected, l.firstChild.id, a.previousSibling, l.children.length].join()"), ",false,a,,2");
        assert_eq!(run(&mut e, "l.innerHTML = '<li id=n>new</li>'; [a.parentNode, a.isConnected, l.firstChild.id, l.children.length, document.getElementById('n').parentNode === l].join()"), ",false,n,1,true");
        assert_eq!(run(&mut e, "l.textContent = 'plain'; [l.firstChild.nodeType, l.children.length, l.childNodes.length, document.getElementById('n')].join()"), "3,0,1,");
        assert_eq!(run(&mut e, "[document.body.lastElementChild.id, document.documentElement.lastElementChild === document.body, document.head.tagName].join()"), "p,true,HEAD");
        // closest and contains agree with the parent chain
        assert_eq!(run(&mut e, "const q = document.createElement('div'); q.innerHTML = '<section class=s><p><em id=e>x</em></p></section>'; const em = q.querySelector('#e'); [em.closest('section').className, em.closest('em') === em, em.closest('table'), q.contains(em), em.contains(q), q.contains(q), em.closest('p').parentNode.className].join()"), "s,true,,true,false,true,s");
        // the document element is the document element after the body is replaced
        assert_eq!(run(&mut e, "const nb = document.createElement('body'); nb.id = 'nb'; document.body = nb; [document.body.id, document.documentElement.lastElementChild.id].join()"), "nb,nb");
    }

    #[test]
    fn selectors_are_reused_and_bad_ones_keep_failing() {
        let mut e = page("<div class='a b' id=x><span class=a>1</span><span class=b>2</span></div>");
        for _ in 0..3 {
            assert_eq!(run(&mut e, "[document.querySelectorAll('.a').length, document.querySelector('div > span.b').textContent, document.getElementById('x').matches('div.a.b'), document.querySelectorAll('span.a, span.b').length].join()"), "2,2,true,2");
            assert!(run(&mut e, "document.querySelector('p[')").starts_with("ERROR"));
        }
        assert_eq!(run(&mut e, "document.querySelector('.a').className = 'z'; document.querySelectorAll('.a').length + ',' + document.querySelectorAll('.z').length"), "1,1");
    }

    #[test]
    fn platform_gaps_that_polyfills_and_frameworks_ran_into() {
        let mut e = page("<div id=h></div>");
        // a polyfill's shadow root is a plain object with a `host` of its own, not a link
        assert_eq!(run(&mut e, "const f = document.createDocumentFragment(); f.host = document.getElementById('h'); f.mode = 'open'; [f.host.id, f.mode].join()"), "h,open");
        assert_eq!(run(&mut e, "const a = new Audio('s.mp3'); [a.tagName, a.getAttribute('src'), a instanceof HTMLElement, typeof a.play].join()"), "AUDIO,s.mp3,true,function");
        assert_eq!(run(&mut e, "const h = document.getElementById('h'); h.style = 'color: red'; h.text = 'hello'; h.classList = 'k l'; [h.getAttribute('style'), h.textContent, h.className].join('|')"), "color: red|hello|k l");
        assert_eq!(run(&mut e, "const l = document.createElement('a'); l.setAttribute('href', '/p?q=1'); [l.pathname, l.search].join()"), "/p,?q=1");
        // a script added by a script is named after its address in errors
        let mut e = page("<div></div>");
        run(&mut e, "const s = document.createElement('script'); s.textContent = 'function boom() { throw new Error(\"x\"); }\\nboom();'; document.body.appendChild(s);");
        let seen = logged(&mut e).join("\n");
        assert!(seen.contains("Uncaught Error: x") && seen.contains("inline:"), "{seen}");
    }
}

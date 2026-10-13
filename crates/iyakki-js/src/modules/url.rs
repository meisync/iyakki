//! `URL` and `URLSearchParams`.

use std::any::Any;

use crate::engine::WebModule;

pub struct UrlModule;

impl WebModule for UrlModule {
    fn name(&self) -> &'static str {
        "url"
    }

    fn prelude(&self) -> &'static str {
        r#"
(function (g) {
  'use strict';
  const enc = (s) => encodeURIComponent(s).replace(/%20/g, '+').replace(/[!'()~]/g, (c) => '%' + c.charCodeAt(0).toString(16).toUpperCase());
  const dec = (s) => { try { return decodeURIComponent(s.replace(/\+/g, ' ')); } catch (e) { return s; } };
  class URLSearchParams {
    constructor(init = '') {
      this._l = [];
      this._u = null;
      if (typeof init === 'string') this._parse(init);
      else if (init instanceof URLSearchParams) this._l = init._l.map((p) => p.slice());
      else if (Array.isArray(init)) for (const [k, v] of init) this._l.push([String(k), String(v)]);
      else if (init && typeof init === 'object') for (const k of Object.keys(init)) this._l.push([k, String(init[k])]);
    }
    _parse(s) { this._l = []; s = s.replace(/^\?/, ''); if (!s) return; for (const part of s.split('&')) { if (!part) continue; const i = part.indexOf('='); this._l.push(i < 0 ? [dec(part), ''] : [dec(part.slice(0, i)), dec(part.slice(i + 1))]); } }
    _touch() { if (this._u) this._u._search = this.toString() ? '?' + this.toString() : ''; }
    append(k, v) { this._l.push([String(k), String(v)]); this._touch(); }
    delete(k) { this._l = this._l.filter((p) => p[0] !== String(k)); this._touch(); }
    get(k) { const p = this._l.find((p) => p[0] === String(k)); return p ? p[1] : null; }
    getAll(k) { return this._l.filter((p) => p[0] === String(k)).map((p) => p[1]); }
    has(k) { return this._l.some((p) => p[0] === String(k)); }
    set(k, v) { k = String(k); v = String(v); const i = this._l.findIndex((p) => p[0] === k); if (i < 0) this._l.push([k, v]); else { this._l[i][1] = v; this._l = this._l.filter((p, j) => j <= i || p[0] !== k); } this._touch(); }
    sort() { this._l.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); this._touch(); }
    forEach(f, t) { for (const [k, v] of this._l) f.call(t, v, k, this); }
    keys() { return this._l.map((p) => p[0])[Symbol.iterator](); }
    values() { return this._l.map((p) => p[1])[Symbol.iterator](); }
    entries() { return this._l.map((p) => [p[0], p[1]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    get size() { return this._l.length; }
    toString() { return this._l.map((p) => enc(p[0]) + '=' + enc(p[1])).join('&'); }
  }
  const DEFAULT_PORT = { 'http:': '80', 'https:': '443' };
  const norm = (path) => {
    const out = [];
    const segs = path.split('/');
    for (let i = 0; i < segs.length; i++) {
      const s = segs[i];
      if (s === '..') { if (out.length > 1) out.pop(); if (i === segs.length - 1) out.push(''); }
      else if (s === '.') { if (i === segs.length - 1) out.push(''); }
      else out.push(s);
    }
    let r = out.join('/');
    if (!r.startsWith('/')) r = '/' + r;
    return r;
  };
  class URL {
    constructor(url, base) {
      url = String(url).trim();
      let m = /^([a-zA-Z][a-zA-Z0-9+.-]*):(\/\/)?([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(url);
      if (m && (m[1].toLowerCase() === 'http' || m[1].toLowerCase() === 'https') && m[2]) {
        this._init(m[1].toLowerCase() + ':', m[3], m[4] || '/', m[5] || '', m[6] || '');
      } else if (m && !m[2] && /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(url) && !/^(https?):/i.test(url)) {
        this._init(m[1].toLowerCase() + ':', '', m[3] + m[4], m[5] || '', m[6] || '', true);
      } else if (base !== undefined) {
        const b = base instanceof URL ? base : new URL(base);
        const r = /^([^?#]*)(\?[^#]*)?(#.*)?$/.exec(url);
        let path = r[1], search = r[2] || '', hash = r[3] || '';
        if (url.startsWith('//')) { const mm = /^\/\/([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(url); return void this._init(b.protocol, mm[1], mm[2] || '/', mm[3] || '', mm[4] || ''); }
        if (path === '') { path = b.pathname; if (!r[2] && url[0] !== '?') search = b.search; }
        else if (path[0] !== '/') path = b.pathname.slice(0, b.pathname.lastIndexOf('/') + 1) + path;
        this._init(b.protocol, b.host, path, search, hash);
      } else throw new TypeError('Invalid URL: ' + url);
    }
    _init(protocol, host, path, search, hash, opaque) {
      if (/@/.test(host)) host = host.slice(host.lastIndexOf('@') + 1);
      this._protocol = protocol; this._opaque = !!opaque;
      const pm = /^(.*?)(?::(\d*))?$/.exec(host.toLowerCase());
      this._hostname = pm[1]; this._port = pm[2] && pm[2] !== DEFAULT_PORT[protocol] ? pm[2] : '';
      this._pathname = opaque ? path : norm(path);
      this._search = search === '?' ? '' : search; this._hash = hash === '#' ? '' : hash;
      this._sp = new URLSearchParams(this._search); this._sp._u = this;
    }
    get protocol() { return this._protocol; }
    get hostname() { return this._hostname; }
    get port() { return this._port; }
    get host() { return this._hostname + (this._port ? ':' + this._port : ''); }
    get origin() { return this._opaque ? 'null' : this._protocol + '//' + this.host; }
    get pathname() { return this._pathname; }
    get search() { return this._search; }
    get hash() { return this._hash; }
    get searchParams() { return this._sp; }
    get username() { return ''; }
    get password() { return ''; }
    set search(v) { v = String(v); this._search = v && v[0] !== '?' ? '?' + v : v; this._sp._parse(this._search); }
    set hash(v) { v = String(v); this._hash = v && v[0] !== '#' ? '#' + v : v; }
    set pathname(v) { this._pathname = norm(String(v)); }
    get href() { return this._opaque ? this._protocol + this._pathname + this._search + this._hash : this._protocol + '//' + this.host + this._pathname + this._search + this._hash; }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static canParse(u, b) { try { new URL(u, b); return true; } catch (e) { return false; } }
  }
  g.URL = URL;
  g.URLSearchParams = URLSearchParams;
})(globalThis);
"#
    }

    fn call(&mut self, op: &str, _args: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err(format!("url has no operation {op}"))
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsEngine, Limits};

    fn run(src: &str) -> String {
        let mut e = JsEngine::new(Limits::default(), vec![Box::new(UrlModule)]).unwrap();
        e.eval("t", src).unwrap()
    }

    #[test]
    fn urls_are_taken_apart_and_resolved_like_a_browser_does() {
        assert_eq!(run("const u = new URL('HTTPS://Example.ORG:8443/a/./b/../c?x=1&y=two#frag'); [u.protocol, u.host, u.hostname, u.port, u.pathname, u.search, u.hash, u.origin].join('|')"), "https:|example.org:8443|example.org|8443|/a/c|?x=1&y=two|#frag|https://example.org:8443");
        assert_eq!(run("new URL('https://a.org:443/').href"), "https://a.org/");
        assert_eq!(run("new URL('/x?q=1', 'https://a.org/dir/page').href"), "https://a.org/x?q=1");
        assert_eq!(run("new URL('sub/y', 'https://a.org/dir/page').href"), "https://a.org/dir/sub/y");
        assert_eq!(run("new URL('../y', 'https://a.org/dir/sub/page').href"), "https://a.org/dir/y");
        assert_eq!(run("new URL('?z=2', 'https://a.org/p?old=1#h').href"), "https://a.org/p?z=2");
        assert_eq!(run("new URL('#top', 'https://a.org/p?q=1').href"), "https://a.org/p?q=1#top");
        assert_eq!(run("new URL('//cdn.org/f.js', 'https://a.org/').href"), "https://cdn.org/f.js");
        assert_eq!(run("new URL('mailto:a@b.c').protocol"), "mailto:");
        assert_eq!(run("try { new URL('not a url'); 'no' } catch (e) { e.constructor.name }"), "TypeError");
        assert_eq!(run("URL.canParse('https://a.org') + ',' + URL.canParse('nope')"), "true,false");
        assert_eq!(run("JSON.stringify({ u: new URL('https://a.org/x') })"), "{\"u\":\"https://a.org/x\"}");
    }

    #[test]
    fn search_params_read_write_and_stay_linked_to_the_url() {
        assert_eq!(run("const p = new URLSearchParams('?a=1&b=two%20words&a=3&c'); [p.get('a'), p.getAll('a').join('+'), p.get('b'), p.get('c'), p.get('zz'), p.has('b')].join('|')"), "1|1+3|two words|||true");
        assert_eq!(run("const p = new URLSearchParams({ q: 'a b', n: 1 }); p.append('q', 'x&y'); p.toString()"), "q=a+b&n=1&q=x%26y");
        assert_eq!(run("const u = new URL('https://a.org/p?x=1'); u.searchParams.set('x', '2'); u.searchParams.append('y', 'é'); u.href"), "https://a.org/p?x=2&y=%C3%A9");
        assert_eq!(run("const u = new URL('https://a.org/p'); u.search = 'k=v'; u.searchParams.get('k')"), "v");
        assert_eq!(run("[...new URLSearchParams('a=1&b=2')].map(([k, v]) => k + v).join()"), "a1,b2");
        assert_eq!(run("const p = new URLSearchParams('b=1&a=2'); p.sort(); p.toString()"), "a=2&b=1");
        assert_eq!(run("const p = new URLSearchParams('a=1&a=2&b=3'); p.set('a', 'x'); p.delete('b'); p.toString()"), "a=x");
    }
}

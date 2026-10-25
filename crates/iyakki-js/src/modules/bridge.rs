//! The bridge of an app that lives in a tile: `window.iyakki`, the one way its scripts reach the system. The script side checks
//! every argument before a call leaves the page (types, key shapes, sizes); the host side ([`Bridge`]) checks them again and
//! decides what the app may do, so a page that talks to `__host` directly gets nothing more than the bridge allows.
//!
//! Calls that need the person (a file dialog, a question to the assistant) cannot answer at once: they return a number and the
//! page gets a promise, settled when the host calls `__iyakkiResolve(id, ok, value)`.
//!
//! The prelude also takes `eval` and the `Function` constructors away (the strict policy of tile apps: no code made from text).

use std::any::Any;

use crate::engine::WebModule;

/// What the embedder answers. `op` is one of the names in the prelude (`storage.get`, `ui.toast`, ...); `args` is an object.
pub trait Bridge: Send {
    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String>;
}

pub struct BridgeModule {
    pub handler: Box<dyn Bridge>,
}

impl BridgeModule {
    pub fn new(handler: Box<dyn Bridge>) -> BridgeModule {
        BridgeModule { handler }
    }
}

const PRELUDE: &str = r#"(function (g) { 'use strict';
  const raw = g.__host;
  // the host's refusals carry a code (`NoCapability`, `BudgetStop`, ...): the page gets an Error with `.code` and the code's extra fields
  const coded = (m) => {
    m = String(m);
    const at = m.indexOf('iyakki-error:');
    if (at >= 0) {
      try {
        const j = JSON.parse(m.slice(at + 13));
        const e = new Error(j.message);
        for (const k of Object.keys(j)) if (k !== 'message') e[k] = j[k];
        return e;
      } catch (x) { /* not ours */ }
    }
    return new Error(m);
  };
  const call = (op, a) => {
    let r;
    try { r = raw('iyakki', op, JSON.stringify(a === undefined ? {} : a)); } catch (e) { throw coded(e && e.message !== undefined ? e.message : e); }
    return JSON.parse(r);
  };
  const pending = new Map();
  const streams = new Map();
  const KEY = /^[A-Za-z0-9._:-]{1,64}$/;
  const key = (k) => need(typeof k === 'string' && KEY.test(k), 'a key is 1 to 64 letters, digits and . _ : -');
  const need = (ok, what) => { if (!ok) throw new TypeError('iyakki: ' + what); };
  const str = (v, what, max) => { need(typeof v === 'string', what + ' must be a string'); need(v.length <= max, what + ' is longer than ' + max + ' characters'); return v; };
  const wait = (op, a) => new Promise((resolve, reject) => {
    let r;
    try { r = call(op, a); } catch (e) { reject(e); return; }
    if (r !== null && typeof r === 'object' && typeof r.pending === 'number') pending.set(r.pending, [resolve, reject]); else resolve(r);
  });
  const ROLES = ['system', 'user', 'assistant'];
  const opt = (v, t, what) => { if (v !== undefined && v !== null) need(typeof v === t, what + ' must be a ' + t); return v; };
  // chat({messages, model?, max_tokens?, stream?}) or chat(messages, opts)
  const chatArgs = (a, o) => {
    const r = Array.isArray(a) ? Object.assign({}, o || {}, { messages: a }) : Object.assign({}, a || {});
    need(Array.isArray(r.messages) && r.messages.length >= 1 && r.messages.length <= 64, 'messages is a list of 1 to 64 {role, content}');
    let n = 0;
    for (const m of r.messages) {
      need(m !== null && typeof m === 'object' && ROLES.indexOf(m.role) >= 0, 'a message role is system, user or assistant');
      n += str(m.content, 'a message content', 100000).length;
    }
    need(n <= 100000, 'the messages are longer than 100000 characters in all');
    if (r.maxTokens !== undefined && r.max_tokens === undefined) r.max_tokens = r.maxTokens;
    delete r.maxTokens;
    opt(r.model, 'string', 'model');
    opt(r.stream, 'boolean', 'stream');
    if (r.max_tokens !== undefined && r.max_tokens !== null) need(Number.isInteger(r.max_tokens) && r.max_tokens >= 1 && r.max_tokens <= 4096, 'max_tokens is a whole number from 1 to 4096');
    return r;
  };
  const openStream = (r) => {
    const res = call('llm.stream', r);
    const on = {};
    const h = {
      on: (ev, fn) => { need(['text', 'done', 'error', 'dropped'].indexOf(ev) >= 0, 'events are text, done, error and dropped'); need(typeof fn === 'function', 'a handler must be a function'); (on[ev] = on[ev] || []).push(fn); return h; },
      off: (ev, fn) => { on[ev] = (on[ev] || []).filter((f) => f !== fn); return h; },
      cancel: () => { call('llm.cancel', { handle: res.handle }); },
    };
    streams.set(res.handle, on);
    return h;
  };
  const say = (text, o) => {
    o = o === undefined || o === null ? {} : o;
    need(typeof o === 'object', 'the options are an object');
    const r = { text: str(text, 'the text', 20000) };
    for (const k of Object.keys(o)) {
      need(['voice', 'lang', 'speed'].indexOf(k) >= 0, 'unknown option ' + k);
      if (o[k] !== undefined) r[k] = o[k];
    }
    if (r.voice !== undefined) str(r.voice, 'the voice', 80);
    if (r.lang !== undefined) str(r.lang, 'the language', 16);
    if (r.speed !== undefined) need(typeof r.speed === 'number' && r.speed >= 0.7 && r.speed <= 1.5, 'speed is a number from 0.7 to 1.5');
    return r;
  };
  const api = {
    app: { info: () => call('app.info') },
    llm: {
      chat: (a, o) => { const r = chatArgs(a, o); return r.stream ? openStream(r) : wait('llm.chat', r); },
      stream: (a, o) => openStream(chatArgs(a, o)),
      usage: (o) => wait('llm.usage', o === undefined ? {} : o),
    },
    models: { list: (o) => wait('models.list', o === undefined ? {} : o) },
    speech: {
      say: (text, o) => wait('speech.say', say(text, o)),
      stop: (id) => wait('speech.stop', id === undefined ? {} : { id }),
    },
    storage: {
      get: (k) => { key(k); return call('storage.get', { k }); },
      set: (k, v) => { key(k); call('storage.set', { k, v: str(v, 'a value', 65536) }); },
      remove: (k) => { key(k); call('storage.remove', { k }); },
      list: () => call('storage.list'),
    },
    ui: {
      toast: (t) => { call('ui.toast', { text: str(t, 'the text', 200) }); },
      title: (t) => { call('ui.title', { text: str(t, 'the title', 60) }); },
    },
    host: {
      openFile: () => wait('host.openFile'),
      saveFile: (name, text) => wait('host.saveFile', { name: str(name, 'the file name', 80), text: str(text, 'the text', 262144) }),
    },
    clipboard: {
      read: () => wait('clipboard.read'),
      write: (t) => wait('clipboard.write', { text: str(t, 'the text', 65536) }),
    },
    notify: { show: (title, body) => { call('notify.show', { title: str(title, 'the title', 80), body: str(body === undefined ? '' : body, 'the text', 300) }); } },
    agent: { ask: (q) => wait('agent.ask', { q: str(q, 'the question', 2000) }) },
  };
  const freeze = (o) => { for (const k of Object.keys(o)) if (o[k] && typeof o[k] === 'object') freeze(o[k]); return Object.freeze(o); };
  Object.defineProperty(g, 'iyakki', { value: freeze(api), enumerable: true, writable: false, configurable: false });
  Object.defineProperty(g, '__iyakkiResolve', { value: (id, ok, v) => { const p = pending.get(id); if (!p) return false; pending.delete(id); (ok ? p[0] : p[1])(ok ? v : coded(v)); return true; }, enumerable: false, writable: false, configurable: false });
  // events of a stream: the host calls this on the page's next turn, one event at a time
  Object.defineProperty(g, '__iyakkiEmit', { value: (id, ev, payload) => {
    const on = streams.get(id);
    if (!on) return false;
    const arg = ev === 'error' ? coded('iyakki-error:' + JSON.stringify(payload)) : payload;
    for (const f of (on[ev] || []).slice()) { try { f(arg); } catch (x) { /* a handler's mistake is its own */ } }
    if (ev === 'done' || ev === 'error') streams.delete(id);
    return true;
  }, enumerable: false, writable: false, configurable: false });
  // no code made from text, in any of its forms
  const blocked = function () { throw new EvalError('this app may not make code from text'); };
  Object.defineProperty(g, 'eval', { value: blocked, writable: false, configurable: false });
  for (const f of [function () {}, async function () {}, function* () {}, async function* () {}]) {
    Object.defineProperty(Object.getPrototypeOf(f), 'constructor', { value: blocked, writable: false, configurable: false });
  }
  Object.defineProperty(g, 'Function', { value: blocked, writable: false, configurable: false });
})(globalThis);
"#;

impl WebModule for BridgeModule {
    fn name(&self) -> &'static str {
        "iyakki"
    }

    fn prelude(&self) -> &'static str {
        PRELUDE
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.handler.call(op, args)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{JsEngine, Limits};
    use crate::modules::console::ConsoleModule;
    use serde_json::{json, Value};

    struct Echo(Vec<String>);
    impl Bridge for Echo {
        fn call(&mut self, op: &str, args: &Value) -> Result<Value, String> {
            self.0.push(format!("{op} {args}"));
            match op {
                "storage.get" => Ok(json!("v")),
                "host.openFile" => Ok(json!({"pending": 7})),
                "ui.toast" => Err("no way".into()),
                _ => Ok(Value::Null),
            }
        }
    }

    fn engine() -> JsEngine {
        JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new()), Box::new(BridgeModule::new(Box::new(Echo(vec![]))))]).unwrap()
    }

    #[test]
    fn the_page_side_checks_arguments_before_a_call_leaves() {
        let mut e = engine();
        assert_eq!(e.eval("t", "iyakki.storage.get('a.b')").unwrap(), "v");
        for bad in ["iyakki.storage.get('has space')", "iyakki.storage.get(5)", "iyakki.storage.set('k', 5)", "iyakki.storage.set('k', 'x'.repeat(70000))", "iyakki.ui.toast('x'.repeat(201))", "iyakki.ui.title({})"] {
            assert!(matches!(e.eval("t", bad), Err(crate::JsError::Exception(m)) if m.contains("iyakki:")), "{bad}");
        }
        assert!(matches!(e.eval("t", "iyakki.ui.toast('hi')"), Err(crate::JsError::Exception(m)) if m.contains("no way")), "a refusal from the host is an exception");
    }

    #[test]
    fn the_bridge_cannot_be_replaced_and_code_cannot_be_made_from_text() {
        let mut e = engine();
        assert_eq!(e.eval("t", "iyakki = 1; typeof iyakki.storage").unwrap(), "object", "assigning to it changes nothing");
        assert_eq!(e.eval("t", "(function(){ try { iyakki.storage.get = null; } catch (x) {} return typeof iyakki.storage.get; })()").unwrap(), "function");
        for code in ["eval('1+1')", "new Function('return 1')()", "(function(){}).constructor('return 1')()", "(async function(){}).constructor('return 1')", "Object.getPrototypeOf(function*(){}).constructor('return 1')"] {
            assert!(matches!(e.eval("t", code), Err(crate::JsError::Exception(m)) if m.contains("code from text")), "{code}");
        }
    }

    #[test]
    fn a_slow_call_gives_a_promise_the_host_settles_later() {
        let mut e = engine();
        e.eval("t", "globalThis.out = []; iyakki.host.openFile().then((v) => out.push('ok ' + v), (x) => out.push('no ' + x.message));").unwrap();
        assert_eq!(e.eval("t", "out.join()").unwrap(), "");
        assert_eq!(e.eval("t", "String(__iyakkiResolve(7, true, 'file text'))").unwrap(), "true");
        e.run_jobs().unwrap();
        assert_eq!(e.eval("t", "out.join()").unwrap(), "ok file text");
        assert_eq!(e.eval("t", "String(__iyakkiResolve(7, true, 'again'))").unwrap(), "false", "an answer settles one request once");
    }
}

(function (g) {
  'use strict';
  // indexedDB: requests, transactions and events on top of the host's database engine (idb.rs). Every callback is
  // delivered by a timer, one request per timer, so promise reactions run between a request and the next.
  const M = 'indexedDB';
  const EventTarget = g.EventTarget, Event = g.Event, DOMException = g.DOMException;
  const call = (op, a) => {
    const r = JSON.parse(__host(M, op, JSON.stringify(a || {})));
    if (r.err) { if (r.err[0] === 'TypeError') throw new TypeError(r.err[1]); throw new DOMException(r.err[1], r.err[0]); }
    return r.ok;
  };
  const dataErr = (m) => new DOMException(m || 'The parameter is not a valid key.', 'DataError');
  const hide = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, enumerable: false, configurable: true });

  // ---------------------------------------------------------------- keys
  const hex = (u8) => { let s = ''; for (let i = 0; i < u8.length; i++) s += (u8[i] < 16 ? '0' : '') + u8[i].toString(16); return s; };
  const toU8 = (v) => (v instanceof ArrayBuffer ? new Uint8Array(v) : ArrayBuffer.isView(v) ? new Uint8Array(v.buffer, v.byteOffset, v.byteLength) : null);
  function enc(k, depth) {
    depth = depth || 0;
    if (depth > 64) throw dataErr();
    if (typeof k === 'number') { if (k !== k) throw dataErr(); return [0, k === Infinity ? 'Infinity' : k === -Infinity ? '-Infinity' : k === 0 ? 0 : k]; }
    if (typeof k === 'string') return [2, k];
    if (k instanceof Date) { const t = k.getTime(); if (t !== t) throw dataErr(); return [1, t]; }
    const u = toU8(k); if (u) return [3, hex(u)];
    if (Array.isArray(k)) return [4, k.map((x) => enc(x, depth + 1))];
    throw dataErr();
  }
  function dec(w) {
    switch (w[0]) {
      case 0: return typeof w[1] === 'string' ? Number(w[1]) : w[1];
      case 1: return new Date(typeof w[1] === 'string' ? Number(w[1]) : w[1]);
      case 2: return w[1];
      case 3: { const h = w[1], u = new Uint8Array(h.length / 2); for (let i = 0; i < u.length; i++) u[i] = parseInt(h.substr(i * 2, 2), 16); return u.buffer; }
      default: return w[1].map(dec);
    }
  }
  const num = (x) => (typeof x === 'string' ? Number(x) : x);
  function cmpw(a, b) {
    if (a[0] !== b[0]) return a[0] < b[0] ? -1 : 1;
    if (a[0] === 4) {
      for (let i = 0; i < Math.min(a[1].length, b[1].length); i++) { const c = cmpw(a[1][i], b[1][i]); if (c) return c; }
      return a[1].length < b[1].length ? -1 : a[1].length > b[1].length ? 1 : 0;
    }
    const x = a[0] <= 1 ? num(a[1]) : a[1], y = b[0] <= 1 ? num(b[1]) : b[1];
    return x < y ? -1 : x > y ? 1 : 0;
  }

  class IDBKeyRange {
    constructor(l, u, lo, uo) { this.lower = l; this.upper = u; this.lowerOpen = !!lo; this.upperOpen = !!uo; }
    static only(v) { enc(v); return new IDBKeyRange(v, v, false, false); }
    static lowerBound(v, open) { enc(v); return new IDBKeyRange(v, undefined, open, true); }
    static upperBound(v, open) { enc(v); return new IDBKeyRange(undefined, v, true, open); }
    static bound(l, u, lo, uo) {
      const c = cmpw(enc(l), enc(u));
      if (c > 0 || (c === 0 && (lo || uo))) throw dataErr('The lower key is greater than the upper key.');
      return new IDBKeyRange(l, u, lo, uo);
    }
    includes(k) {
      const w = enc(k);
      if (this.lower !== undefined) { const c = cmpw(w, enc(this.lower)); if (c < 0 || (c === 0 && this.lowerOpen)) return false; }
      if (this.upper !== undefined) { const c = cmpw(w, enc(this.upper)); if (c > 0 || (c === 0 && this.upperOpen)) return false; }
      return true;
    }
  }
  function rangeWire(q) {
    if (q === undefined || q === null) return null;
    if (q instanceof IDBKeyRange) return { l: q.lower === undefined ? null : enc(q.lower), u: q.upper === undefined ? null : enc(q.upper), lo: q.lowerOpen, uo: q.upperOpen };
    const w = enc(q);
    return { l: w, u: w, lo: false, uo: false };
  }

  // ---------------------------------------------------------------- structured clone
  const b64 = (u8) => { let s = ''; for (let i = 0; i < u8.length; i += 8192) s += String.fromCharCode.apply(null, u8.subarray(i, i + 8192)); return btoa(s); };
  const unb64 = (t) => { const s = atob(t), u = new Uint8Array(s.length); for (let i = 0; i < s.length; i++) u[i] = s.charCodeAt(i); return u; };
  const cloneErr = (what) => new DOMException(what + ' could not be cloned.', 'DataCloneError');
  function ser(v, stack) {
    switch (typeof v) {
      case 'undefined': return { $idb: 'u' };
      case 'number': return Number.isFinite(v) && !Object.is(v, -0) ? v : { $idb: 'n', v: Object.is(v, -0) ? '-0' : String(v) };
      case 'string': case 'boolean': return v;
      case 'bigint': return { $idb: 'big', v: String(v) };
      case 'function': case 'symbol': throw cloneErr(typeof v === 'function' ? 'A function' : 'A symbol');
    }
    if (v === null) return null;
    if (v instanceof Date) { const t = v.getTime(); return { $idb: 'date', v: t === t ? t : 'NaN' }; }
    const u = toU8(v);
    if (u) return { $idb: 'buf', t: v instanceof ArrayBuffer ? 'ArrayBuffer' : v.constructor.name, v: b64(u) };
    if (stack.indexOf(v) >= 0 || stack.length > 200) throw cloneErr('A cyclic or too deeply nested value');
    stack.push(v);
    try {
      if (Array.isArray(v)) { const out = []; for (let i = 0; i < v.length; i++) out.push(ser(v[i], stack)); return out; }
      if (v instanceof Map) return { $idb: 'map', v: Array.from(v, (e) => [ser(e[0], stack), ser(e[1], stack)]) };
      if (v instanceof Set) return { $idb: 'set', v: Array.from(v, (e) => ser(e, stack)) };
      if (v instanceof RegExp) return { $idb: 're', s: v.source, f: v.flags };
      const out = {};
      for (const k of Object.keys(v)) out[k] = ser(v[k], stack);
      return Object.prototype.hasOwnProperty.call(out, '$idb') ? { $idb: 'obj', v: out } : out;
    } finally { stack.pop(); }
  }
  function de(w) {
    if (w === null || typeof w !== 'object') return w;
    if (Array.isArray(w)) return w.map(de);
    const out = {};
    const t = w.$idb;
    if (t === undefined) { for (const k of Object.keys(w)) out[k] = de(w[k]); return out; }
    switch (t) {
      case 'u': return undefined;
      case 'n': return Number(w.v);
      case 'big': return BigInt(w.v);
      case 'date': return new Date(num(w.v));
      case 'buf': { const u = unb64(w.v); const buf = u.buffer; if (w.t === 'ArrayBuffer') return buf; const C = g[w.t]; return typeof C === 'function' ? new C(buf) : u; }
      case 'map': return new Map(w.v.map((e) => [de(e[0]), de(e[1])]));
      case 'set': return new Set(w.v.map(de));
      case 're': return new RegExp(w.s, w.f);
      default: for (const k of Object.keys(w.v)) out[k] = de(w.v[k]); return out;
    }
  }

  // ---------------------------------------------------------------- events, requests, transactions
  const fire = (target, type, bubbles, cancelable) => { const ev = new Event(type, { bubbles, cancelable }); target.dispatchEvent(ev); return ev; };
  let txSeq = 0;
  const conns = new Map(); // open connections by database name

  class IDBRequest extends EventTarget {
    constructor(source, tx) {
      super();
      this.result = undefined; this.error = null; this.source = source || null; this.transaction = tx || null; this.readyState = 'pending';
      this.onsuccess = null; this.onerror = null;
    }
    _parentTarget() { return this.transaction; }
  }
  class IDBOpenDBRequest extends IDBRequest {
    constructor() { super(null, null); this.onupgradeneeded = null; this.onblocked = null; }
    _parentTarget() { return null; }
  }
  class IDBVersionChangeEvent extends Event {
    constructor(type, init) { super(type, init); init = init || {}; this.oldVersion = init.oldVersion || 0; this.newVersion = init.newVersion === undefined ? null : init.newVersion; }
  }

  class IDBTransaction extends EventTarget {
    constructor(db, names, mode, id) {
      super();
      this.db = db; this.mode = mode; this.error = null; this.durability = 'default';
      this.oncomplete = null; this.onabort = null; this.onerror = null;
      hide(this, '_names', names); hide(this, '_id', id); hide(this, '_pending', 0); hide(this, '_done', false); hide(this, '_wrote', mode !== 'readonly');
      hide(this, '_after', null); hide(this, '_cache', new Map());
      setTimeout(() => maybeFinish(this), 0);
    }
    _parentTarget() { return this.db; }
    get objectStoreNames() { return names(this.mode === 'versionchange' ? this.db._storeNames() : this._names); }
    objectStore(name) {
      name = String(name);
      if (this._done) throw new DOMException('The transaction has finished.', 'InvalidStateError');
      const ok = this.mode === 'versionchange' ? this.db._storeNames().includes(name) : this._names.includes(name);
      if (!ok) throw new DOMException("The object store '" + name + "' is not in this transaction.", 'NotFoundError');
      let s = this._cache.get(name);
      if (!s) { s = new IDBObjectStore(this, name); this._cache.set(name, s); }
      return s;
    }
    abort() {
      if (this._done) throw new DOMException('The transaction has finished.', 'InvalidStateError');
      abortTx(this, null);
    }
    commit() { if (this._done) throw new DOMException('The transaction has finished.', 'InvalidStateError'); }
  }
  const names = (a) => { const l = Array.from(a); Object.defineProperty(l, 'contains', { value: (n) => l.includes(n) }); Object.defineProperty(l, 'item', { value: (i) => l[i] === undefined ? null : l[i] }); return l; };

  function schedule(tx, req, run) {
    tx._pending++;
    setTimeout(() => step(tx, req, run), 0);
  }
  function step(tx, req, run) {
    if (tx._done) {
      req.readyState = 'done'; req.result = undefined; req.error = new DOMException('The transaction was aborted.', 'AbortError');
      fire(req, 'error', true, true);
      tx._pending--;
      return;
    }
    let ok = true, result, err;
    try { result = run(); } catch (e) { ok = false; err = e; }
    req.readyState = 'done';
    if (ok) { req.result = result; req.error = null; fire(req, 'success', false, false); }
    else {
      req.result = undefined; req.error = err;
      const ev = fire(req, 'error', true, true);
      if (!ev.defaultPrevented) abortTx(tx, err);
    }
    tx._pending--;
    if (!tx._done && tx._pending === 0) setTimeout(() => maybeFinish(tx), 0);
  }
  function maybeFinish(tx) {
    if (tx._done || tx._pending > 0) return;
    tx._done = true;
    if (tx._wrote) { try { call('release', { tx: tx._id }); } catch (_) {} }
    fire(tx, 'complete', false, false);
    if (tx._after) tx._after(false);
  }
  function abortTx(tx, err) {
    if (tx._done) return;
    tx._done = true;
    tx.error = err;
    if (tx._wrote) { try { call('rollback', { tx: tx._id }); } catch (_) {} if (tx.db) tx.db._refresh(); }
    setTimeout(() => { fire(tx, 'abort', true, false); if (tx._after) tx._after(true); }, 0);
  }

  // ---------------------------------------------------------------- reading and writing (stores and indexes)
  function queryArgs(q, count) {
    // getAll({ query, count, direction }) as well as getAll(query, count)
    if (q && typeof q === 'object' && !(q instanceof IDBKeyRange) && !(q instanceof Date) && !Array.isArray(q) && !toU8(q) && ('query' in q || 'count' in q || 'direction' in q)) return [q.query, q.count];
    return [q, count];
  }
  const DIRS = ['next', 'nextunique', 'prev', 'prevunique'];

  class Reader {
    // subclasses give: _tx, _storeName, _indexName
    _base() { return { db: this._tx.db.name, store: this._storeName, index: this._indexName || undefined, tx: this._tx._id }; }
    _req(write, run) {
      const tx = this._tx;
      if (tx._done) throw new DOMException('The transaction has finished.', 'TransactionInactiveError');
      if (write && tx.mode === 'readonly') throw new DOMException('The transaction is read-only.', 'ReadOnlyError');
      const req = new IDBRequest(this, tx);
      schedule(tx, req, run);
      return req;
    }
    get(q) {
      if (q === undefined) throw dataErr();
      const a = Object.assign(this._base(), { range: rangeWire(q) });
      return this._req(false, () => { const r = call('get', a); return 'v' in r ? de(r.v) : undefined; });
    }
    getKey(q) {
      if (q === undefined) throw dataErr();
      const a = Object.assign(this._base(), { range: rangeWire(q) });
      return this._req(false, () => { const r = call('get_key', a); return 'v' in r ? dec(r.v) : undefined; });
    }
    getAll(q, count) {
      [q, count] = queryArgs(q, count);
      const a = Object.assign(this._base(), { range: rangeWire(q), count: count === undefined ? 0 : count >>> 0 });
      return this._req(false, () => call('all', a).map(de));
    }
    getAllKeys(q, count) {
      [q, count] = queryArgs(q, count);
      const a = Object.assign(this._base(), { range: rangeWire(q), count: count === undefined ? 0 : count >>> 0, keys: true });
      return this._req(false, () => call('all', a).map(dec));
    }
    count(q) {
      const a = Object.assign(this._base(), { range: rangeWire(q) });
      return this._req(false, () => call('count', a));
    }
    openCursor(q, dir) { return this._cursor(q, dir, false); }
    openKeyCursor(q, dir) { return this._cursor(q, dir, true); }
    _cursor(q, dir, keysOnly) {
      dir = dir === undefined ? 'next' : String(dir);
      if (!DIRS.includes(dir)) throw new TypeError("'" + dir + "' is not a valid cursor direction");
      const a = Object.assign(this._base(), { range: rangeWire(q), dir, keys_only: keysOnly });
      const req = this._req(false, () => nextOf(cur, null, null, 1));
      const cur = new (keysOnly ? IDBCursor : IDBCursorWithValue)(this, req, a, dir, keysOnly);
      return req;
    }
  }

  class IDBObjectStore extends Reader {
    constructor(tx, name) { super(); hide(this, '_tx', tx); hide(this, '_storeName', name); this.transaction = tx; }
    get _meta() { return this._tx.db._schema.stores.find((s) => s.name === this._storeName); }
    get name() { return this._storeName; }
    get keyPath() { const m = this._meta; return m ? m.keyPath : null; }
    get autoIncrement() { const m = this._meta; return !!(m && m.auto); }
    get indexNames() { const m = this._meta; return names(m ? m.indexes.map((i) => i.name) : []); }
    _write(value, key, noOverwrite) {
      const m = this._meta;
      if (m.keyPath !== null && key !== undefined) throw dataErr('A key was given for a store with a key path.');
      if (m.keyPath === null && !m.auto && key === undefined) throw dataErr('No key was given.');
      const a = Object.assign(this._base(), { value: ser(value, []), key: key === undefined ? null : enc(key), no_overwrite: noOverwrite });
      return this._req(true, () => dec(call('put', a)));
    }
    add(value, key) { return this._write(value, key, true); }
    put(value, key) { return this._write(value, key, false); }
    delete(q) {
      if (q === undefined) throw dataErr();
      const a = Object.assign(this._base(), { range: rangeWire(q) });
      return this._req(true, () => { call('delete', a); return undefined; });
    }
    clear() { const a = this._base(); return this._req(true, () => { call('clear', a); return undefined; }); }
    index(name) {
      name = String(name);
      const m = this._meta;
      if (!m || !m.indexes.some((i) => i.name === name)) throw new DOMException("The index '" + name + "' does not exist.", 'NotFoundError');
      return new IDBIndex(this, name);
    }
    createIndex(name, keyPath, opts) {
      const tx = this._tx;
      if (tx.mode !== 'versionchange' || tx._done) throw new DOMException('Indexes can only be created during an upgrade.', 'InvalidStateError');
      opts = opts || {};
      if (typeof keyPath !== 'string' && !Array.isArray(keyPath)) throw new DOMException('The key path is not valid.', 'SyntaxError');
      if (Array.isArray(keyPath) && opts.multiEntry) throw new DOMException('A compound key path cannot be multiEntry.', 'InvalidAccessError');
      call('create_index', { db: tx.db.name, store: this._storeName, name: String(name), keyPath, unique: !!opts.unique, multi: !!opts.multiEntry, tx: tx._id });
      tx.db._refresh();
      return new IDBIndex(this, String(name));
    }
    deleteIndex(name) {
      const tx = this._tx;
      if (tx.mode !== 'versionchange' || tx._done) throw new DOMException('Indexes can only be deleted during an upgrade.', 'InvalidStateError');
      call('delete_index', { db: tx.db.name, store: this._storeName, name: String(name), tx: tx._id });
      tx.db._refresh();
    }
  }

  class IDBIndex extends Reader {
    constructor(store, name) { super(); hide(this, '_tx', store._tx); hide(this, '_storeName', store._storeName); hide(this, '_indexName', name); this.objectStore = store; }
    get _meta() { return this.objectStore._meta.indexes.find((i) => i.name === this._indexName) || {}; }
    get name() { return this._indexName; }
    get keyPath() { return this._meta.keyPath; }
    get unique() { return !!this._meta.unique; }
    get multiEntry() { return !!this._meta.multi; }
  }

  class IDBCursor {
    constructor(source, req, args, dir, keysOnly) {
      this.source = source; this.direction = dir; this.key = undefined; this.primaryKey = undefined; this.request = req;
      hide(this, '_req', req); hide(this, '_a', args); hide(this, '_pos', null); hide(this, '_got', false); hide(this, '_keysOnly', keysOnly);
    }
    _go(target, tp, n) {
      if (!this._got) throw new DOMException('The cursor is being iterated or has iterated past its end.', 'InvalidStateError');
      const tx = this.source._tx;
      if (tx._done) throw new DOMException('The transaction has finished.', 'TransactionInactiveError');
      this._got = false; this._req.readyState = 'pending';
      schedule(tx, this._req, () => nextOf(this, target, tp, n));
    }
    continue(key) {
      let t = null;
      if (key !== undefined) {
        t = enc(key);
        const c = this._pos ? cmpw(t, this._pos[0]) : 1;
        if ((this.direction.startsWith('next') && c <= 0) || (this.direction.startsWith('prev') && c >= 0)) throw dataErr('The key is not past the cursor position.');
      }
      this._go(t, null, 1);
    }
    continuePrimaryKey(key, pk) { this._go(enc(key), enc(pk), 1); }
    advance(n) { n = Number(n); if (!(n >= 1)) throw new TypeError('The count must be greater than zero.'); this._go(null, null, Math.floor(n)); }
    update(value) {
      if (this._keysOnly || !this._got) throw new DOMException('The cursor cannot be used to update here.', 'InvalidStateError');
      const store = this.source instanceof IDBIndex ? this.source.objectStore : this.source;
      return store.put(value, store.keyPath === null ? dec(this._pos[1]) : undefined);
    }
    delete() {
      if (this._keysOnly || !this._got) throw new DOMException('The cursor cannot be used to delete here.', 'InvalidStateError');
      const store = this.source instanceof IDBIndex ? this.source.objectStore : this.source;
      return store.delete(dec(this._pos[1]));
    }
  }
  class IDBCursorWithValue extends IDBCursor {}
  function nextOf(cur, target, tp, n) {
    let r = null;
    for (let i = 0; i < n; i++) {
      r = call('cursor', Object.assign({}, cur._a, { pos: cur._pos, target: i === 0 ? target : null, tp: i === 0 ? tp : null }));
      if (!('key' in r)) { cur._got = true; cur._pos = null; return null; }
      cur._pos = [r.key, r.primary];
    }
    cur.key = dec(r.key); cur.primaryKey = dec(r.primary);
    if (!cur._keysOnly) cur.value = de(r.value);
    cur._got = true;
    return cur;
  }

  // ---------------------------------------------------------------- databases
  class IDBDatabase extends EventTarget {
    constructor(name, version) {
      super();
      this.name = name; this.version = version;
      this.onabort = null; this.onclose = null; this.onerror = null; this.onversionchange = null;
      hide(this, '_closed', false); hide(this, '_upgradeTx', null); hide(this, '_schema', { version, stores: [] });
    }
    _refresh() { try { this._schema = call('schema', { db: this.name }); } catch (_) {} }
    _storeNames() { return this._schema.stores.map((s) => s.name); }
    get objectStoreNames() { return names(this._storeNames()); }
    transaction(stores, mode) {
      if (this._closed) throw new DOMException('The database connection is closing.', 'InvalidStateError');
      const list = (Array.isArray(stores) ? stores : stores !== null && typeof stores === 'object' && typeof stores.length === 'number' ? Array.from(stores) : [stores]).map(String);
      mode = mode === undefined ? 'readonly' : String(mode);
      if (mode !== 'readonly' && mode !== 'readwrite') throw new TypeError("'" + mode + "' is not a valid transaction mode");
      if (!list.length) throw new DOMException('At least one object store is needed.', 'InvalidAccessError');
      const have = this._storeNames();
      for (const n of list) if (!have.includes(n)) throw new DOMException("The object store '" + n + "' does not exist.", 'NotFoundError');
      return new IDBTransaction(this, Array.from(new Set(list)).sort(), mode, 't' + (++txSeq));
    }
    createObjectStore(name, opts) {
      const tx = this._upgradeTx;
      if (!tx || tx._done) throw new DOMException('Object stores can only be created during an upgrade.', 'InvalidStateError');
      opts = opts || {};
      const kp = opts.keyPath === undefined || opts.keyPath === null ? null : opts.keyPath;
      if (kp !== null && typeof kp !== 'string' && !Array.isArray(kp)) throw new DOMException('The key path is not valid.', 'SyntaxError');
      call('create_store', { db: this.name, name: String(name), keyPath: kp, auto: !!opts.autoIncrement, tx: tx._id });
      this._refresh();
      return tx.objectStore(String(name));
    }
    deleteObjectStore(name) {
      const tx = this._upgradeTx;
      if (!tx || tx._done) throw new DOMException('Object stores can only be deleted during an upgrade.', 'InvalidStateError');
      call('delete_store', { db: this.name, name: String(name), tx: tx._id });
      this._refresh();
    }
    close() {
      this._closed = true;
      const s = conns.get(this.name);
      if (s) s.delete(this);
    }
  }

  function failOpen(req, e) {
    req.readyState = 'done'; req.result = undefined; req.error = e;
    fire(req, 'error', true, true);
  }
  function notify(name, except, oldV, newV) {
    // other open connections hear about a new version or a deletion, and may close
    let blocked = false;
    for (const c of Array.from(conns.get(name) || [])) {
      if (c === except || c._closed) continue;
      c.dispatchEvent(new IDBVersionChangeEvent('versionchange', { oldVersion: oldV, newVersion: newV }));
      if (!c._closed) blocked = true;
    }
    return blocked;
  }
  function runOpen(req, name, version) {
    const id = 't' + (++txSeq);
    let info;
    try { info = call('open', { name, version: version === undefined ? null : version, tx: id }); } catch (e) { failOpen(req, e); return; }
    const db = new IDBDatabase(name, info.new);
    db._refresh();
    if (!conns.has(name)) conns.set(name, new Set());
    conns.get(name).add(db);
    req.readyState = 'done'; req.result = db;
    if (!info.upgrade) { fire(req, 'success', false, false); return; }
    if (notify(name, db, info.old, info.new)) fire(req, 'blocked', false, false);
    const tx = new IDBTransaction(db, [], 'versionchange', id);
    db._upgradeTx = tx; req.transaction = tx; tx._wrote = true;
    tx._after = (aborted) => {
      db._upgradeTx = null; req.transaction = null;
      if (aborted) {
        db.close();
        failOpen(req, new DOMException('The upgrade transaction was aborted.', 'AbortError'));
      } else fire(req, 'success', false, false);
    };
    tx._pending++; // held while the upgrade handler runs
    const ev = new IDBVersionChangeEvent('upgradeneeded', { oldVersion: info.old, newVersion: info.new });
    try { req.dispatchEvent(ev); } catch (_) {}
    tx._pending--;
    setTimeout(() => maybeFinish(tx), 0);
  }

  class IDBFactory {
    open(name, version) {
      name = String(name);
      if (version !== undefined) {
        version = Number(version);
        if (!(version >= 1) || !Number.isFinite(version)) throw new TypeError('The version must be a positive number.');
        version = Math.floor(version);
      }
      const req = new IDBOpenDBRequest();
      setTimeout(() => runOpen(req, name, version), 0);
      return req;
    }
    deleteDatabase(name) {
      name = String(name);
      const req = new IDBOpenDBRequest();
      let old = 0;
      setTimeout(() => {
        try { old = call('databases').find((d) => d.name === name); old = old ? old.version : 0; } catch (e) { failOpen(req, e); return; }
        notify(name, null, old, null);
        for (const c of Array.from(conns.get(name) || [])) c._closed = true;
        conns.delete(name);
        call('delete_db', { name });
        req.readyState = 'done'; req.result = undefined;
        fire(req, 'success', false, false);
      }, 0);
      const origFire = req.dispatchEvent.bind(req);
      req.dispatchEvent = (ev) => {
        if (ev.type === 'success') { const e2 = new IDBVersionChangeEvent('success', { oldVersion: old, newVersion: null }); return origFire(e2); }
        return origFire(ev);
      };
      return req;
    }
    databases() { return new Promise((res, rej) => setTimeout(() => { try { res(call('databases')); } catch (e) { rej(e); } }, 0)); }
    cmp(a, b) { return cmpw(enc(a), enc(b)); }
  }

  const exported = { IDBFactory, IDBRequest, IDBOpenDBRequest, IDBDatabase, IDBTransaction, IDBObjectStore, IDBIndex, IDBCursor, IDBCursorWithValue, IDBKeyRange, IDBVersionChangeEvent };
  for (const k of Object.keys(exported)) Object.defineProperty(g, k, { value: exported[k], writable: true, enumerable: false, configurable: true });
  Object.defineProperty(g, 'indexedDB', { value: new IDBFactory(), writable: true, enumerable: false, configurable: true });
})(globalThis);

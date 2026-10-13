//! `indexedDB`: the database engine lives here (keys, ordering, stores, indexes, ranges, cursors, quota, undo for aborted
//! transactions, saving and restoring); `idb.js` is the thin layer that gives pages the `IDB*` objects, requests and
//! events, and delivers every callback through the timers.
//!
//! Values reach this side already "structured-cloned" into JSON: dates, typed arrays and the like are small tagged
//! objects (`{"$idb":"date","v":ms}`), which the engine stores without looking inside, except to find key paths.
//! Keys travel as `[tag, value]`: `[0,number]`, `[1,date ms]`, `[2,string]`, `[3,hex bytes]`, `[4,[keys]]`.

use std::any::Any;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::engine::WebModule;

/// Per-origin quota for all databases together, as for `localStorage`.
pub const QUOTA_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Key {
    Min,
    Num(f64),
    Date(f64),
    Str(String),
    Bin(String),
    Arr(Vec<Key>),
    Max,
}

impl Key {
    fn rank(&self) -> u8 {
        match self {
            Key::Min => 0,
            Key::Num(_) => 1,
            Key::Date(_) => 2,
            Key::Str(_) => 3,
            Key::Bin(_) => 4,
            Key::Arr(_) => 5,
            Key::Max => 6,
        }
    }

    fn from_wire(v: &Value) -> Option<Key> {
        let a = v.as_array()?;
        let x = a.get(1)?;
        let num = |x: &Value| -> Option<f64> {
            match x {
                Value::String(s) => match s.as_str() {
                    "Infinity" => Some(f64::INFINITY),
                    "-Infinity" => Some(f64::NEG_INFINITY),
                    _ => None,
                },
                _ => x.as_f64().filter(|n| !n.is_nan()),
            }
        };
        Some(match a.first()?.as_u64()? {
            0 => Key::Num(num(x)? + 0.0),
            1 => Key::Date(num(x)? + 0.0),
            2 => Key::Str(x.as_str()?.to_string()),
            3 => Key::Bin(x.as_str()?.to_string()),
            4 => Key::Arr(x.as_array()?.iter().map(Key::from_wire).collect::<Option<Vec<_>>>()?),
            _ => return None,
        })
    }

    fn to_wire(&self) -> Value {
        let n = |n: f64| if n.is_infinite() { json!(if n > 0.0 { "Infinity" } else { "-Infinity" }) } else { json!(n) };
        match self {
            Key::Num(x) => json!([0, n(*x)]),
            Key::Date(x) => json!([1, n(*x)]),
            Key::Str(s) => json!([2, s]),
            Key::Bin(s) => json!([3, s]),
            Key::Arr(a) => json!([4, a.iter().map(Key::to_wire).collect::<Vec<_>>()]),
            Key::Min | Key::Max => Value::Null,
        }
    }

    /// A key from a stored (cloned) value, if the value can be a key.
    fn of_value(v: &Value) -> Option<Key> {
        match v {
            Value::Number(n) => n.as_f64().filter(|x| x.is_finite()).map(|x| Key::Num(x + 0.0)),
            Value::String(s) => Some(Key::Str(s.clone())),
            Value::Array(a) => a.iter().map(Key::of_value).collect::<Option<Vec<_>>>().map(Key::Arr),
            Value::Object(o) if o.get("$idb").and_then(Value::as_str) == Some("date") => o.get("v").and_then(Value::as_f64).map(|x| Key::Date(x + 0.0)),
            _ => None,
        }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Key) -> Ordering {
        match (self, other) {
            (Key::Num(a), Key::Num(b)) | (Key::Date(a), Key::Date(b)) => a.total_cmp(b),
            (Key::Str(a), Key::Str(b)) => a.encode_utf16().cmp(b.encode_utf16()),
            (Key::Bin(a), Key::Bin(b)) => a.cmp(b),
            (Key::Arr(a), Key::Arr(b)) => a.cmp(b),
            (a, b) => a.rank().cmp(&b.rank()),
        }
    }
}

fn path_get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    if path.is_empty() {
        return Some(cur);
    }
    for part in path.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

fn path_set(v: &mut Value, path: &str, x: Value) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for (i, part) in parts.iter().enumerate() {
        let Some(o) = cur.as_object_mut() else { return false };
        if i + 1 == parts.len() {
            o.insert((*part).to_string(), x);
            return true;
        }
        cur = o.entry((*part).to_string()).or_insert_with(|| json!({}));
    }
    false
}

fn key_for_path(v: &Value, kp: &Value) -> Option<Key> {
    match kp {
        Value::String(p) => Key::of_value(path_get(v, p)?),
        Value::Array(ps) => ps.iter().map(|p| Key::of_value(path_get(v, p.as_str()?)?)).collect::<Option<Vec<_>>>().map(Key::Arr),
        _ => None,
    }
}

#[derive(Clone)]
struct Index {
    key_path: Value,
    unique: bool,
    multi: bool,
    /// Rebuilt from the records, never saved.
    set: BTreeSet<(Key, Key)>,
}

impl Index {
    fn keys_of(&self, v: &Value) -> Vec<Key> {
        if self.multi {
            if let Some(Value::Array(a)) = self.key_path.as_str().and_then(|p| path_get(v, p)) {
                let mut out: Vec<Key> = Vec::new();
                for k in a.iter().filter_map(Key::of_value) {
                    if !out.contains(&k) {
                        out.push(k);
                    }
                }
                return out;
            }
        }
        key_for_path(v, &self.key_path).into_iter().collect()
    }
}

#[derive(Clone)]
struct Store {
    key_path: Option<Value>,
    auto: bool,
    next: f64,
    records: BTreeMap<Key, (Value, usize)>,
    indexes: BTreeMap<String, Index>,
}

#[derive(Clone)]
struct Db {
    version: u64,
    stores: BTreeMap<String, Store>,
    bytes: usize,
}

type Fail = (&'static str, String);
type R = Result<Value, Fail>;

fn fail<T>(name: &'static str, msg: &str) -> Result<T, Fail> {
    Err((name, msg.to_string()))
}

impl Store {
    fn new(key_path: Option<Value>, auto: bool) -> Store {
        Store { key_path, auto, next: 1.0, records: BTreeMap::new(), indexes: BTreeMap::new() }
    }

    fn unindex(&mut self, key: &Key, value: &Value) {
        for ix in self.indexes.values_mut() {
            for ik in ix.keys_of(value) {
                ix.set.remove(&(ik, key.clone()));
            }
        }
    }

    fn put(&mut self, mut value: Value, explicit: Option<Key>, no_overwrite: bool, bytes: &mut usize, room: usize) -> Result<Key, Fail> {
        let key = match (&self.key_path, explicit) {
            (Some(_), Some(_)) => return fail("DataError", "a key was given for a store that takes its key from the value"),
            (Some(kp), None) => match key_for_path(&value, kp) {
                Some(k) => k,
                None if self.auto => {
                    let Some(p) = kp.as_str() else { return fail("DataError", "the key cannot be generated for a compound key path") };
                    if !path_set(&mut value, p, json!(self.next)) {
                        return fail("DataError", "the key could not be put into the value");
                    }
                    Key::Num(self.next)
                }
                None => return fail("DataError", "the value has no key at the key path"),
            },
            (None, Some(k)) => k,
            (None, None) if self.auto => Key::Num(self.next),
            (None, None) => return fail("DataError", "no key was given and the store does not generate one"),
        };
        if no_overwrite && self.records.contains_key(&key) {
            return fail("ConstraintError", "a record with this key already exists");
        }
        let new_keys: Vec<(String, Vec<Key>)> = self.indexes.iter().map(|(n, ix)| (n.clone(), ix.keys_of(&value))).collect();
        for (n, ks) in &new_keys {
            let ix = &self.indexes[n];
            if ix.unique {
                for ik in ks {
                    let clash = ix.set.range((Bound::Included((ik.clone(), Key::Min)), Bound::Included((ik.clone(), Key::Max)))).any(|(_, p)| *p != key);
                    if clash {
                        return fail("ConstraintError", &format!("the unique index '{n}' already has this key"));
                    }
                }
            }
        }
        let size = key.to_wire().to_string().len() + value.to_string().len() + 32;
        let old_size = self.records.get(&key).map_or(0, |(_, s)| *s);
        if *bytes - old_size + size + room > QUOTA_BYTES {
            return fail("QuotaExceededError", "the storage for this site is full");
        }
        if let Some((old, _)) = self.records.get(&key).cloned() {
            self.unindex(&key, &old);
        }
        for (n, ks) in new_keys {
            if let Some(ix) = self.indexes.get_mut(&n) {
                for ik in ks {
                    ix.set.insert((ik, key.clone()));
                }
            }
        }
        if let (true, Key::Num(n)) = (self.auto, &key) {
            if *n >= self.next {
                self.next = n.floor() + 1.0;
            }
        }
        *bytes = *bytes - old_size + size;
        self.records.insert(key.clone(), (value, size));
        Ok(key)
    }

    fn remove(&mut self, key: &Key, bytes: &mut usize) {
        if let Some((v, size)) = self.records.remove(key) {
            *bytes -= size;
            self.unindex(key, &v);
        }
    }

    fn rebuild_index(&mut self, name: &str) {
        let Some(mut ix) = self.indexes.remove(name) else { return };
        ix.set.clear();
        for (k, (v, _)) in &self.records {
            for ik in ix.keys_of(v) {
                ix.set.insert((ik, k.clone()));
            }
        }
        self.indexes.insert(name.to_string(), ix);
    }

    /// Entries `(index key, primary key)` within the bounds, up to `limit`.
    fn scan(&self, index: Option<&str>, lo: Bound<(Key, Key)>, hi: Bound<(Key, Key)>, rev: bool, limit: usize) -> Vec<(Key, Key)> {
        match index.and_then(|n| self.indexes.get(n)) {
            None => {
                let (l, h) = (lo.map(|p| p.0), hi.map(|p| p.0));
                if !valid(&l, &h) {
                    return Vec::new();
                }
                let it = self.records.range((l, h)).map(|(k, _)| (k.clone(), k.clone()));
                if rev { it.rev().take(limit).collect() } else { it.take(limit).collect() }
            }
            Some(ix) => {
                if !valid(&lo, &hi) {
                    return Vec::new();
                }
                let it = ix.set.range((lo, hi)).cloned();
                if rev { it.rev().take(limit).collect() } else { it.take(limit).collect() }
            }
        }
    }

    fn describe(&self) -> Value {
        json!({
            "keyPath": self.key_path.clone().unwrap_or(Value::Null), "auto": self.auto,
            "indexes": self.indexes.iter().map(|(n, ix)| json!({ "name": n, "keyPath": ix.key_path, "unique": ix.unique, "multi": ix.multi })).collect::<Vec<_>>(),
        })
    }
}

fn valid<T: Ord>(lo: &Bound<T>, hi: &Bound<T>) -> bool {
    match (val(lo), val(hi)) {
        (Some((x, xe)), Some((y, ye))) => match x.cmp(y) {
            Ordering::Less => true,
            Ordering::Equal => !xe && !ye,
            Ordering::Greater => false,
        },
        _ => true,
    }
}

fn val<T>(b: &Bound<T>) -> Option<(&T, bool)> {
    match b {
        Bound::Unbounded => None,
        Bound::Included(x) => Some((x, false)),
        Bound::Excluded(x) => Some((x, true)),
    }
}

fn max_lower<T: Ord>(a: Bound<T>, b: Bound<T>) -> Bound<T> {
    match (val(&a), val(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some((x, xe)), Some((y, _))) => match x.cmp(y) {
            Ordering::Less => b,
            Ordering::Greater => a,
            Ordering::Equal => {
                if xe { a } else { b }
            }
        },
    }
}

fn min_upper<T: Ord>(a: Bound<T>, b: Bound<T>) -> Bound<T> {
    match (val(&a), val(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some((x, xe)), Some((y, _))) => match x.cmp(y) {
            Ordering::Less => a,
            Ordering::Greater => b,
            Ordering::Equal => {
                if xe { a } else { b }
            }
        },
    }
}

/// A query (null, or `{l, u, lo, uo}`) as bounds over `(index key, primary key)`.
fn bounds(range: &Value) -> Result<(Bound<(Key, Key)>, Bound<(Key, Key)>), Fail> {
    if range.is_null() {
        return Ok((Bound::Unbounded, Bound::Unbounded));
    }
    let bad = || ("DataError", "the key range is not valid".to_string());
    let lo = match range.get("l").filter(|x| !x.is_null()) {
        None => Bound::Unbounded,
        Some(w) => {
            let k = Key::from_wire(w).ok_or_else(bad)?;
            if range["lo"].as_bool() == Some(true) { Bound::Excluded((k, Key::Max)) } else { Bound::Included((k, Key::Min)) }
        }
    };
    let hi = match range.get("u").filter(|x| !x.is_null()) {
        None => Bound::Unbounded,
        Some(w) => {
            let k = Key::from_wire(w).ok_or_else(bad)?;
            if range["uo"].as_bool() == Some(true) { Bound::Excluded((k, Key::Min)) } else { Bound::Included((k, Key::Max)) }
        }
    };
    Ok((lo, hi))
}

impl Db {
    fn to_json(&self) -> Value {
        let stores: serde_json::Map<String, Value> = self
            .stores
            .iter()
            .map(|(n, s)| {
                let ix: serde_json::Map<String, Value> = s.indexes.iter().map(|(n, i)| (n.clone(), json!({ "kp": i.key_path, "u": i.unique, "m": i.multi }))).collect();
                (n.clone(), json!({ "kp": s.key_path, "a": s.auto, "n": s.next, "i": ix, "r": s.records.iter().map(|(k, (v, _))| json!([k.to_wire(), v])).collect::<Vec<_>>() }))
            })
            .collect();
        json!({ "v": self.version, "s": stores })
    }

    fn from_json(v: &Value) -> Option<Db> {
        let mut db = Db { version: v["v"].as_u64()?, stores: BTreeMap::new(), bytes: 0 };
        for (name, s) in v["s"].as_object()? {
            let kp = s["kp"].clone();
            let mut st = Store::new(if kp.is_null() { None } else { Some(kp) }, s["a"].as_bool().unwrap_or(false));
            st.next = s["n"].as_f64().unwrap_or(1.0);
            for (iname, i) in s["i"].as_object()? {
                st.indexes.insert(iname.clone(), Index { key_path: i["kp"].clone(), unique: i["u"].as_bool().unwrap_or(false), multi: i["m"].as_bool().unwrap_or(false), set: BTreeSet::new() });
            }
            for r in s["r"].as_array()? {
                let key = Key::from_wire(&r[0])?;
                let size = r[0].to_string().len() + r[1].to_string().len() + 32;
                db.bytes += size;
                st.records.insert(key, (r[1].clone(), size));
            }
            let names: Vec<String> = st.indexes.keys().cloned().collect();
            for n in names {
                st.rebuild_index(&n);
            }
            db.stores.insert(name.clone(), st);
        }
        Some(db)
    }
}

/// All the databases of one origin.
#[derive(Default)]
pub struct IdbStore {
    dbs: BTreeMap<String, Db>,
    /// Undo points of running transactions: transaction id -> (database, how it was; `None` = it did not exist).
    snaps: HashMap<String, (String, Option<Db>)>,
}

impl IdbStore {
    pub fn is_empty(&self) -> bool {
        self.dbs.is_empty()
    }

    /// Everything as JSON, to be saved.
    pub fn to_json(&self) -> String {
        json!({ "dbs": self.dbs.iter().map(|(n, d)| (n.clone(), d.to_json())).collect::<serde_json::Map<_, _>>() }).to_string()
    }

    /// Put saved databases back; `None` when the text is not what `to_json` wrote.
    pub fn from_json(text: &str) -> Option<IdbStore> {
        let v: Value = serde_json::from_str(text).ok()?;
        let mut s = IdbStore::default();
        for (n, d) in v["dbs"].as_object()? {
            s.dbs.insert(n.clone(), Db::from_json(d)?);
        }
        Some(s)
    }

    /// Bytes used by every database (what the quota counts).
    pub fn bytes(&self) -> usize {
        self.dbs.values().map(|d| d.bytes).sum()
    }

    fn db_mut(&mut self, a: &Value) -> Result<&mut Db, Fail> {
        let name = a["db"].as_str().unwrap_or("");
        self.dbs.get_mut(name).ok_or(("NotFoundError", format!("no database '{name}'")))
    }

    /// Remember how the database was before the transaction's first change.
    fn touch(&mut self, a: &Value) {
        if let (Some(tx), Some(db)) = (a["tx"].as_str(), a["db"].as_str()) {
            if !self.snaps.contains_key(tx) {
                if let Some(d) = self.dbs.get(db) {
                    self.snaps.insert(tx.to_string(), (db.to_string(), Some(d.clone())));
                }
            }
        }
    }

    fn run(&mut self, op: &str, a: &Value) -> R {
        match op {
            "databases" => Ok(json!(self.dbs.iter().map(|(n, d)| json!({ "name": n, "version": d.version })).collect::<Vec<_>>())),
            "open" => {
                let name = a["name"].as_str().unwrap_or("").to_string();
                let want = a["version"].as_u64();
                let old = self.dbs.get(&name).map(|d| d.version);
                let new = match (old, want) {
                    (Some(o), Some(w)) if w < o => return fail("VersionError", "the requested version is lower than the existing one"),
                    (Some(o), None) => o,
                    (None, None) => 1,
                    (_, Some(w)) => w,
                };
                let upgrade = old != Some(new);
                if upgrade {
                    if let Some(tx) = a["tx"].as_str() {
                        self.snaps.insert(tx.to_string(), (name.clone(), self.dbs.get(&name).cloned()));
                    }
                    self.dbs.entry(name).or_insert_with(|| Db { version: 0, stores: BTreeMap::new(), bytes: 0 }).version = new;
                }
                Ok(json!({ "old": old.unwrap_or(0), "new": new, "upgrade": upgrade }))
            }
            "delete_db" => {
                let name = a["name"].as_str().unwrap_or("");
                Ok(json!(self.dbs.remove(name).map_or(0, |d| d.version)))
            }
            "schema" => {
                let db = self.db_mut(a)?;
                Ok(json!({ "version": db.version, "stores": db.stores.iter().map(|(n, s)| { let mut d = s.describe(); d["name"] = json!(n); d }).collect::<Vec<_>>() }))
            }
            "rollback" => {
                if let Some((name, old)) = a["tx"].as_str().and_then(|t| self.snaps.remove(t)) {
                    match old {
                        Some(d) => {
                            self.dbs.insert(name, d);
                        }
                        None => {
                            self.dbs.remove(&name);
                        }
                    }
                }
                Ok(Value::Null)
            }
            "release" => {
                if let Some(t) = a["tx"].as_str() {
                    self.snaps.remove(t);
                }
                Ok(Value::Null)
            }
            "create_store" => {
                self.touch(a);
                let db = self.db_mut(a)?;
                let name = a["name"].as_str().unwrap_or("").to_string();
                if db.stores.contains_key(&name) {
                    return fail("ConstraintError", "an object store with this name already exists");
                }
                let kp = a["keyPath"].clone();
                let auto = a["auto"].as_bool().unwrap_or(false);
                if auto && (kp.is_array() || kp.as_str() == Some("")) {
                    return fail("InvalidAccessError", "a generated key needs a plain key path");
                }
                db.stores.insert(name, Store::new(if kp.is_null() { None } else { Some(kp) }, auto));
                Ok(Value::Null)
            }
            "delete_store" => {
                self.touch(a);
                let db = self.db_mut(a)?;
                let st = db.stores.remove(a["name"].as_str().unwrap_or("")).ok_or(("NotFoundError", "no such object store".to_string()))?;
                db.bytes -= st.records.values().map(|(_, s)| *s).sum::<usize>();
                Ok(Value::Null)
            }
            "create_index" => {
                self.touch(a);
                let db = self.db_mut(a)?;
                let st = db.stores.get_mut(a["store"].as_str().unwrap_or("")).ok_or(("NotFoundError", "no such object store".to_string()))?;
                let name = a["name"].as_str().unwrap_or("").to_string();
                if st.indexes.contains_key(&name) {
                    return fail("ConstraintError", "an index with this name already exists");
                }
                let ix = Index { key_path: a["keyPath"].clone(), unique: a["unique"].as_bool().unwrap_or(false), multi: a["multi"].as_bool().unwrap_or(false), set: BTreeSet::new() };
                st.indexes.insert(name.clone(), ix);
                st.rebuild_index(&name);
                let dup = {
                    let ix = &st.indexes[&name];
                    ix.unique && ix.set.iter().zip(ix.set.iter().skip(1)).any(|(x, y)| x.0 == y.0)
                };
                if dup {
                    st.indexes.remove(&name);
                    return fail("ConstraintError", "existing records break the unique index");
                }
                Ok(Value::Null)
            }
            "delete_index" => {
                self.touch(a);
                let db = self.db_mut(a)?;
                let st = db.stores.get_mut(a["store"].as_str().unwrap_or("")).ok_or(("NotFoundError", "no such object store".to_string()))?;
                st.indexes.remove(a["name"].as_str().unwrap_or("")).ok_or(("NotFoundError", "no such index".to_string()))?;
                Ok(Value::Null)
            }
            "put" | "delete" | "clear" => {
                self.touch(a);
                let others: usize = {
                    let me = a["db"].as_str().unwrap_or("");
                    self.dbs.iter().filter(|(n, _)| n.as_str() != me).map(|(_, d)| d.bytes).sum()
                };
                let db = self.db_mut(a)?;
                let Db { stores, bytes, .. } = db;
                let st = stores.get_mut(a["store"].as_str().unwrap_or("")).ok_or(("NotFoundError", "no such object store".to_string()))?;
                match op {
                    "put" => {
                        let explicit = match a.get("key").filter(|k| !k.is_null()) {
                            Some(w) => Some(Key::from_wire(w).ok_or(("DataError", "not a valid key".to_string()))?),
                            None => None,
                        };
                        let k = st.put(a["value"].clone(), explicit, a["no_overwrite"].as_bool().unwrap_or(false), bytes, others)?;
                        Ok(k.to_wire())
                    }
                    "delete" => {
                        let (lo, hi) = bounds(&a["range"])?;
                        let keys: Vec<Key> = st.scan(None, lo, hi, false, usize::MAX).into_iter().map(|(k, _)| k).collect();
                        for k in keys {
                            st.remove(&k, bytes);
                        }
                        Ok(Value::Null)
                    }
                    _ => {
                        *bytes -= st.records.values().map(|(_, s)| *s).sum::<usize>();
                        st.records.clear();
                        for ix in st.indexes.values_mut() {
                            ix.set.clear();
                        }
                        Ok(Value::Null)
                    }
                }
            }
            "get" | "get_key" | "all" | "count" | "cursor" => {
                let db = self.db_mut(a)?;
                let st = db.stores.get(a["store"].as_str().unwrap_or("")).ok_or(("NotFoundError", "no such object store".to_string()))?;
                let index = a["index"].as_str();
                if let Some(i) = index {
                    if !st.indexes.contains_key(i) {
                        return fail("NotFoundError", "no such index");
                    }
                }
                let (lo, hi) = bounds(&a["range"])?;
                let value_of = |pk: &Key| st.records.get(pk).map_or(Value::Null, |(v, _)| v.clone());
                match op {
                    "get" | "get_key" => Ok(match st.scan(index, lo, hi, false, 1).first() {
                        Some((_, pk)) if op == "get" => json!({ "v": value_of(pk) }),
                        Some((_, pk)) => json!({ "v": pk.to_wire() }),
                        None => json!({}),
                    }),
                    "all" => {
                        let limit = a["count"].as_u64().filter(|n| *n > 0).map_or(usize::MAX, |n| n as usize);
                        let es = st.scan(index, lo, hi, false, limit);
                        Ok(Value::Array(es.iter().map(|(_, pk)| if a["keys"].as_bool() == Some(true) { pk.to_wire() } else { value_of(pk) }).collect()))
                    }
                    "count" => Ok(json!(st.scan(index, lo, hi, false, usize::MAX).len())),
                    _ => {
                        let dir = a["dir"].as_str().unwrap_or("next");
                        let (rev, unique) = (dir.starts_with("prev"), dir.ends_with("unique"));
                        let pos = match a.get("pos").filter(|p| !p.is_null()) {
                            Some(p) => Some((Key::from_wire(&p[0]), Key::from_wire(&p[1]))).and_then(|(x, y)| Some((x?, y?))),
                            None => None,
                        };
                        let target = a.get("target").filter(|t| !t.is_null()).and_then(Key::from_wire);
                        let tp = a.get("tp").filter(|t| !t.is_null()).and_then(Key::from_wire);
                        let found = if !rev {
                            let mut lo = lo;
                            if let Some((ik, pk)) = pos {
                                lo = max_lower(lo, Bound::Excluded(if unique { (ik, Key::Max) } else { (ik, pk) }));
                            }
                            if let Some(t) = target {
                                lo = max_lower(lo, Bound::Included((t, tp.unwrap_or(Key::Min))));
                            }
                            st.scan(index, lo, hi, false, 1)
                        } else {
                            let mut hi = hi;
                            if let Some((ik, pk)) = pos {
                                hi = min_upper(hi, Bound::Excluded(if unique { (ik, Key::Min) } else { (ik, pk) }));
                            }
                            if let Some(t) = target {
                                hi = min_upper(hi, Bound::Included((t, tp.unwrap_or(Key::Max))));
                            }
                            let mut f = st.scan(index, lo, hi, true, 1);
                            if let (true, Some((ik, _))) = (unique, f.first().cloned()) {
                                f = st.scan(index, Bound::Included((ik.clone(), Key::Min)), Bound::Included((ik, Key::Max)), false, 1);
                            }
                            f
                        };
                        Ok(match found.first() {
                            None => json!({}),
                            Some((ik, pk)) => {
                                let mut r = json!({ "key": ik.to_wire(), "primary": pk.to_wire() });
                                if a["keys_only"].as_bool() != Some(true) {
                                    r["value"] = value_of(pk);
                                }
                                r
                            }
                        })
                    }
                }
            }
            other => Err(("TypeError", format!("indexedDB has no operation {other}"))),
        }
    }
}

/// `indexedDB` for one page, working on the origin's databases.
pub struct IdbModule {
    store: Arc<Mutex<IdbStore>>,
}

impl IdbModule {
    pub fn new(store: Arc<Mutex<IdbStore>>) -> IdbModule {
        // transactions of an earlier page are over
        if let Ok(mut s) = store.lock() {
            s.snaps.clear();
        }
        IdbModule { store }
    }
}

impl WebModule for IdbModule {
    fn name(&self) -> &'static str {
        "indexedDB"
    }

    fn prelude(&self) -> &'static str {
        include_str!("idb.js")
    }

    fn call(&mut self, op: &str, args: &Value) -> Result<Value, String> {
        let mut s = self.store.lock().map_err(|_| "storage is unavailable".to_string())?;
        Ok(match s.run(op, args) {
            Ok(v) => json!({ "ok": v }),
            Err((name, msg)) => json!({ "err": [name, msg] }),
        })
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::{run_page, Limits, SiteData, Sites};
    use iyakki_web::{Fetch, Response, Url};
    use std::sync::Arc;

    struct Nothing;
    impl Fetch for Nothing {
        fn get(&self, _: &Url) -> Result<Response, String> {
            Err("offline".into())
        }
    }

    /// Run one page's script against the given site data; what it logged, and the page's own notes.
    fn page(origin: &str, script: &str, data: &SiteData) -> Vec<String> {
        let url = Url::parse(origin).unwrap();
        let html = format!("<script>{script}</script>");
        let r = run_page(&html, &url, Arc::new(Nothing), |_| true, data, Limits::default());
        assert_eq!(r.report.failed, 0, "{:?}", r.report.notes);
        r.report.console.iter().map(|l| l.text.clone()).collect()
    }

    /// The promise wrapper every library puts around requests (what `idb` does).
    const WRAP: &str = "const p = (r) => new Promise((ok, no) => { r.addEventListener('success', () => ok(r.result)); r.addEventListener('error', () => no(r.error)); });
        const done = (t) => new Promise((ok, no) => { t.addEventListener('complete', () => ok()); t.addEventListener('error', (e) => no(e.target.error)); t.addEventListener('abort', () => no(t.error || new Error('aborted'))); });
        const open = (name, v, up) => { const r = indexedDB.open(name, v); r.onupgradeneeded = (e) => up(r.result, e, r.transaction); return p(r); };";

    #[test]
    fn upgrade_put_get_and_cursor_work_with_plain_callbacks() {
        let data = SiteData::default();
        let out = page(
            "https://a.org/",
            "const log = (...a) => console.log(a.join(' '));
             const rq = indexedDB.open('shop', 1);
             rq.onupgradeneeded = (e) => {
               log('upgrade', e.oldVersion, e.newVersion);
               const db = rq.result;
               const s = db.createObjectStore('items', { keyPath: 'id' });
               s.createIndex('by_cat', 'cat');
               s.add({ id: 1, name: 'pen', cat: 'office' });
               s.add({ id: 3, name: 'desk', cat: 'office' });
               s.add({ id: 2, name: 'apple', cat: 'food' });
             };
             rq.onsuccess = () => {
               const db = rq.result; log('open', db.name, db.version, Array.from(db.objectStoreNames).join());
               const t = db.transaction('items', 'readwrite');
               const s = t.objectStore('items');
               s.get(2).onsuccess = (e) => log('get', e.target.result.name);
               s.get(99).onsuccess = (e) => log('missing', String(e.target.result));
               s.count().onsuccess = (e) => log('count', e.target.result);
               s.getAll(IDBKeyRange.bound(2, 3)).onsuccess = (e) => log('range', e.target.result.map((x) => x.name).join());
               s.index('by_cat').getAll('office').onsuccess = (e) => log('index', e.target.result.map((x) => x.id).join());
               s.index('by_cat').getAllKeys().onsuccess = (e) => log('ixkeys', e.target.result.join());
               const names = [];
               s.openCursor(null, 'prev').onsuccess = (e) => { const c = e.target.result; if (c) { names.push(c.key + c.value.name); c.continue(); } else log('cursor', names.join()); };
               s.put({ id: 2, name: 'pear', cat: 'food' }).onsuccess = (e) => log('put key', e.target.result);
               s.delete(1);
               s.add({ id: 3 }).onerror = (e) => { log('dup', e.target.error.name); e.preventDefault(); };
               t.oncomplete = () => log('complete');
             };",
            &data,
        );
        assert_eq!(
            out,
            vec![
                "upgrade 0 1", "open shop 1 items", "get apple", "missing undefined", "count 3", "range apple,desk", "index 1,3", "ixkeys 2,1,3", "put key 2", "dup ConstraintError", "cursor 3desk,2pear", "complete"
            ]
        );
    }

    #[test]
    fn idb_style_promise_wrappers_and_async_functions_work_and_survive_a_reload_with_a_new_version() {
        let data = SiteData::default();
        let first = format!(
            "{WRAP}
             (async () => {{
               const db = await open('app', 1, (db) => {{ const s = db.createObjectStore('kv'); s.put({{ n: 1 }}, 'a'); }});
               let t = db.transaction('kv', 'readwrite');
               await p(t.objectStore('kv').put({{ n: 2, when: new Date(86400000), bytes: new Uint8Array([1, 2, 250]) }}, 'b'));
               const a = await p(t.objectStore('kv').get('a'));
               await p(t.objectStore('kv').put({{ n: a.n + 10 }}, 'c'));
               await done(t);
               console.log('first', (await p(db.transaction('kv').objectStore('kv').getAllKeys())).join());
               db.close();
             }})().catch((e) => console.log('failed ' + e));"
        );
        assert_eq!(page("https://a.org/", &first, &data), vec!["first a,b,c"]);
        let second = format!(
            "{WRAP}
             (async () => {{
               const db = await open('app', 3, (db, e, tx) => {{ console.log('upgrade ' + e.oldVersion + '->' + e.newVersion + ' ' + Array.from(db.objectStoreNames)); tx.objectStore('kv').createIndex('by_n', 'n'); }});
               const s = db.transaction('kv').objectStore('kv');
               const b = await p(s.get('b'));
               console.log(b.when instanceof Date, b.when.getTime(), b.bytes instanceof Uint8Array, Array.from(b.bytes).join('.'));
               console.log((await p(s.index('by_n').getAll(IDBKeyRange.lowerBound(2)))).map((x) => x.n).join());
               console.log((await indexedDB.databases()).map((d) => d.name + d.version).join());
               try {{ await open('app', 2, () => {{}}); }} catch (e) {{ console.log('older: ' + e.name); }}
             }})().catch((e) => console.log('failed ' + e));"
        );
        assert_eq!(page("https://a.org/", &second, &data), vec!["upgrade 1->3 kv", "true 86400000 true 1.2.250", "2,11", "app3", "older: VersionError"]);
    }

    #[test]
    fn keys_sort_numbers_then_dates_strings_binary_arrays() {
        let data = SiteData::default();
        let out = page(
            "https://a.org/",
            &format!(
                "{WRAP}
                 (async () => {{
                   const db = await open('k', 1, (db) => {{ db.createObjectStore('s'); }});
                   const t = db.transaction('s', 'readwrite'); const s = t.objectStore('s');
                   const keys = [[1, 2], 'b', new Uint8Array([9]), new Date(5), 10, -Infinity, 'a', [1], 2.5, new Date(1), 'B', [0, 'x'], new Uint8Array([1, 2])];
                   keys.forEach((k, i) => s.put(i, k));
                   const all = await p(s.getAllKeys());
                   console.log(all.map((k) => k instanceof ArrayBuffer ? 'bin' + Array.from(new Uint8Array(k)) : k instanceof Date ? 'd' + k.getTime() : Array.isArray(k) ? '[' + k + ']' : typeof k + k).join(' '));
                   console.log(indexedDB.cmp(1, 'a'), indexedDB.cmp('a', 1), indexedDB.cmp([1], [1, 0]), indexedDB.cmp(2, 2));
                   try {{ s.put(1, NaN); }} catch (e) {{ console.log(e.name); }}
                   try {{ s.put(1, {{}}); }} catch (e) {{ console.log(e.name); }}
                   console.log(IDBKeyRange.bound(1, 5, true).includes(1), IDBKeyRange.upperBound('z').includes(3));
                 }})();"
            ),
            &data,
        );
        assert_eq!(
            out,
            vec!["number-Infinity number2.5 number10 d1 d5 stringB stringa stringb bin1,2 bin9 [0,x] [1] [1,2]", "-1 1 -1 0", "DataError", "DataError", "false true"]
        );
    }

    #[test]
    fn cursors_walk_indexes_in_both_directions_skip_advance_update_and_delete() {
        let data = SiteData::default();
        let out = page(
            "https://a.org/",
            &format!(
                "{WRAP}
                 (async () => {{
                   const db = await open('c', 1, (db) => {{ const s = db.createObjectStore('s', {{ autoIncrement: true }}); s.createIndex('v', 'v'); s.createIndex('tags', 'tags', {{ multiEntry: true }}); for (const v of [5, 3, 5, 1, 3, 9]) s.add({{ v, tags: ['t' + v, 'all'] }}); }});
                   const t = db.transaction('s', 'readwrite'); const s = t.objectStore('s');
                   const walk = (src, range, dir, f) => new Promise((ok) => {{ const out = []; const r = src.openCursor(range, dir); r.onsuccess = () => {{ const c = r.result; if (!c) return ok(out.join(' ')); out.push(f ? f(c) : c.key + ':' + c.primaryKey); c.continue(); }}; }});
                   console.log(await walk(s.index('v'), null, 'next'));
                   console.log(await walk(s.index('v'), null, 'prev'));
                   console.log(await walk(s.index('v'), null, 'nextunique'));
                   console.log(await walk(s.index('v'), null, 'prevunique'));
                   console.log(await walk(s.index('v'), IDBKeyRange.bound(3, 5, false, true), 'next'));
                   console.log(await p(s.index('tags').count('all')), await p(s.index('tags').count('t3')));
                   console.log(await walk(s, IDBKeyRange.lowerBound(4), 'next', (c) => c.value.v));
                   const r = s.index('v').openCursor(); let steps = [];
                   await new Promise((ok) => {{ r.onsuccess = () => {{ const c = r.result; if (!c) return ok(); steps.push(c.key); if (c.key === 1) c.advance(2); else if (c.key === 3 && steps.length < 4) c.continue(9); else c.continue(); }}; }});
                   console.log(steps.join());
                   const u = s.openCursor(); await new Promise((ok) => {{ u.onsuccess = () => {{ const c = u.result; if (!c) return ok(); if (c.value.v === 5) c.update({{ v: 50 }}); else if (c.value.v === 1) c.delete(); c.continue(); }}; }});
                   console.log((await p(s.getAll())).map((x) => x.v).join(), await p(s.count()));
                   const kc = s.index('v').openKeyCursor(); console.log(await new Promise((ok) => {{ kc.onsuccess = () => ok(kc.result.key + ' ' + kc.result.primaryKey + ' ' + ('value' in kc.result)); }}));
                 }})().catch((e) => console.log('failed ' + e));"
            ),
            &data,
        );
        assert_eq!(
            out,
            vec![
                "1:4 3:2 3:5 5:1 5:3 9:6", "9:6 5:3 5:1 3:5 3:2 1:4", "1:4 3:2 5:1 9:6", "9:6 5:1 3:2 1:4", "3:2 3:5", "6 2", "1 3 9", "1,3,9", "50,3,50,3,9 5", "3 2 false"
            ]
        );
    }

    #[test]
    fn a_failed_request_aborts_the_transaction_and_undoes_it_and_unique_indexes_hold() {
        let data = SiteData::default();
        let out = page(
            "https://a.org/",
            &format!(
                "{WRAP}
                 (async () => {{
                   const db = await open('u', 1, (db) => {{ const s = db.createObjectStore('users', {{ keyPath: 'id' }}); s.createIndex('email', 'email', {{ unique: true }}); s.add({{ id: 1, email: 'a@x' }}); }});
                   let t = db.transaction('users', 'readwrite');
                   t.objectStore('users').add({{ id: 2, email: 'b@x' }});
                   t.objectStore('users').add({{ id: 3, email: 'a@x' }});
                   try {{ await done(t); console.log('no'); }} catch (e) {{ console.log('aborted', e.name); }}
                   console.log(await p(db.transaction('users').objectStore('users').count()));
                   t = db.transaction('users', 'readwrite'); t.objectStore('users').add({{ id: 9, email: 'z@x' }}); t.abort();
                   try {{ await done(t); }} catch (e) {{ console.log('explicit', e.message); }}
                   console.log(await p(db.transaction('users').objectStore('users').count()));
                   try {{ db.transaction('users').objectStore('users').put({{ id: 1 }}); }} catch (e) {{ console.log(e.name); }}
                   try {{ db.transaction('nope'); }} catch (e) {{ console.log(e.name); }}
                   const u = indexedDB.open('u2', 1); u.onupgradeneeded = () => {{ u.result.createObjectStore('x').put(1, 'k'); u.transaction.abort(); }};
                   u.onerror = () => console.log('upgrade aborted', u.error.name);
                   await new Promise((ok) => setTimeout(ok, 5));
                   console.log((await indexedDB.databases()).map((d) => d.name).join());
                 }})().catch((e) => console.log('failed ' + e));"
            ),
            &data,
        );
        assert_eq!(out, vec!["aborted ConstraintError", "1", "explicit The transaction was aborted.", "1", "ReadOnlyError", "NotFoundError", "upgrade aborted AbortError", "u"]);
    }

    #[test]
    fn auto_increment_key_paths_delete_database_and_quota() {
        let data = SiteData::default();
        let out = page(
            "https://a.org/",
            &format!(
                "{WRAP}
                 (async () => {{
                   const db = await open('q', 1, (db) => {{ db.createObjectStore('a', {{ keyPath: 'id', autoIncrement: true }}); db.createObjectStore('b', {{ autoIncrement: true }}); }});
                   const t = db.transaction(['a', 'b'], 'readwrite');
                   const ids = [await p(t.objectStore('a').add({{ x: 1 }})), await p(t.objectStore('a').add({{ id: 10 }})), await p(t.objectStore('a').add({{ x: 2 }})), await p(t.objectStore('b').add('s')), await p(t.objectStore('b').put('t', 'k')), await p(t.objectStore('b').add('u'))];
                   console.log(ids.join(), (await p(t.objectStore('a').get(1))).id);
                   await p(t.objectStore('a').clear()); console.log(await p(t.objectStore('a').count()));
                   await done(t);
                   const big = 'x'.repeat(100000); let n = 0, err = '';
                   const t2 = db.transaction('b', 'readwrite');
                   await new Promise((ok) => {{ const go = () => {{ const r = t2.objectStore('b').add(big); r.onsuccess = () => {{ n++; go(); }}; r.onerror = (e) => {{ err = r.error.name; e.preventDefault(); ok(); }}; }}; go(); }});
                   console.log(err, n > 40 && n < 60);
                   db.close();
                   await new Promise((ok) => {{ const r = indexedDB.deleteDatabase('q'); r.onsuccess = (e) => {{ console.log('deleted', e.oldVersion, e.newVersion); ok(); }}; }});
                   console.log((await indexedDB.databases()).length);
                 }})().catch((e) => console.log('failed ' + e));"
            ),
            &data,
        );
        assert_eq!(out, vec!["1,10,11,1,k,2 1", "0", "QuotaExceededError true", "deleted 1 null", "0"]);
    }

    #[test]
    fn origins_are_isolated_and_databases_can_be_saved_restored_and_forgotten() {
        let sites = Sites::default();
        let write = "const r = indexedDB.open('d', 1); r.onupgradeneeded = () => { r.result.createObjectStore('s', { keyPath: 'k' }).createIndex('i', 'v'); }; r.onsuccess = () => { const t = r.result.transaction('s', 'readwrite'); t.objectStore('s').put({ k: 'one', v: [1, new Date(7)], u: new Uint8Array([4]) }); t.objectStore('s').put({ k: 'two', v: 2 }); };";
        let read = "const r = indexedDB.open('d'); r.onsuccess = () => { const s = r.result.transaction('s').objectStore('s'); s.getAll().onsuccess = (e) => console.log(r.result.version, e.target.result.map((x) => x.k).join()); s.index('i').getAllKeys(2).onsuccess = (e) => console.log('ix', e.target.result.join()); };";
        let a = Url::parse("https://a.org/").unwrap();
        let b = Url::parse("https://b.org/").unwrap();
        page("https://a.org/", write, &sites.for_origin(&a));
        let peek = "const r = indexedDB.open('d'); r.onsuccess = () => console.log('stores ' + r.result.objectStoreNames.length + ' v' + r.result.version);";
        assert_eq!(page("https://b.org/", peek, &sites.for_origin(&b)), vec!["stores 0 v1"], "another origin sees none of it");
        let saved = sites.snapshot_idb();
        assert_eq!(saved.len(), 2, "b.org now has an (empty) version-1 database");
        let a_json = saved.iter().find(|(o, _)| o == "https://a.org:443").unwrap().1.clone();
        let fresh = Sites::default();
        assert!(fresh.restore_idb("https://a.org:443", &a_json));
        assert!(!fresh.restore_idb("https://a.org:443", "not json"));
        assert_eq!(page("https://a.org/", read, &fresh.for_origin(&a)), vec!["1 one,two", "ix two"]);
        fresh.forget_host("a.org");
        assert!(fresh.snapshot_idb().is_empty());
    }

    #[test]
    fn broadcast_channels_reach_other_channels_of_the_same_name_in_the_page() {
        let out = page(
            "https://a.org/",
            "const a = new BroadcastChannel('x'), b = new BroadcastChannel('x'), c = new BroadcastChannel('y');
             b.onmessage = (e) => console.log('b got ' + JSON.stringify(e.data)); a.onmessage = () => console.log('a heard itself'); c.onmessage = () => console.log('c wrong');
             a.postMessage({ n: 1 }); const m = new MessageChannel(); m.port2.onmessage = (e) => console.log('port ' + e.data); m.port1.postMessage('hi');",
            &SiteData::default(),
        );
        assert_eq!(out, vec!["b got {\"n\":1}", "port hi"]);
    }
}

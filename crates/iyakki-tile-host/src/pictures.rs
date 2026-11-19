//! Pictures a tile asks for at run time (F-WASM-17): the `pictures` service domain, in front of any [`ServiceBackend`].
//!
//! A tile never holds picture memory of its own: the host's table ([`ImageTable`]) does, with the same caps whether a picture came from the
//! tile's bundle (F-WASM-16) or from a call. `pictures.load {handle, path}` asks the host's [`PictureSource`] for the decoded picture at a
//! path the person owns (the source checks the path and decodes; this crate has no decoder and no file access), and the table refuses what
//! breaks a cap; `pictures.drop {handle}` frees it; `pictures.list` says what the tile may draw. The tile then names the handle in an `Image`
//! operation as before. At most [`MAX_LOADS`] loads per session, so a tile cannot make the host decode without end.
//!
//! The manifest decides first, as for every domain: `pictures`, `pictures.*` or `pictures.load` must be listed, otherwise the call never
//! reaches here. Every other domain goes to the inner backend untouched.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::{ImageTable, ServiceBackend};

/// The table of one tile, shared between the route (which fills it on a call) and the renderer or exporter (which reads it).
pub type SharedTable = Arc<Mutex<ImageTable>>;

/// The most `pictures.load` calls one session may make.
pub const MAX_LOADS: u32 = 64;
/// The longest path a tile may name.
pub const MAX_PATH: usize = 1024;
/// The largest handle a tile may use (a bundle's handles are small numbers too).
pub const MAX_HANDLE: u32 = 1_000_000;

/// Where the pictures of a person come from: the host reads the file the person owns and decodes it. `(width, height, RGBA)`.
pub trait PictureSource: Send + Sync {
    fn fetch(&self, path: &str) -> Result<(i32, i32, Vec<u8>), String>;
}

pub struct PictureRoute {
    inner: Arc<dyn ServiceBackend>,
    table: SharedTable,
    source: Arc<dyn PictureSource>,
    loads: Mutex<u32>,
}

impl PictureRoute {
    pub fn new(inner: Arc<dyn ServiceBackend>, table: SharedTable, source: Arc<dyn PictureSource>) -> PictureRoute {
        PictureRoute { inner, table, source, loads: Mutex::new(0) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ImageTable> {
        self.table.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn handle(v: &Value) -> Result<u32, String> {
        v.get("handle").and_then(Value::as_u64).filter(|h| (1..=MAX_HANDLE as u64).contains(h)).map(|h| h as u32).ok_or_else(|| format!("\"handle\" is a whole number from 1 to {MAX_HANDLE}"))
    }

    fn pictures(&self, method: &str, json: &str) -> Result<String, String> {
        let v: Value = serde_json::from_str(if json.trim().is_empty() { "{}" } else { json }).map_err(|_| "the arguments are not JSON".to_string())?;
        if !v.is_object() {
            return Err("the arguments are a JSON object".into());
        }
        match method {
            "load" => {
                let h = Self::handle(&v)?;
                let path = v.get("path").and_then(Value::as_str).filter(|p| !p.is_empty() && p.len() <= MAX_PATH && !p.contains('\0')).ok_or_else(|| format!("\"path\" is text of 1 to {MAX_PATH} bytes"))?;
                {
                    let mut n = self.loads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *n >= MAX_LOADS {
                        return Err(format!("this app has loaded {MAX_LOADS} pictures; that is as many as one session may"));
                    }
                    *n += 1;
                }
                let (w, hgt, rgba) = self.source.fetch(path)?;
                self.lock().put(h, w, hgt, rgba).map_err(|e| e.to_string())?;
                Ok(json!({"handle": h, "w": w, "h": hgt}).to_string())
            }
            "drop" => Ok(json!({"dropped": self.lock().remove(Self::handle(&v)?)}).to_string()),
            "list" => {
                let t = self.lock();
                let list: Vec<Value> = t.handles().into_iter().filter_map(|h| t.get(h).map(|p| json!({"handle": h, "w": p.w, "h": p.h}))).collect();
                Ok(Value::Array(list).to_string())
            }
            _ => Err("that is not a call this tile may make".into()),
        }
    }
}

impl ServiceBackend for PictureRoute {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String> {
        if domain == "pictures" {
            self.pictures(method, json)
        } else {
            self.inner.call(domain, method, json)
        }
    }

    fn denied(&self, domain: &str, method: &str, json: &str) {
        self.inner.denied(domain, method, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Photos;
    impl PictureSource for Photos {
        fn fetch(&self, path: &str) -> Result<(i32, i32, Vec<u8>), String> {
            match path {
                "photo.png" => Ok((2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255])),
                "huge.png" => Ok((2000, 1, vec![0; 8000])),
                "../etc/passwd" => Err("that is not a picture you can use".into()),
                _ => Err("no such picture".into()),
            }
        }
    }

    struct Echo;
    impl ServiceBackend for Echo {
        fn call(&self, d: &str, m: &str, _: &str) -> Result<String, String> {
            Ok(format!("{d}.{m}"))
        }
    }

    fn route() -> (PictureRoute, SharedTable) {
        let t: SharedTable = Arc::default();
        (PictureRoute::new(Arc::new(Echo), t.clone(), Arc::new(Photos)), t)
    }

    #[test]
    fn a_picture_is_loaded_by_the_host_into_the_same_table_and_dropped_again_and_other_domains_pass_through() {
        let (r, t) = route();
        assert_eq!(r.call("pictures", "load", r#"{"handle":7,"path":"photo.png"}"#).unwrap(), r#"{"h":1,"handle":7,"w":2}"#);
        assert_eq!(t.lock().unwrap().get(7).map(|p| (p.w, p.h, p.rgba.len())), Some((2, 1, 8)));
        assert_eq!(r.call("pictures", "list", "").unwrap(), r#"[{"h":1,"handle":7,"w":2}]"#);
        assert_eq!(r.call("pictures", "drop", r#"{"handle":7}"#).unwrap(), r#"{"dropped":true}"#);
        assert_eq!(r.call("pictures", "drop", r#"{"handle":7}"#).unwrap(), r#"{"dropped":false}"#);
        assert!(t.lock().unwrap().get(7).is_none());
        assert_eq!(r.call("storage", "get", "{}").unwrap(), "storage.get", "another domain is the inner backend's");
    }

    #[test]
    fn what_the_source_or_the_table_refuses_is_a_sentence_and_the_arguments_are_checked() {
        let (r, t) = route();
        assert!(r.call("pictures", "load", r#"{"handle":1,"path":"../etc/passwd"}"#).unwrap_err().contains("not a picture you can use"));
        assert!(r.call("pictures", "load", r#"{"handle":1,"path":"huge.png"}"#).unwrap_err().contains("larger than"), "the table's cap, same as for a bundle");
        assert!(t.lock().unwrap().get(1).is_none());
        for bad in [r#"{"path":"photo.png"}"#, r#"{"handle":0,"path":"photo.png"}"#, r#"{"handle":99999999,"path":"photo.png"}"#, r#"{"handle":1}"#, r#"{"handle":1,"path":""}"#, r#"{"handle":-1,"path":"a"}"#, "[1]", "nope"] {
            assert!(r.call("pictures", "load", bad).is_err(), "{bad}");
        }
        assert!(r.call("pictures", "save", "{}").unwrap_err().contains("not a call"));
    }

    #[test]
    fn a_session_loads_a_bounded_number_of_pictures() {
        let (r, _) = route();
        for i in 0..MAX_LOADS {
            let _ = r.call("pictures", "load", &format!(r#"{{"handle":{},"path":"photo.png"}}"#, i % 8 + 1));
        }
        assert!(r.call("pictures", "load", r#"{"handle":1,"path":"photo.png"}"#).unwrap_err().contains("as many as one session may"));
        assert_eq!(r.call("pictures", "list", "").map(|l| l.len() > 2), Ok(true), "listing and dropping are not loads");
    }
}

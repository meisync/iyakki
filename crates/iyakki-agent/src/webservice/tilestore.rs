//! The `storage` domain of a tile in a browser (F-SDK-34): a small key/value store per person and per tile id, kept in the service's data
//! folder (`tilestore/<person>__<tile>.json`, mode 0600), so a tile keeps its settings (the text size of EDIT) across visits and restarts.
//! A tile reaches only its own keys of its own person; the manifest must name `storage`; the audit lines name the call, never a key or a value.
//! Limits match the SDK's storage port: a key is 1 to 128 characters without control characters, a value is up to 64 KiB, at most 64 keys and
//! 256 KiB in all for one tile of one person (`too big` beyond that). The folder of an installed tile is cleared when the tile is removed.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_KEY: usize = 128;
pub const MAX_VALUE: usize = 64 * 1024;
pub const MAX_KEYS: usize = 64;
pub const MAX_TOTAL: usize = 256 * 1024;

/// One read-change-write at a time: two tiles of one person (or two requests) never lose each other's change.
static WRITE: Mutex<()> = Mutex::new(());

pub fn store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tilestore")
}

fn file_of(dir: &Path, user: &str, tile: &str) -> PathBuf {
    let hex: String = user.bytes().map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{hex}__{tile}.json"))
}

fn load(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(path: &Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = path.parent().ok_or("no folder")?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|_| "the tile's store cannot be kept".to_string())?;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|_| "the tile's store cannot be kept".to_string())?;
    f.write_all(&serde_json::to_vec(map).unwrap_or_default()).map_err(|_| "the tile's store cannot be kept".to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|_| "the tile's store cannot be kept".to_string())
}

fn key_of(args: &Value) -> Result<String, String> {
    let k = args.get("k").and_then(Value::as_str).ok_or("k is the key, a string")?;
    if k.is_empty() || k.len() > MAX_KEY || k.chars().any(char::is_control) {
        return Err("the key is 1 to 128 characters without control characters".into());
    }
    Ok(k.to_string())
}

/// `get {k}`, `set {k, v}`, `remove {k}`, `list {}` for this person's copy of this tile. The answers are what the tile's storage port reads:
/// `get` gives the string or null, `list` gives the keys, `set` and `remove` give `{ok:true}`; a limit says "too big".
pub fn call(dir: &Path, user: &str, tile: &str, method: &str, json: &str) -> Result<String, String> {
    let args: Value = serde_json::from_str(json).map_err(|_| "the arguments are not JSON".to_string())?;
    let _g = WRITE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = file_of(dir, user, tile);
    match method {
        "get" => Ok(load(&path).get(&key_of(&args)?).map(|v| Value::String(v.clone())).unwrap_or(Value::Null).to_string()),
        "list" => Ok(json!(load(&path).keys().collect::<Vec<_>>()).to_string()),
        "set" => {
            let k = key_of(&args)?;
            let v = args.get("v").and_then(Value::as_str).ok_or("v is the value, a string")?;
            if v.len() > MAX_VALUE {
                return Err("the value is too big (64 KiB at most)".into());
            }
            let mut map = load(&path);
            map.insert(k, v.to_string());
            let total: usize = map.iter().map(|(k, v)| k.len() + v.len()).sum();
            if map.len() > MAX_KEYS || total > MAX_TOTAL {
                return Err("the tile's store is too big (64 keys, 256 KiB)".into());
            }
            save(&path, &map)?;
            Ok(json!({"ok": true}).to_string())
        }
        "remove" => {
            let k = key_of(&args)?;
            let mut map = load(&path);
            if map.remove(&k).is_some() {
                save(&path, &map)?;
            }
            Ok(json!({"ok": true}).to_string())
        }
        _ => Err("unknown storage call".into()),
    }
}

/// Every person's store of one tile is deleted (the tile was removed).
pub fn clear_tile(dir: &Path, tile: &str) {
    let _g = WRITE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            // the person's hex name has no underscore, so the first "__" is the separator and what follows is exactly the tile id
            let name = e.file_name().to_string_lossy().into_owned();
            if name.strip_suffix(".json").and_then(|n| n.split_once("__")).is_some_and(|(_, t)| t == tile) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-tilestore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn g(d: &Path, u: &str, t: &str, k: &str) -> Value {
        serde_json::from_str(&call(d, u, t, "get", &json!({"k": k}).to_string()).unwrap()).unwrap()
    }

    fn set(d: &Path, u: &str, t: &str, k: &str, v: &str) -> Result<String, String> {
        call(d, u, t, "set", &json!({"k": k, "v": v}).to_string())
    }

    #[test]
    fn a_value_is_kept_in_a_file_and_read_back_after_a_restart_and_removed() {
        let d = dir("keep");
        set(&d, "ann", "edit-sdk", "text-size", "1200").unwrap();
        assert_eq!(g(&d, "ann", "edit-sdk", "text-size"), json!("1200"));
        // nothing is held in memory: a new call (as after a restart of the host) reads the file
        assert_eq!(call(&d, "ann", "edit-sdk", "list", "{}").unwrap(), r#"["text-size"]"#);
        use std::os::unix::fs::PermissionsExt;
        let file = std::fs::read_dir(&d).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        call(&d, "ann", "edit-sdk", "remove", r#"{"k":"text-size"}"#).unwrap();
        assert_eq!(g(&d, "ann", "edit-sdk", "text-size"), Value::Null);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn people_and_tiles_never_see_each_others_keys() {
        let d = dir("sep");
        set(&d, "ann", "edit-sdk", "k", "ann-edit").unwrap();
        set(&d, "bob", "edit-sdk", "k", "bob-edit").unwrap();
        set(&d, "ann", "cli", "k", "ann-cli").unwrap();
        assert_eq!(g(&d, "ann", "edit-sdk", "k"), json!("ann-edit"));
        assert_eq!(g(&d, "bob", "edit-sdk", "k"), json!("bob-edit"));
        assert_eq!(g(&d, "ann", "cli", "k"), json!("ann-cli"));
        // a name that tries to climb out of the folder is only a hex name and a tile id: it makes no other file
        assert_eq!(g(&d, "../ann", "edit-sdk", "k"), Value::Null);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keys_values_and_the_whole_store_have_limits_and_a_refused_set_changes_nothing() {
        let d = dir("caps");
        for bad in ["", &"k".repeat(MAX_KEY + 1), "a\nb"] {
            assert!(set(&d, "ann", "t", bad, "v").is_err(), "{bad:?}");
        }
        assert!(set(&d, "ann", "t", "big", &"x".repeat(MAX_VALUE + 1)).unwrap_err().contains("too big"));
        for i in 0..MAX_KEYS {
            set(&d, "ann", "t", &format!("k{i}"), "v").unwrap();
        }
        assert!(set(&d, "ann", "t", "one-more", "v").unwrap_err().contains("too big"));
        assert_eq!(g(&d, "ann", "t", "one-more"), Value::Null);
        set(&d, "ann", "t", "k0", "again").unwrap(); // a key that exists may change
        // the whole store is capped by size too
        let d2 = dir("caps2");
        for i in 0..3 {
            set(&d2, "ann", "t", &format!("k{i}"), &"x".repeat(MAX_VALUE)).unwrap();
        }
        assert!(set(&d2, "ann", "t", "k3", &"x".repeat(MAX_VALUE)).unwrap_err().contains("too big"));
        assert!(call(&d, "ann", "t", "nope", "{}").is_err() && call(&d, "ann", "t", "get", "{}").is_err() && call(&d, "ann", "t", "get", "not json").is_err());
        let _ = (std::fs::remove_dir_all(&d), std::fs::remove_dir_all(&d2));
    }

    #[test]
    fn removing_a_tile_clears_its_stores_for_everyone_and_only_that_tile() {
        let d = dir("clear");
        for (u, t) in [("ann", "x"), ("bob", "x"), ("ann", "a__x"), ("ann", "y")] {
            set(&d, u, t, "k", "v").unwrap();
        }
        clear_tile(&d, "x");
        assert_eq!((g(&d, "ann", "x", "k"), g(&d, "bob", "x", "k")), (Value::Null, Value::Null));
        assert_eq!((g(&d, "ann", "a__x", "k"), g(&d, "ann", "y", "k")), (json!("v"), json!("v")));
        let _ = std::fs::remove_dir_all(&d);
    }
}

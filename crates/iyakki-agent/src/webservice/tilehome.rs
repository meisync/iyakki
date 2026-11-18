//! The `home` domain of a browser tile (F-SDK-64): what the UI SDK's files port asks of a tile host (`home.list {dir}`, `home.read {path}`),
//! answered by the core's `Files` facet for the signed-in person. Read only: a change needs the person's answer from the page, which a tile cannot
//! give, so `change` and `decide` are not calls a tile may make.

use iyakki_core_api::files::{check_rel, Files, MAX_READ};
use serde_json::{json, Value};

/// The most text one `home.read` returns to a tile.
pub const TILE_READ_MAX: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Req {
    List(String),
    Read(String),
}

pub fn plan(method: &str, args_json: &str) -> Result<Req, String> {
    let args: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !args.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    match method {
        "list" => {
            let dir = args["dir"].as_str().unwrap_or("");
            check_rel(dir)?;
            Ok(Req::List(dir.to_string()))
        }
        "read" => {
            let path = args["path"].as_str().unwrap_or("");
            if check_rel(path)?.is_empty() {
                return Err("name a file".into());
            }
            Ok(Req::Read(path.to_string()))
        }
        _ => Err("that is not a call this tile may make".into()),
    }
}

/// Run a planned request; the answer is the JSON the tile's port reads.
pub fn run(files: &dyn Files, user: &str, req: &Req) -> Result<String, String> {
    match req {
        Req::List(dir) => {
            let l = files.list(user, dir)?;
            Ok(json!({"dir": l.dir, "truncated": l.truncated, "entries": l.entries.iter().map(|e| json!({"name": e.name, "is_dir": e.is_dir, "size": e.size})).collect::<Vec<_>>()}).to_string())
        }
        Req::Read(path) => {
            let t = files.read_text(user, path, TILE_READ_MAX.min(MAX_READ))?;
            Ok(json!({"text": t}).to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::{FileChange, NoFiles};

    #[test]
    fn only_list_and_read_are_calls_and_the_arguments_are_checked() {
        assert_eq!(plan("list", "").unwrap(), Req::List(String::new()));
        assert_eq!(plan("list", r#"{"dir":"Docs/x"}"#).unwrap(), Req::List("Docs/x".into()));
        assert_eq!(plan("read", r#"{"path":"a/b.txt"}"#).unwrap(), Req::Read("a/b.txt".into()));
        for bad in [("list", r#"{"dir":".."}"#), ("list", r#"{"dir":"/etc"}"#), ("read", "{}"), ("read", r#"{"path":"../x"}"#), ("read", "[1]"), ("read", "nope"), ("change", "{}"), ("decide", "{}"), ("write", "{}"), ("", "{}")] {
            assert!(plan(bad.0, bad.1).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_core_with_no_files_answers_with_its_sentence_and_the_facet_is_read_only_here() {
        let e = run(&NoFiles, "u", &Req::List(String::new())).unwrap_err();
        assert!(e.contains("no file browser"), "{e}");
        let _ = FileChange::Trash { path: "x".into() };
    }
}

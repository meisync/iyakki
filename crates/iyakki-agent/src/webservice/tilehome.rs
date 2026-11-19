//! The `home` domain of a browser tile (F-SDK-64, F-SDK-107): what the UI SDK's files port asks of a tile host (`home.list {dir}`, `home.read {path}`,
//! `home.change {kind, ...}`), answered by the core's `Files` facet for the signed-in person. A change needs the person's answer, and the tile
//! (untrusted code) cannot give it: the host asks in the person's page (`TileAsks`, a card naming the change in words), and only when the person
//! said yes there does it make the core's own question and answer it in one step (`change` then `decide`); `decide` is not a call a tile may make.

use iyakki_core_api::files::{check_change, check_rel, FileChange, Files, MAX_READ};
use serde_json::{json, Value};

/// The most text one `home.read` returns to a tile.
pub const TILE_READ_MAX: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Req {
    List(String),
    Read(String),
    /// A change the person has to say yes to, in the page.
    Change(FileChange),
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
        "change" => {
            let text = |k: &str| args[k].as_str().unwrap_or("").to_string();
            let c = match args["kind"].as_str().unwrap_or("") {
                "make_dir" => FileChange::MakeDir { dir: text("dir"), name: text("name") },
                "rename" => FileChange::Rename { path: text("path"), new_name: text("new_name") },
                "trash" => FileChange::Trash { path: text("path") },
                "save" => FileChange::Save { path: text("path"), text: text("text") },
                _ => return Err("a change is make_dir, rename, trash or save".into()),
            };
            check_change(&c)?;
            Ok(Req::Change(c))
        }
        _ => Err("that is not a call this tile may make".into()),
    }
}

/// The words of the question a tile's change makes in the person's page (the core's own description: names and sizes, never the text saved).
pub fn change_words(c: &FileChange) -> String {
    format!("A tile asks to change your files: {}", iyakki_core_api::files::describe(c))
}

/// What is told to a tile whose change waits for the person's answer in the page; the tile asks the same again after they answer.
pub const ASKED_IN_PAGE: &str = "the person was asked in their page; try the same call again after they answer";

/// After the person's yes in the page: the core's own question for the change, answered yes in the same step. The answer is what happened.
pub fn run_change(files: &dyn Files, user: &str, c: &FileChange) -> Result<String, String> {
    let ask = files.change(user, c)?;
    let done = files.decide(user, &ask.call, true)?;
    Ok(json!({"done": done}).to_string())
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
        // never run without the person's yes: the host asks first (`run_change` after the answer)
        Req::Change(_) => Err("a change waits for the person's answer in their page".into()),
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
        for bad in [("list", r#"{"dir":".."}"#), ("list", r#"{"dir":"/etc"}"#), ("read", "{}"), ("read", r#"{"path":"../x"}"#), ("read", "[1]"), ("read", "nope"), ("change", "{}"), ("change", r#"{"kind":"rm"}"#), ("change", r#"{"kind":"trash","path":"../x"}"#), ("change", r#"{"kind":"make_dir","dir":"","name":".hidden"}"#), ("decide", "{}"), ("write", "{}"), ("", "{}")] {
            assert!(plan(bad.0, bad.1).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_change_is_planned_asked_in_words_without_the_text_and_never_run_by_run() {
        let c = plan("change", r#"{"kind":"save","path":"notes/a.txt","text":"secret words"}"#).unwrap();
        let Req::Change(fc) = &c else { panic!("a change") };
        assert_eq!(change_words(fc), "A tile asks to change your files: Save `notes/a.txt` (12 bytes), replacing it if it is there");
        assert!(!change_words(fc).contains("secret"));
        assert_eq!(plan("change", r#"{"kind":"make_dir","dir":"Docs","name":"new"}"#).unwrap(), Req::Change(FileChange::MakeDir { dir: "Docs".into(), name: "new".into() }));
        assert_eq!(plan("change", r#"{"kind":"rename","path":"a.txt","new_name":"b.txt"}"#).unwrap(), Req::Change(FileChange::Rename { path: "a.txt".into(), new_name: "b.txt".into() }));
        assert_eq!(plan("change", r#"{"kind":"trash","path":"a.txt"}"#).unwrap(), Req::Change(FileChange::Trash { path: "a.txt".into() }));
        assert!(run(&NoFiles, "u", &c).unwrap_err().contains("waits for the person"), "run never makes a change");
    }

    #[test]
    fn a_core_with_no_files_answers_with_its_sentence_and_the_facet_is_read_only_here() {
        let e = run(&NoFiles, "u", &Req::List(String::new())).unwrap_err();
        assert!(e.contains("no file browser"), "{e}");
        let _ = FileChange::Trash { path: "x".into() };
    }
}

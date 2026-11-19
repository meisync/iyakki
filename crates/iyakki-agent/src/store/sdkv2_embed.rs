//! The embed backend of `iyakki.embed` (SDK 1.2, F-SDK-14): what the shell paints inside a rectangle of a tile page. The bridge has already
//! checked the app's permission, the kinds, the sizes and (for a browser) the first-use card; this backend keeps what is mounted on an
//! [`EmbedBoard`] that the tile screen reads to paint and to route input, and it adds the rules only the host knows:
//!
//! * a browser embed may visit only the hosts the app declared **that the person's own host policy also allows** (the intersection); an empty
//!   intersection refuses the mount in words, and the page has its own cookie profile named after the app (never the person's browser's);
//! * a markdown embed shows text the app sent or a `.md` file inside the app (at most 56 KB), never a path of the person's computer;
//! * `embed.call` for the viewer is kept as the viewer's next action (scroll position, find text, play state) for the painter to take.
//!
//! The painter does not run here: [`EmbedBoard::snapshot`] hands it the plain facts.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::sdkv2_backends::{Backend, Who};
use super::sdkv2_playback::AppFile;
use super::tilesdk::{code, SdkError};

/// Most bytes of a markdown embed.
pub const MAX_MARKDOWN: usize = 56 * 1024;

/// One mounted viewer, as the painter needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbedView {
    pub id: u32,
    pub kind: String,
    /// An element id (`"chart"`) or a box `{x, y, w, h}` in page pixels.
    pub target: Value,
    pub options: Value,
    /// Markdown: the text to show. Empty for the others.
    pub text: String,
    /// Browser: the hosts it may visit (declared and allowed by the person's policy); everything else is refused by name.
    pub hosts: Vec<String>,
    /// Browser: the cookie profile of this app's embedded page: not shared with any other app or with the person's browser.
    pub profile: String,
    /// Calls the app made since the painter last looked: `(method, args)`, oldest first, at most 20.
    pub calls: Vec<(String, Value)>,
}

#[derive(Clone, Default)]
pub struct EmbedBoard(Arc<Mutex<BTreeMap<u32, EmbedView>>>);

impl EmbedBoard {
    fn st(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, EmbedView>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Everything mounted, oldest first.
    pub fn snapshot(&self) -> Vec<EmbedView> {
        self.st().values().cloned().collect()
    }

    /// The painter took the calls of one viewer.
    pub fn take_calls(&self, id: u32) -> Vec<(String, Value)> {
        self.st().get_mut(&id).map(|v| std::mem::take(&mut v.calls)).unwrap_or_default()
    }
}

pub struct EmbedEngine {
    board: EmbedBoard,
    files: AppFile,
    /// Whether the person's host policy lets this host be visited.
    host_ok: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl EmbedEngine {
    pub fn new(board: EmbedBoard, files: AppFile, host_ok: Arc<dyn Fn(&str) -> bool + Send + Sync>) -> EmbedEngine {
        EmbedEngine { board, files, host_ok }
    }

    fn markdown_text(&self, who: &Who, o: &Value) -> Result<String, SdkError> {
        let text = if let Some(t) = o.get("text").and_then(Value::as_str) {
            t.to_string()
        } else if let Some(f) = o.get("file").and_then(Value::as_str) {
            let bytes = (self.files)(&who.app, f).ok_or_else(|| SdkError::bad("options.file", "a file inside the app"))?;
            String::from_utf8(bytes).map_err(|_| SdkError::bad("options.file", "a text file"))?
        } else {
            return Err(SdkError::bad("options", "{text} or {file}"));
        };
        if text.len() > MAX_MARKDOWN {
            return Err(SdkError::bad("options", "the document is too long (about 56 KB)"));
        }
        Ok(text)
    }
}

/// The profile name of an app's embedded page cookies.
pub fn cookie_profile(user: &str, app: &str) -> String {
    format!("embed:{user}:{app}")
}

/// The hosts an app declared and the person's policy allows, in the declared order.
pub fn allowed_hosts(declared: &[String], host_ok: &dyn Fn(&str) -> bool) -> Vec<String> {
    declared.iter().filter(|h| host_ok(h)).cloned().collect()
}

impl Backend for EmbedEngine {
    fn call(&self, who: &Who, op: &str, a: &Value) -> Result<Value, SdkError> {
        let id = a.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
        match op {
            "embed.mount" => {
                let kind = a.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
                let options = a.get("options").cloned().unwrap_or_else(|| json!({}));
                let declared: Vec<String> = a.get("hosts").and_then(Value::as_array).map(|h| h.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let (mut text, mut hosts, mut profile) = (String::new(), Vec::new(), String::new());
                match kind.as_str() {
                    "markdown" => text = self.markdown_text(who, &options)?,
                    "browser" => {
                        hosts = allowed_hosts(&declared, &*self.host_ok);
                        if hosts.is_empty() {
                            return Err(SdkError::new(code::BLOCKED, "your web access settings do not allow any of the sites this app may show").with("fix", "Allow the site in Settings -> Web access, or choose another."));
                        }
                        let url_host = options.get("url").and_then(Value::as_str).and_then(super::sdkv2::url_host);
                        if let Some(h) = url_host {
                            if !hosts.contains(&h) {
                                return Err(SdkError::new(code::BLOCKED, format!("your web access settings do not allow {h}")).with("host", h));
                            }
                        }
                        profile = cookie_profile(&who.user, &who.app);
                    }
                    "media" => {
                        let f = options.get("file").and_then(Value::as_str).unwrap_or("");
                        if (self.files)(&who.app, f).is_none() {
                            return Err(SdkError::bad("options.file", "a sound file inside the app"));
                        }
                    }
                    _ => {}
                }
                self.board.st().insert(id, EmbedView { id, kind, target: a.get("target").cloned().unwrap_or(Value::Null), options, text, hosts, profile, calls: Vec::new() });
                Ok(json!({}))
            }
            "embed.update" => {
                let o = a.get("options").cloned().unwrap_or_else(|| json!({}));
                let kind = self.board.st().get(&id).map(|v| v.kind.clone()).ok_or_else(|| SdkError::bad("id", "not mounted"))?;
                let text = if kind == "markdown" { Some(self.markdown_text(who, &o)?) } else { None };
                let mut st = self.board.st();
                let v = st.get_mut(&id).ok_or_else(|| SdkError::bad("id", "not mounted"))?;
                if let (Some(cur), Some(new)) = (v.options.as_object_mut(), o.as_object()) {
                    for (k, x) in new {
                        cur.insert(k.clone(), x.clone());
                    }
                }
                if let Some(t) = text {
                    v.text = t;
                }
                Ok(json!({}))
            }
            "embed.call" => {
                let mut st = self.board.st();
                let v = st.get_mut(&id).ok_or_else(|| SdkError::bad("id", "not mounted"))?;
                if v.calls.len() >= 20 {
                    v.calls.remove(0);
                }
                v.calls.push((a.get("method").and_then(Value::as_str).unwrap_or("").to_string(), a.get("args").cloned().unwrap_or(Value::Null)));
                Ok(json!({"ok": true}))
            }
            "embed.unmount" => {
                self.board.st().remove(&id);
                Ok(json!({}))
            }
            other => Err(SdkError::new(code::UNSUPPORTED, format!("{other} is not an embed call"))),
        }
    }
}

#[cfg(test)]
#[path = "sdkv2_embed_tests.rs"]
mod tests;

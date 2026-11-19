//! The address bar's own short words (Settings -> Browser -> Short words, and `short add NAME https://site/search?q=%s` in the address
//! bar), and the chat tools for them: `browser_short_words_list` (free), `browser_short_words_add` and `browser_short_words_remove`
//! (each asks the person on a card that names the word and the address). A short word sends what follows it to a site: `w cats` opens
//! Wikipedia's search for cats. The words live in the person's browser file (`browser-<name>.json`, key `short_words`), which the
//! browser tile reads before every page; this file edits only that key and leaves every other setting as it was.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
pub use iyakki_shared::browsershort::{MAX_WORDS, PROMPT_RULE};

/// The words that are built in (the person's own words beat them).
pub const BUILT_IN: [&str; 7] = ["w", "ddg", "b", "yt", "gh", "mdn", "rs"];

pub const LIST_TOOL: &str = "browser_short_words_list";
pub const ADD_TOOL: &str = "browser_short_words_add";
pub const REMOVE_TOOL: &str = "browser_short_words_remove";
pub const WRITE_TOOLS: [&str; 2] = [ADD_TOOL, REMOVE_TOOL];


pub fn is_short_tool(n: &str) -> bool {
    n == LIST_TOOL || WRITE_TOOLS.contains(&n)
}

/// A short word is one to twelve lowercase letters or digits; the address must be https (or http) and hold `%s` once. Anything else is refused.
pub fn valid(word: &str, template: &str) -> Result<(), String> {
    if word.is_empty() || word.len() > 12 || !word.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return Err("a short word is 1 to 12 lowercase letters or digits".into());
    }
    if !(template.starts_with("https://") || template.starts_with("http://")) || template.len() > 300 || template.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the address must start with https:// and hold no spaces".into());
    }
    if template.matches("%s").count() != 1 {
        return Err("the address must hold %s once, where the search words go".into());
    }
    Ok(())
}

/// Where the words are kept (a fake in tests).
pub trait Backend: Send + Sync {
    fn list(&self) -> Vec<(String, String)>;
    fn add(&self, word: &str, template: &str) -> Result<(), String>;
    fn remove(&self, word: &str) -> Result<(), String>;
}

/// The person's browser file in the data folder. Only the `short_words` key is read and written; a file that is not valid JSON is
/// never overwritten (the browser moves it aside the next time it reads it).
pub struct FileBackend {
    pub data: PathBuf,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl FileBackend {
    fn path(&self) -> PathBuf {
        let who: String = self.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        self.data.join(format!("browser-{who}.json"))
    }

    fn read(&self) -> Result<serde_json::Map<String, Value>, String> {
        match std::fs::read_to_string(self.path()) {
            Ok(t) => match serde_json::from_str::<Value>(&t) {
                Ok(Value::Object(m)) => Ok(m),
                _ => Err("the browser settings file is damaged; open Settings -> Browser first so it is repaired".into()),
            },
            Err(_) => Ok(serde_json::Map::new()),
        }
    }

    fn write(&self, m: serde_json::Map<String, Value>) -> Result<(), String> {
        let p = self.path();
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, Value::Object(m).to_string()).map_err(|e| format!("could not save: {e}"))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("could not save: {e}"))
    }
}

impl Backend for FileBackend {
    fn list(&self) -> Vec<(String, String)> {
        let m = self.read().unwrap_or_default();
        m.get("short_words").and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, v)| v.as_str().filter(|t| valid(k, t).is_ok()).map(|t| (k.clone(), t.to_string()))).take(MAX_WORDS).collect()).unwrap_or_default()
    }

    fn add(&self, word: &str, template: &str) -> Result<(), String> {
        valid(word, template)?;
        let mut m = self.read()?;
        let mut words = m.get("short_words").and_then(Value::as_object).cloned().unwrap_or_default();
        if words.len() >= MAX_WORDS && !words.contains_key(word) {
            return Err(format!("at most {MAX_WORDS} short words"));
        }
        words.insert(word.to_string(), Value::String(template.to_string()));
        m.insert("short_words".into(), Value::Object(words));
        self.write(m)?;
        (self.audit)("settings.browser", &format!("Short word {word} saved (chosen in chat)"));
        Ok(())
    }

    fn remove(&self, word: &str) -> Result<(), String> {
        let mut m = self.read()?;
        let mut words = m.get("short_words").and_then(Value::as_object).cloned().unwrap_or_default();
        if words.remove(word).is_none() {
            return Err(format!("no short word {word}"));
        }
        m.insert("short_words".into(), Value::Object(words));
        self.write(m)?;
        (self.audit)("settings.browser", &format!("Short word {word} removed (chosen in chat)"));
        Ok(())
    }
}

pub struct ShortTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn arg<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

impl ShortTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ShortTools {
        ShortTools { inner, backend }
    }

    /// The card for a change, or why it cannot be made.
    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        let word = arg(args, "word").to_ascii_lowercase();
        let list = self.backend.list();
        match name {
            ADD_TOOL => {
                let template = arg(args, "address");
                valid(&word, template)?;
                if list.len() >= MAX_WORDS && !list.iter().any(|(k, _)| *k == word) {
                    return Err(format!("at most {MAX_WORDS} short words"));
                }
                let beats = if BUILT_IN.contains(&word.as_str()) { " It replaces the built-in word of the same name." } else { "" };
                match list.iter().find(|(k, _)| *k == word) {
                    Some((_, old)) => Ok(format!("Change the short word {word} so it searches {template} (now {old}).{beats}")),
                    None => Ok(format!("Add the short word {word}: typing {word} and some words in the address bar searches {template}.{beats}")),
                }
            }
            REMOVE_TOOL => match list.iter().find(|(k, _)| *k == word) {
                Some((_, t)) => Ok(format!("Remove the short word {word} (it searched {t}).")),
                None => Err(format!("no short word {word} of your own{}", if BUILT_IN.contains(&word.as_str()) { " (it is built in and cannot be removed)" } else { "" })),
            },
            _ => Err("unknown tool".into()),
        }
    }

    fn list_text(&self) -> String {
        let list = self.backend.list();
        let mut t = if list.is_empty() { "No short words of your own yet.".to_string() } else { format!("Your short words ({}):\n{}", list.len(), list.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n")) };
        t.push_str(&format!("\nBuilt in: {}.", BUILT_IN.join(", ")));
        t
    }
}

impl Tools for ShortTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_short_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: LIST_TOOL.into(), description: "List the person's own address-bar short words and the built-in ones.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: ADD_TOOL.into(),
                description: "Add or change an address-bar short word. The person approves a card naming the word and its address. word: 1 to 12 lowercase letters or digits. address: https (or http) address with %s once where the search words go.".into(),
                schema: json!({"type": "object", "properties": {"word": {"type": "string"}, "address": {"type": "string", "description": "for example https://www.youtube.com/results?search_query=%s"}}, "required": ["word", "address"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v.push(ToolDef {
            spec: ToolSpec { name: REMOVE_TOOL.into(), description: "Remove one of the person's own short words (asks).".into(), schema: json!({"type": "object", "properties": {"word": {"type": "string"}}, "required": ["word"]}) },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LIST_TOOL => Ok(self.list_text()),
            ADD_TOOL | REMOVE_TOOL => {
                let card = self.card(name, args)?;
                let word = arg(args, "word").to_ascii_lowercase();
                if name == ADD_TOOL {
                    self.backend.add(&word, arg(args, "address"))?;
                } else {
                    self.backend.remove(&word)?;
                }
                Ok(format!("Done. {card}\n{}", self.list_text()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.split(". ").next().unwrap_or("").trim_end_matches('.').to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == LIST_TOOL { "List the browser short words".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == LIST_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == LIST_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_short_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}

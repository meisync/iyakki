//! The vendor session id kept per chat and agent, so the next message resumes the same conversation (moved out of `iyakki_agent::extagent::run`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Keep a vendor session id per chat and agent, so the next message resumes the same conversation.
pub struct SessionMap {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SessionMap {
    pub fn open(path: &Path) -> SessionMap {
        SessionMap { path: path.into(), lock: Mutex::new(()) }
    }
    fn load(&self) -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn get(&self, chat: &str, agent: &str) -> Option<String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().get(&format!("{chat}|{agent}")).and_then(|v| v.as_str()).map(str::to_string)
    }
    pub fn set(&self, chat: &str, agent: &str, id: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.load();
        m.insert(format!("{chat}|{agent}"), serde_json::Value::String(id.into()));
        if let Some(p) = self.path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&self.path, serde_json::Value::Object(m).to_string());
    }
    pub fn forget_chat(&self, chat: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = self.load();
        m.retain(|k, _| !k.starts_with(&format!("{chat}|")));
        let _ = std::fs::write(&self.path, serde_json::Value::Object(m).to_string());
    }
}

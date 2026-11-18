use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Lexical,
    /// A reranking model of the local Lemonade server, by its name.
    Model { model: String },
}

impl Mode {
    pub fn text(&self) -> String {
        match self {
            Mode::Off => "off".into(),
            Mode::Lexical => "lexical".into(),
            Mode::Model { model } => format!("model {model}"),
        }
    }

    pub fn path(data: &Path) -> PathBuf {
        data.join("rag-rerank.json")
    }

    pub fn load(data: &Path) -> Mode {
        std::fs::read_to_string(Mode::path(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, data: &Path) -> Result<(), String> {
        if let Mode::Model { model } = self {
            if !model_ok(model) {
                return Err("a model name has letters, digits and . - _ : / only (80 at most)".into());
            }
        }
        let t = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&Mode::path(data), t.as_bytes(), 0o644)
    }

    /// `off`, `lexical` or a model name from the arguments of the chat tool.
    pub fn parse(mode: &str, model: &str) -> Result<Mode, String> {
        match mode.trim() {
            "off" => Ok(Mode::Off),
            "lexical" => Ok(Mode::Lexical),
            "model" => {
                let m = model.trim();
                if model_ok(m) { Ok(Mode::Model { model: m.to_string() }) } else { Err("mode model needs the name of a reranking model of the local Lemonade server (Settings -> Models lists them)".into()) }
            }
            _ => Err("mode is off, lexical or model".into()),
        }
    }
}

pub fn model_ok(m: &str) -> bool {
    !m.is_empty() && m.len() <= 80 && m.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'/'))
}

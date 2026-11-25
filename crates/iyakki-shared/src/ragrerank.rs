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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_names_off_lexical_and_the_model_by_name() {
        assert_eq!(Mode::Off.text(), "off");
        assert_eq!(Mode::Lexical.text(), "lexical");
        assert_eq!(Mode::Model { model: "bge-reranker-v2".into() }.text(), "model bge-reranker-v2");
    }

    #[test]
    fn the_default_mode_is_off() {
        assert_eq!(Mode::default(), Mode::Off);
    }

    #[test]
    fn parse_accepts_off_and_lexical_ignoring_surrounding_spaces() {
        assert_eq!(Mode::parse("  lexical ", ""), Ok(Mode::Lexical));
        assert_eq!(Mode::parse("off", "ignored"), Ok(Mode::Off));
    }

    #[test]
    fn parse_takes_a_trimmed_model_name_for_model_mode() {
        assert_eq!(Mode::parse("model", "  bge-reranker-v2.m3 "), Ok(Mode::Model { model: "bge-reranker-v2.m3".into() }));
    }

    #[test]
    fn parse_refuses_model_mode_without_a_valid_model_name() {
        assert!(Mode::parse("model", "").unwrap_err().contains("reranking model"));
        assert!(Mode::parse("model", "my model").is_err());
    }

    #[test]
    fn parse_refuses_an_unknown_mode_word_and_is_case_sensitive() {
        assert_eq!(Mode::parse("fancy", ""), Err("mode is off, lexical or model".to_string()));
        assert!(Mode::parse("OFF", "").is_err());
    }

    #[test]
    fn model_names_accept_letters_digits_and_the_separators_up_to_80_bytes() {
        assert!(model_ok("a"));
        assert!(model_ok("org/model:tag.v1-2_x"));
        assert!(model_ok(&"a".repeat(80)));
    }

    #[test]
    fn model_names_refuse_empty_long_spaced_and_non_ascii_names() {
        assert!(!model_ok(""));
        assert!(!model_ok(&"a".repeat(81)));
        assert!(!model_ok("a b"));
        assert!(!model_ok("modèle"));
    }

    #[test]
    fn the_mode_file_is_rag_rerank_json_in_the_data_folder() {
        assert_eq!(Mode::path(Path::new("/data")), PathBuf::from("/data/rag-rerank.json"));
    }

    #[test]
    fn mode_serialises_with_a_mode_tag_and_the_model_as_a_field() {
        assert_eq!(serde_json::to_string(&Mode::Lexical).unwrap(), r#"{"mode":"lexical"}"#);
        assert_eq!(serde_json::to_string(&Mode::Model { model: "m1".into() }).unwrap(), r#"{"mode":"model","model":"m1"}"#);
    }
}

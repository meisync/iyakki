//! `/etc/iyakki/ai.conf`: where the chat model lives and where RAG documents are.
//!
//! ```text
//! chat.url      = https://api.openai.com/v1/chat/completions   # or a LAN Ollama/vLLM URL
//! chat.model    = gpt-4o-mini
//! chat.key_file = /etc/iyakki/chat.key       # file holding the API key (preferred on bare metal)
//! chat.key_env  = OPENAI_API_KEY           # or an environment variable
//! rag.docs      = /var/lib/iyakki/docs
//! mcp.config    = /etc/iyakki/mcp.json
//! ```

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiConfig {
    pub chat_url: Option<String>,
    pub chat_model: String,
    pub chat_key_file: Option<String>,
    pub chat_key_env: Option<String>,
    pub rag_docs: String,
    pub mcp_config: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("line {0}: expected `key = value`")]
    Syntax(usize),
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("chat.url must be an http(s) URL with a host")]
    BadUrl,
}

impl AiConfig {
    pub fn parse(text: &str) -> Result<AiConfig, ConfigError> {
        let mut c = AiConfig { chat_model: "default".into(), rag_docs: "/var/lib/iyakki/docs".into(), mcp_config: "/etc/iyakki/mcp.json".into(), ..Default::default() };
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or(ConfigError::Syntax(i + 1))?;
            let v = v.trim().to_string();
            match k.trim() {
                "chat.url" => {
                    iyakki_sec::url_host(&v).ok_or(ConfigError::BadUrl)?;
                    c.chat_url = Some(v);
                }
                "chat.model" => c.chat_model = v,
                "chat.key_file" => c.chat_key_file = Some(v),
                "chat.key_env" => c.chat_key_env = Some(v),
                "rag.docs" => c.rag_docs = v,
                "mcp.config" => c.mcp_config = v,
                other => return Err(ConfigError::UnknownKey(other.into())),
            }
        }
        Ok(c)
    }

    pub fn load(path: &str) -> AiConfig {
        std::fs::read_to_string(path).ok().and_then(|t| AiConfig::parse(&t).ok()).unwrap_or_else(|| AiConfig::parse("").unwrap())
    }

    pub fn api_key(&self) -> Option<String> {
        if let Some(f) = &self.chat_key_file {
            if let Ok(k) = std::fs::read_to_string(f) {
                let k = k.trim().to_string();
                if !k.is_empty() {
                    return Some(k);
                }
            }
        }
        self.chat_key_env.as_ref().and_then(|e| std::env::var(e).ok())
    }
}

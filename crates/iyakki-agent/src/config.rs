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

use std::sync::Arc;

use iyakki_proxy::{https::UreqUpstream, EgressProxy, HostPolicy, Upstream};

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

    fn api_key(&self) -> Option<String> {
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

    /// Egress proxy that allows exactly the configured chat host, injecting the key as a bearer
    /// token. The key lives only in host memory; sandboxed guests never see it.
    pub fn build_proxy(&self, upstream: Arc<dyn Upstream>) -> EgressProxy {
        let mut p = EgressProxy::new(upstream);
        if let Some(host) = self.chat_url.as_deref().and_then(iyakki_sec::url_host) {
            let mut pol = HostPolicy { raw_response: true, ..Default::default() };
            if let Some(k) = self.api_key() {
                pol.inject_headers.push(("Authorization".into(), format!("Bearer {k}")));
            }
            p = p.allow_host(&host, pol);
        }
        p
    }

    pub fn build_proxy_real(&self) -> EgressProxy {
        self.build_proxy(Arc::new(UreqUpstream::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates() {
        let c = AiConfig::parse("# c\nchat.url = http://10.0.0.5:11434/v1/chat/completions\nchat.model=llama3 # m\nrag.docs=/d").unwrap();
        assert_eq!(c.chat_url.as_deref(), Some("http://10.0.0.5:11434/v1/chat/completions"));
        assert_eq!((c.chat_model.as_str(), c.rag_docs.as_str()), ("llama3", "/d"));
        assert_eq!(AiConfig::parse("chat.url = ftp://x"), Err(ConfigError::BadUrl));
        assert_eq!(AiConfig::parse("nonsense"), Err(ConfigError::Syntax(1)));
        assert!(matches!(AiConfig::parse("a = b"), Err(ConfigError::UnknownKey(_))));
        assert_eq!(AiConfig::parse("").unwrap().chat_url, None);
    }

    #[test]
    fn proxy_allows_only_chat_host_and_hides_key() {
        let dir = std::env::temp_dir().join(format!("iyakki-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kf = dir.join("key");
        std::fs::write(&kf, "sk-secret\n").unwrap();
        let c = AiConfig::parse(&format!("chat.url = https://api.example.com/v1/chat/completions\nchat.key_file = {}", kf.display())).unwrap();
        struct Spy(std::sync::Mutex<Vec<(String, String)>>);
        impl Upstream for Spy {
            fn request(&self, _m: &str, _u: &str, h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                self.0.lock().unwrap().extend(h.iter().cloned());
                Ok(iyakki_proxy::UpstreamResponse { status: 200, body: b"{}".to_vec() })
            }
        }
        let spy = Arc::new(Spy(Default::default()));
        let p = c.build_proxy(spy.clone());
        assert_eq!(p.hosts(), vec!["api.example.com"]);
        use iyakki_wasm::NetBackend;
        assert!(p.fetch("POST", "https://api.example.com/v1/chat/completions", b"{}").is_ok());
        assert!(p.fetch("GET", "https://other.example.com/", b"").is_err());
        let seen = spy.0.lock().unwrap().clone();
        assert!(seen.contains(&("Authorization".into(), "Bearer sk-secret".into())));
        assert!(seen.contains(&("Content-Type".into(), "application/json".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

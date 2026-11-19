//! Moved out of `iyakki_agent::ocipush` (F-SPLIT-03, bucket b0); the agent re-exports it.


/// One reply of a registry.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).chars().take(300).collect()
    }
}

/// The one way this module speaks HTTP. `host` is `name:port`; `path` starts with `/` and may carry a query.
pub trait RegistryHttp: Send + Sync {
    fn request(&self, method: &str, host: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Reply, String>;
}

pub const MAX_REPLY: usize = 8 * 1024 * 1024;

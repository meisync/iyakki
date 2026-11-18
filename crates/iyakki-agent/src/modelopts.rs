//! The agent's part of `iyakki_shared::modelopts`: asking the provider for its model list (the HTTPS client lives here).

pub use iyakki_shared::modelopts::*;

/// Ask the provider for its own list (`GET {base}/models` with the key from the vault) and keep it for the session. The request goes
/// through the provider client (the HTTPS client with its timeout and body cap); the key is never shown. Failures are in plain words.
pub fn fetch_models(client: &crate::providers::ProviderClient) -> Result<Vec<String>, String> {
    let id = client.cfg.id.clone();
    match client.list_models() {
        Ok(mut v) => {
            v.truncate(LIST_CAP);
            remember_models(&id, v.clone());
            Ok(v)
        }
        Err(e) => Err(crate::providererr::plain_error(&id, &e)),
    }
}

impl crate::services::Services {
    /// Ask a provider for its own model list (a network call; admin only: it uses the provider's key).
    pub fn fetch_provider_models(&self, provider: &str) -> Result<Vec<String>, String> {
        self.client_for(provider, true).and_then(|c| fetch_models(&c))
    }
}

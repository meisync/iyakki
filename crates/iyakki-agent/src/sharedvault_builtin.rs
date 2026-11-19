//! The built-in shared vault as a [`Backend`] (the service client lives in `cloudvaultd`).

use iyakki_shared::cloudvaultd::Scope;
use iyakki_shared::sharedvault::{Backend, Provider};
use crate::cloudvaultd::Client;

/// The built-in service (`cloudvaultd.rs`): the member's side seals and opens.
pub struct Builtin(pub Client);

impl Backend for Builtin {
    fn provider(&self) -> Provider {
        Provider::Builtin
    }
    fn put(&self, s: &Scope, n: &str, v: &[u8]) -> Result<(), String> {
        self.0.put(s, n, v)
    }
    fn get(&self, s: &Scope, n: &str) -> Result<Vec<u8>, String> {
        self.0.get(s, n)
    }
    fn list(&self, s: &Scope) -> Result<Vec<String>, String> {
        self.0.list(s)
    }
    fn delete(&self, s: &Scope, n: &str) -> Result<(), String> {
        self.0.delete(s, n)
    }
}

//! The vault rule: the contract exposes only (1) whether it is locked, (2) unlock with the login, local seat only, (3) lock and
//! (4) "use a secret for a named action". NO method returns a secret value, and none ever will: a new method that returns text or
//! bytes fails `vault_trait_returns_no_secret_shaped_type` in `iyakki-core-fake`.

use super::consent::LocalSeat;
use super::context::CallContext;
use super::error::{CallResult, IdempotencyKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultStatus {
    pub locked: bool,
    /// How many entries it holds; names are not listed.
    pub entries: u32,
}

/// An action registered on the machine that needs a secret (for example `sign_in:git`). The caller names the action; the machine
/// picks the secret and performs the action locally.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedAction(pub String);

/// What happened, in words. Never contains a secret, a password or a token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub ok: bool,
    pub summary: String,
}

pub trait Vault: Send + Sync {
    fn status(&self, ctx: &CallContext) -> CallResult<VaultStatus>;
    /// Local seat only: the person types the machine login on the machine. A remote client cannot call this at all.
    fn unlock(&self, seat: &LocalSeat, login_password: &str) -> CallResult<VaultStatus>;
    fn lock(&self, ctx: &CallContext) -> CallResult<VaultStatus>;
    fn use_secret(&self, ctx: &CallContext, action: &NamedAction, idem: Option<&IdempotencyKey>) -> CallResult<ActionOutcome>;
}

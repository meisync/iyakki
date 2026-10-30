//! `vault.*` over the person's own vault: whether it is locked, unlock with the login (local seat only), lock, and "use a secret for a
//! named action". **No call returns a secret value.** An action is registered on the machine with the name of the secret it needs and
//! a function that performs it; the caller only names the action and gets words back (scrubbed of the secret if it ever appeared).
//!
//! Which vault: the personal vault of the account the call acts for. A local call acts for the signed-in person; a remote call acts
//! for the account its computer is paired to (the transport gives the mapping with [`CoreVault::set_account_resolver`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iyakki_core_api::contract::*;

use super::{guarded, Audit};
use crate::services::Services;

/// Performs an action with the secret it was registered for; returns what to tell the caller, in words.
pub type Perform = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

struct ActionDef {
    secret: String,
    perform: Perform,
}

type AccountOf = Arc<dyn Fn(&CallContext) -> String + Send + Sync>;

pub struct CoreVault {
    svc: Arc<Services>,
    actions: Mutex<HashMap<String, ActionDef>>,
    idem: Mutex<HashMap<IdempotencyKey, (String, ActionOutcome)>>,
    account_of: Mutex<AccountOf>,
    audit: Arc<Audit>,
}

impl CoreVault {
    pub fn new(svc: Arc<Services>) -> CoreVault {
        CoreVault { svc, actions: Mutex::new(HashMap::new()), idem: Mutex::new(HashMap::new()), account_of: Mutex::new(Arc::new(|c: &CallContext| c.identity.user.clone())), audit: Arc::new(Audit::default()) }
    }

    /// Register an action that needs the personal-vault secret `secret` (service code does this as it migrates; the caller of the
    /// contract only ever names `action`, such as `sign_in:git`).
    pub fn register_action(&self, action: &str, secret: &str, perform: Perform) {
        self.actions.lock().unwrap_or_else(|e| e.into_inner()).insert(action.to_string(), ActionDef { secret: secret.to_string(), perform });
    }

    /// Which account a call acts for (default: the caller's own name, which is right for a local call).
    pub fn set_account_resolver(&self, f: AccountOf) {
        *self.account_of.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }

    pub fn audit(&self) -> Vec<String> {
        self.audit.all()
    }
    pub(crate) fn audit_handle(&self) -> &Audit {
        &self.audit
    }

    fn owner(&self, ctx: &CallContext) -> String {
        // a person at the machine acts for themselves; only a remote caller is mapped to an account
        if ctx.is_local() {
            return ctx.identity.user.clone();
        }
        let f = self.account_of.lock().unwrap_or_else(|e| e.into_inner()).clone();
        f(ctx)
    }

    fn status_of(&self, owner: &str) -> VaultStatus {
        let unlocked = self.svc.user_vault_unlocked(owner);
        let entries = if unlocked { self.svc.user_names(owner).map(|n| n.len() as u32).unwrap_or(0) } else { 0 };
        VaultStatus { locked: !unlocked, entries }
    }
}

impl Vault for CoreVault {
    fn status(&self, ctx: &CallContext) -> CallResult<VaultStatus> {
        guarded(&self.audit, ctx, "vault.status", Scope::VAULT_STATUS, Risk::Read)?;
        Ok(self.status_of(&self.owner(ctx)))
    }

    fn unlock(&self, seat: &LocalSeat, login_password: &str) -> CallResult<VaultStatus> {
        let line = |outcome: &str| self.audit.raw(format!("{} via local: vault.unlock [local_only] {outcome}", seat.user()));
        if self.svc.user_vault_unlocked(seat.user()) {
            // already open: nothing to prove, and no password is looked at
            line("ok");
            return Ok(self.status_of(seat.user()));
        }
        match self.svc.unlock_user_vault(seat.user(), login_password) {
            Ok(()) => {
                line("ok");
                Ok(self.status_of(seat.user()))
            }
            Err(m) if m.contains("no personal vault") => {
                line("refused: not_found");
                Err(CallError::new(ErrorCode::NotFound, "vault.none", "You have no personal vault yet.").hint("It is made when you sign in with your password."))
            }
            Err(_) => {
                line("refused: denied");
                Err(CallError::new(ErrorCode::Denied, "vault.wrong_login", "That is not the login password."))
            }
        }
    }

    fn lock(&self, ctx: &CallContext) -> CallResult<VaultStatus> {
        guarded(&self.audit, ctx, "vault.lock", Scope::VAULT_LOCK, Risk::Change)?;
        let owner = self.owner(ctx);
        self.svc.lock_user_vault(&owner);
        self.audit.line(ctx, "vault.lock", Risk::Change, "ok");
        Ok(self.status_of(&owner))
    }

    fn use_secret(&self, ctx: &CallContext, action: &NamedAction, idem: Option<&IdempotencyKey>) -> CallResult<ActionOutcome> {
        guarded(&self.audit, ctx, "vault.use_secret", Scope::VAULT_USE, Risk::Change)?;
        let owner = self.owner(ctx);
        if let Some(k) = idem {
            if let Some((a, out)) = self.idem.lock().unwrap_or_else(|e| e.into_inner()).get(k) {
                if *a == action.0 {
                    return Ok(out.clone());
                }
                return Err(CallError::new(ErrorCode::Conflict, "core.idempotency_conflict", "That key was already used for a different request."));
            }
        }
        if !self.svc.user_vault_unlocked(&owner) {
            self.audit.line(ctx, "vault.use_secret", Risk::Change, "refused: locked");
            return Err(CallError::new(ErrorCode::Locked, "vault.locked", "The vault is locked.").hint("Ask the person at the machine to unlock it."));
        }
        let (secret_name, perform) = {
            let g = self.actions.lock().unwrap_or_else(|e| e.into_inner());
            match g.get(&action.0) {
                Some(d) => (d.secret.clone(), d.perform.clone()),
                None => {
                    drop(g);
                    self.audit.line(ctx, "vault.use_secret", Risk::Change, "refused: not_found");
                    return Err(CallError::new(ErrorCode::NotFound, "vault.unknown_action", "No such action is registered."));
                }
            }
        };
        let secret = self.svc.user_get(&owner, &secret_name).map_err(|_| CallError::new(ErrorCode::NotFound, "vault.no_secret", "The secret this action needs is not in the vault."))?;
        let out = match perform(secret.as_str()) {
            Ok(words) => ActionOutcome { ok: true, summary: scrub(&words, secret.as_str(), &action.0) },
            Err(words) => ActionOutcome { ok: false, summary: scrub(&words, secret.as_str(), &action.0) },
        };
        // the audit line names the action, never the secret or its name
        self.audit.line(ctx, &format!("vault.use_secret {}", action.0), Risk::Change, if out.ok { "ok" } else { "failed" });
        if let Some(k) = idem {
            self.idem.lock().unwrap_or_else(|e| e.into_inner()).insert(k.clone(), (action.0.clone(), out.clone()));
        }
        Ok(out)
    }
}

/// Words for the caller: whatever the action said, unless it contains the secret (then only that the action ran).
fn scrub(words: &str, secret: &str, action: &str) -> String {
    if !secret.is_empty() && words.contains(secret) {
        format!("{action} done")
    } else {
        words.chars().take(300).collect()
    }
}

//! Making and keeping webhooks (F-FLOW-15): what the FLOWS screen and the assistant's `webhook_*` tools call, and the machine's answers
//! for the gate ([`RealWorld`]). A webhook is a trigger of the flow (its settings are versioned with the flow, so it fires only after
//! the owner published it) plus a record in the hook store (the hash of the secret address, on or off, public until, deliveries).
//!
//! The address and the shared secret are returned once, to the screen. The assistant's tools never get them: they leave a
//! [`Reveal`](super::hookstore::Reveal) for the person's screen and tell the model only that it is waiting there.

use std::sync::Arc;

use iyakki_core_api::{AuditSink, Authenticator};
use iyakki_flow::engine::StartRequest;
use iyakki_flow::{Trigger, TriggerKind};
use serde_json::Value;

use super::hookstore::{new_id, new_token, token_hash, Delivery, HookRec, PUBLIC_MAX_S};
use super::webhook::{HookCfg, HookWorld, Hooks};
use super::{allowed, FlowEnv};
use crate::services::Services;

/// What a person (or the assistant) may choose when making a webhook.
#[derive(Clone, Debug, Default)]
pub struct HookOpts {
    pub signed: bool,
    pub methods: Vec<String>,
    pub max_body: usize,
    pub content_types: Vec<String>,
    pub rate_per_min: u32,
}

/// What making or renewing a hook gives back: shown once on the screen, never to the assistant.
pub struct Created {
    pub id: String,
    pub flow: String,
    /// `/hooks/<id>/<token>`.
    pub path: String,
    pub secret: Option<String>,
}

impl std::fmt::Debug for Created {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Created({}, hidden)", self.id)
    }
}

/// A hook as the screen and the assistant see it: nothing secret.
#[derive(Clone, Debug)]
pub struct HookView {
    pub id: String,
    pub flow: String,
    pub flow_name: String,
    pub enabled: bool,
    pub signed: bool,
    pub has_secret: bool,
    /// The published version has the trigger (so the hook can fire).
    pub published: bool,
    pub public_until: i64,
    pub created: i64,
    pub rotated: i64,
    pub deliveries: Vec<Delivery>,
}

impl HookView {
    pub fn line(&self, now: i64) -> String {
        let last = self.deliveries.last().map_or("no deliveries yet".to_string(), |d| format!("last: {} ({})", d.outcome, d.http));
        format!(
            "{} flow {} \"{}\": {}{}{}{}; {last}",
            self.id,
            self.flow,
            self.flow_name,
            if self.enabled { "ON" } else { "off" },
            if self.signed { ", signed" } else { ", not signed" },
            if self.published { "" } else { ", not published yet (publish the flow first)" },
            if self.public_until > now { format!(", open to the public for another {} min", (self.public_until - now + 59) / 60) } else { String::new() },
        )
    }
}

impl FlowEnv {
    pub fn hooks(&self) -> Arc<Hooks> {
        Hooks::shared(self.svc.dir())
    }

    fn hook_audit(&self, action: &str, detail: &str) {
        iyakki_flow::host::Host::audit(self.host.as_ref(), action, detail);
    }

    fn vault_name(id: &str) -> String {
        format!("hook.{id}")
    }

    fn own_hook(&self, id: &str) -> Result<HookRec, String> {
        if !allowed(&self.policy) {
            return Err("your account may not use FLOWS".into());
        }
        self.hooks().store.get(id).filter(|r| r.owner == self.user).ok_or_else(|| format!("there is no webhook `{id}` (webhook_list shows yours)"))
    }

    /// The person's webhooks (all, or those of one flow), newest last.
    pub fn hook_views(&self, flow: Option<&str>) -> Vec<HookView> {
        let mut v: Vec<HookView> = self
            .hooks()
            .store
            .of_owner(&self.user)
            .into_iter()
            .filter(|r| flow.is_none_or(|f| r.flow == f))
            .map(|r| {
                let live = self.store.published(&r.flow).and_then(|f| f.triggers.iter().find(|t| t.id == r.trigger).cloned());
                let draft_signed = self.store.get_draft(&r.flow).and_then(|f| f.triggers.iter().find(|t| t.id == r.trigger).cloned());
                let signed = live.as_ref().or(draft_signed.as_ref()).is_some_and(|t| matches!(t.kind, TriggerKind::Webhook { signed: true, .. }));
                HookView {
                    flow_name: self.store.get_draft(&r.flow).map_or_else(|| "(flow removed)".into(), |f| f.name),
                    published: live.as_ref().is_some_and(|t| HookCfg::from_trigger(t).is_some()),
                    id: r.id,
                    flow: r.flow,
                    enabled: r.enabled,
                    signed,
                    has_secret: r.has_secret,
                    public_until: r.public_until,
                    created: r.created,
                    rotated: r.rotated,
                    deliveries: r.deliveries,
                }
            })
            .collect();
        v.sort_by_key(|h| h.created);
        v
    }

    /// Make a webhook for a flow: a trigger in the draft, a record (off), the secret address and, when signed, a shared secret in the
    /// vault. Nothing fires until the flow is published and the hook switched on.
    pub fn hook_create(&self, flow: &str, o: &HookOpts, by: &str) -> Result<Created, String> {
        if !allowed(&self.policy) {
            return Err("your account may not use FLOWS".into());
        }
        let mut f = self.store.get_draft(flow).ok_or_else(|| format!("there is no flow `{flow}` (flow_list shows them)"))?;
        if o.signed && !self.svc.user_vault_unlocked(&self.user) {
            return Err("unlock your vault first: the shared secret of a signed webhook is kept there".into());
        }
        let (id, token) = (new_id(), new_token());
        let mut t = Trigger::new(&id, TriggerKind::Webhook { path: id.clone(), methods: o.methods.iter().map(|m| m.to_ascii_uppercase()).collect(), max_body: o.max_body, content_types: o.content_types.clone(), rate_per_min: o.rate_per_min, signed: o.signed });
        t.enabled = true;
        let secret = o.signed.then(|| new_token());
        if let Some(s) = &secret {
            self.svc.user_set(&self.user, &Self::vault_name(&id), s).map_err(|e| format!("cannot keep the shared secret in your vault: {e:?}"))?;
        }
        let undo = |me: &FlowEnv| {
            let _ = me.hooks().store.remove(&id);
            let _ = me.svc.user_delete(&me.user, &Self::vault_name(&id));
        };
        let rec = HookRec { id: id.clone(), owner: self.user.clone(), flow: flow.into(), trigger: id.clone(), token_hash: token_hash(&token), enabled: false, created: crate::clock::now_unix(), rotated: 0, public_until: 0, has_secret: secret.is_some(), deliveries: vec![] };
        if let Err(e) = self.hooks().store.insert(rec) {
            let _ = self.svc.user_delete(&self.user, &Self::vault_name(&id));
            return Err(e);
        }
        f.triggers.push(t);
        if let Err(e) = self.save_edit(&f, by, &format!("webhook {id}"), "") {
            undo(self);
            return Err(e);
        }
        self.hook_audit("hook.create", &format!("{id} for {flow} by {by} ({})", if o.signed { "signed" } else { "not signed" }));
        Ok(Created { path: format!("/hooks/{id}/{token}"), id, flow: flow.into(), secret })
    }

    /// Make the secret address (and the shared secret) again; the old ones stop working at once. The hook keeps its on or off state.
    pub fn hook_regenerate(&self, id: &str, by: &str) -> Result<Created, String> {
        let rec = self.own_hook(id)?;
        let token = new_token();
        let secret = rec.has_secret.then(new_token);
        if let Some(s) = &secret {
            if !self.svc.user_vault_unlocked(&self.user) {
                return Err("unlock your vault first: the shared secret is kept there".into());
            }
            self.svc.user_set(&self.user, &Self::vault_name(id), s).map_err(|e| format!("cannot keep the shared secret in your vault: {e:?}"))?;
        }
        self.hooks().store.update(id, |r| {
            r.token_hash = token_hash(&token);
            r.rotated = crate::clock::now_unix();
        })?;
        self.hook_audit("hook.regenerate", &format!("{id} by {by}"));
        Ok(Created { path: format!("/hooks/{id}/{token}"), id: id.into(), flow: rec.flow, secret })
    }

    /// Switch a hook on. Needs the right to run flows and a published flow that carries the trigger.
    pub fn hook_enable(&self, id: &str, by: &str) -> Result<(), String> {
        let rec = self.own_hook(id)?;
        let live = self.store.published(&rec.flow).ok_or("publish the flow first: a webhook fires the published version")?;
        let t = live.triggers.iter().find(|t| t.id == rec.trigger).ok_or("the published version has no such webhook trigger: publish the flow again")?;
        let cfg = HookCfg::from_trigger(t).ok_or("the webhook trigger is switched off in the flow")?;
        if cfg.signed && !rec.has_secret {
            return Err("the trigger is signed but has no shared secret: regenerate the webhook".into());
        }
        self.hooks().store.update(id, |r| r.enabled = true)?;
        self.hook_audit("hook.enable", &format!("{id} flow {} by {by}", rec.flow));
        Ok(())
    }

    pub fn hook_disable(&self, id: &str, by: &str) -> Result<(), String> {
        self.own_hook(id)?;
        self.hooks().store.update(id, |r| {
            r.enabled = false;
            r.public_until = 0;
        })?;
        self.hook_audit("hook.disable", &format!("{id} by {by}"));
        Ok(())
    }

    /// Take senders from outside the local network for `minutes` (0: close again). The longest is seven days. Returns the end time.
    pub fn hook_public(&self, id: &str, minutes: u64, by: &str) -> Result<i64, String> {
        self.own_hook(id)?;
        let until = if minutes == 0 { 0 } else { crate::clock::now_unix() + ((minutes * 60) as i64).min(PUBLIC_MAX_S) };
        self.hooks().store.update(id, |r| r.public_until = until)?;
        self.hook_audit("hook.public", &format!("{id} until {until} by {by}"));
        Ok(until)
    }
}

// ----------------------------------------------------------------------------------------------- the machine's answers for the gate

/// The gate's questions, answered from the machine: accounts, the owner's flows and vault, the owner's engine.
pub struct RealWorld<'a> {
    pub svc: &'a Arc<Services>,
    pub auth: &'a dyn Authenticator,
    pub audit: &'a dyn AuditSink,
    /// An environment to use for its own user instead of opening the owner's (tests keep their flows out of the real home folder).
    pub only: Option<Arc<FlowEnv>>,
}

impl RealWorld<'_> {
    fn env(&self, owner: &str) -> Option<Arc<FlowEnv>> {
        if let Some(e) = self.only.as_ref().filter(|e| e.user == owner) {
            return Some(e.clone());
        }
        let p = self.auth.lookup(owner)?;
        FlowEnv::open(self.svc, owner, &p.policy).ok()
    }
}

impl HookWorld for RealWorld<'_> {
    fn owner_allowed(&self, owner: &str) -> bool {
        self.auth.lookup(owner).is_some_and(|p| allowed(&p.policy))
    }

    fn config(&self, rec: &HookRec) -> Option<HookCfg> {
        let live = self.env(&rec.owner)?.store.published(&rec.flow)?;
        let t = live.triggers.iter().find(|t| t.id == rec.trigger)?;
        matches!(&t.kind, TriggerKind::Webhook { path, .. } if *path == rec.id).then(|| HookCfg::from_trigger(t)).flatten()
    }

    fn secret(&self, rec: &HookRec) -> Result<Vec<u8>, String> {
        self.svc.user_get(&rec.owner, &FlowEnv::vault_name(&rec.id)).map(|s| s.as_bytes().to_vec()).map_err(|_| "the vault is locked or the secret is missing".to_string())
    }

    fn start(&self, rec: &HookRec, _cfg: &HookCfg, inputs: Value, key: Option<String>) -> Result<(String, bool), String> {
        let env = self.env(&rec.owner).ok_or("the owner has no flows environment")?;
        // as the owner, with the owner's account policy; no extra grants; the published version only
        let mut req = StartRequest::new(&rec.flow).trigger("webhook", &format!("hook:{}", rec.id)).inputs(inputs);
        req.trigger.detail = format!("webhook {}", rec.id);
        req.idempotency_key = key;
        env.engine.start_run(req).map(|o| (o.run_id, o.existing))
    }

    fn audit(&self, owner: &str, event: &str, detail: &str) {
        self.audit.record(owner, event, detail);
    }

    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
}

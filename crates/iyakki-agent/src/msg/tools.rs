//! The assistant's messaging tools: `message_accounts` lists the person's connected accounts and the room names they saved (no
//! token, no address of a room); `message_send` sends one text to a saved room by name and asks on a card that shows the account, the
//! room and the whole text, every time (`agent::hard_ask`: also in Auto mode; "Allow for this session" does not remember it).
//!
//! * Only room names the person saved in Settings are accepted, never a raw room id; the account's hourly cap, the encrypted-room
//!   refusal and the audit line (names and counts only) are the service's own ([`super::service::MessageService::send`]).
//! * `dry_run: true` checks the account, the room and the cap and sends nothing.
//! * Nothing read from a room is returned here: there is no inbound path in these tools.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

use super::service::MessageService;

pub const PROMPT_RULE: &str = "\nMESSAGES: message_accounts lists the person's connected messaging accounts and their saved room names. message_send sends ONE text to a saved room by name and asks the person every time, showing the text. Never put passwords, keys, tokens or other secrets in a message. Use dry_run first when unsure.";

pub const SEND: &str = "message_send";
pub const LIST: &str = "message_accounts";
/// The longest text one call sends.
pub const MAX_TEXT: usize = 4000;

pub fn is_message_tool(n: &str) -> bool {
    n == SEND || n == LIST
}

pub struct MessageTools {
    inner: Arc<dyn Tools>,
    svc: Arc<MessageService>,
}

impl MessageTools {
    pub fn new(inner: Arc<dyn Tools>, svc: Arc<MessageService>) -> MessageTools {
        MessageTools { inner, svc }
    }

    fn text(args: &Value, k: &str) -> String {
        args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
    }

    /// The checked call: account and room must exist by name, the text must be there and short enough.
    fn check(&self, args: &Value) -> Result<(String, String, String, bool), String> {
        let text = args.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if text.is_empty() {
            return Err("there is no text to send".into());
        }
        if text.chars().count() > MAX_TEXT {
            return Err(format!("the text is longer than {MAX_TEXT} characters; send a shorter one"));
        }
        let a = self.svc.accounts().pick(&Self::text(args, "account"))?;
        let r = self.svc.resolve_room(&a, &Self::text(args, "room"))?;
        Ok((a.label, r.name, text, args.get("dry_run").and_then(Value::as_bool).unwrap_or(false)))
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let (account, room, text, dry) = self.check(args)?;
        Ok(format!("{} a message to the room \"{room}\" on {account}:\n{text}", if dry { "Check (send nothing):" } else { "Send" }))
    }
}

impl Tools for MessageTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_message_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: LIST.into(), description: "List the person's messaging accounts (label, kind) and the room names they saved. No secrets.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: SEND.into(),
                description: "Send one text message to a saved room by name. The person approves a card showing the text, every time. Never include secrets. `dry_run` sends nothing.".into(),
                schema: json!({"type": "object", "properties": {
                    "text": {"type": "string", "description": "The message (at most 4000 characters)."},
                    "account": {"type": "string", "description": "The account's label or id; leave out when there is only one."},
                    "room": {"type": "string", "description": "A room name saved in Settings; leave out for the account's only or main room."},
                    "dry_run": {"type": "boolean", "description": "Check everything, send nothing."}
                }, "required": ["text"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            LIST => {
                let rows = self.svc.accounts().list();
                if rows.is_empty() {
                    return Ok("No messaging accounts yet (Settings, Notifications, Matrix account).".into());
                }
                Ok(rows.iter().map(|a| format!("{} ({}) rooms: {}", a.label, a.kind, self.svc.accounts().rooms(&a.id).iter().map(|r| r.name.clone()).collect::<Vec<_>>().join(", "))).collect::<Vec<_>>().join("\n"))
            }
            SEND => {
                let (account, room, text, dry) = self.check(args)?;
                let key = format!("assistant-{}", crate::clock::now_unix());
                let rep = self.svc.send(&account, &room, &text, None, &key, dry)?;
                Ok(format!("{} ({} / {})", rep.note, rep.account, rep.room))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            SEND => self.card(args).map(|c| c.lines().next().unwrap_or("").to_string()).unwrap_or_else(|e| format!("{name} (invalid: {e})")),
            LIST => "List the messaging accounts".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SEND => self.card(args).ok(),
            LIST => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SEND => self.card(args).err(),
            LIST => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        name != SEND && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != SEND && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SEND { Some("sending a message asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn begin_step(&self) {
        self.inner.begin_step()
    }

    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}

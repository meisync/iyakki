//! The INBOX facade: the plain shapes of what waits for a person's answer (a tool call of an agent run, a secret the assistant asked for, a
//! question about the assistant in the person's browser). The agent's `runs::PendingApproval`, `secretreq::SecretRequest` and
//! `browserconsent::{ConsentRequest, ConsentKind, ConsentChoice, Answer}` have the same fields; the shell converts between them
//! (`apps/inbox_port.rs`), and a shell test keeps the two copies in agreement (alpha.9 P5, D-P1-01).

/// One tool call that an agent run is waiting to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub run_id: String,
    pub user: String,
    /// The start of what the run was asked.
    pub prompt: String,
    pub call_id: String,
    pub tool: String,
    /// One line on what the call will do.
    pub summary: String,
    /// What exactly will change, when there is more to show.
    pub detail: String,
    /// `low`, `medium` or `high`.
    pub risk: &'static str,
    /// When the call started waiting.
    pub since: i64,
}

/// A secret the assistant asked the person for (the person types it; it goes to the vault, never to the assistant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRequest {
    pub id: String,
    pub user: String,
    /// The vault name suggested to the person (they may change it).
    pub name: String,
    /// What the secret is for, as the assistant described it.
    pub purpose: String,
    /// `password`, `api_key`, `token` or `other`.
    pub kind: String,
    pub since: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentKind {
    /// May the assistant look at this host in your browser?
    Look,
    /// May the assistant act on your behalf on this domain?
    Interact,
    /// One action to approve.
    Step,
    /// The assistant needs the person to do something (a code, a captcha) and press Continue.
    Handover,
    /// A flow step stopped for want of a permission only the person can give.
    Permission,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    AllowSession,
    Deny,
    ApproveAll,
    OneByOne,
    Continue,
    AllowOnce,
    AlwaysAllow,
    NotNow,
    Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentChoice {
    pub answer: Answer,
    pub label: String,
}

/// One question about the assistant in the person's browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRequest {
    pub id: String,
    pub user: String,
    pub kind: ConsentKind,
    pub domain: String,
    /// The short question (the row in the Approvals list).
    pub title: String,
    /// The full text of the dialog.
    pub text: String,
    /// What exactly would happen (steps), may be empty.
    pub detail: String,
    pub choices: Vec<ConsentChoice>,
    pub since: i64,
    /// What the answer acts on (a permission request: run, step, permission), never shown.
    pub payload: String,
    /// The popup was dismissed ("Not now"); the entry stays in the INBOX.
    pub quiet: bool,
}

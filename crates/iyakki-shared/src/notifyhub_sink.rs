//! The contract: what a producer (a flow's `notify` node, an approval, the cloud scheduler) hands to the hub. Nothing else is needed
//! to raise a notification; the hub decides who gets it, where and when.

use crate::inbox::Link;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Collected in the daily digest where a channel has one.
    Low,
    Normal,
    High,
    /// Goes through quiet hours.
    Critical,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Normal => "normal",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
    pub fn parse(s: &str) -> Option<Severity> {
        [Severity::Low, Severity::Normal, Severity::High, Severity::Critical].into_iter().find(|x| x.name() == s.trim().to_lowercase())
    }
}

/// A group of people named by what they do, not by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", content = "scope", rename_all = "snake_case")]
pub enum RoleSel {
    TenantAdmin,
    /// The admins of the notification's team.
    TeamAdmin,
    /// Holders of the approver role for this scope (an environment or a kind) in the team.
    Approver(String),
    /// A named group (the team's, else the tenant-wide one).
    Group(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "to", content = "who", rename_all = "snake_case")]
pub enum Recipients {
    People(Vec<String>),
    Roles(Vec<RoleSel>),
    /// Every member of the team (every member of the tenant when there is no team).
    Team,
}

/// One notification to raise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub tenant: String,
    pub team: Option<String>,
    pub recipients: Recipients,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    pub link: Option<Link>,
    /// The same key within an hour is one notification (empty: never deduplicated).
    pub dedupe_key: String,
    /// Who raised it: `flow:<name>`, `approval`, `scheduler`, ...
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub id: String,
    /// Dropped as a repeat of an earlier one with the same key.
    pub deduped: bool,
    /// The people it reached (their INBOX).
    pub recipients: Vec<String>,
    /// Channel deliveries queued.
    pub queued: usize,
}

/// What a producer needs. The cloud flows' `NotifySink` is this trait.
pub trait NotifySink: Send + Sync {
    fn emit(&self, n: &Notification) -> Result<Receipt, String>;
}

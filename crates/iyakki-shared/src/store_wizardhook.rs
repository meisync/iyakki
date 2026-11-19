//! A setup wizard an install or update of an app queued: the pending entry and the one line that says what the wizard will ask and run.
//! The file that keeps the list and the lookup of the installed apps' wizards stay in `iyakki_agent::store::wizardhook`.

use iyakki_core_api::wizard::WizardSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub app: String,
    pub wizard: String,
    pub title: String,
    pub trigger: String,
    #[serde(default)]
    pub from_version: Option<String>,
    pub queued_at: i64,
}

/// What a wizard will ask and run, in one plain line (the same facts as the card of `wizard_continue`, shown before anything starts).
pub fn outline(w: &WizardSpec) -> String {
    use iyakki_core_api::wizard::Step;
    let (mut forms, mut fields, mut checks, mut moves) = (0, 0, vec![], vec![]);
    for s in &w.steps {
        match s {
            Step::Form(f) => {
                forms += 1;
                fields += f.fields.len();
            }
            Step::Check(c) => checks.push(format!("{} {}", c.check, c.param.clone().unwrap_or_default()).trim().to_string()),
            Step::Migration(m) => moves.push(m.migration.clone()),
            _ => {}
        }
    }
    let mut parts = vec![];
    if forms > 0 {
        parts.push(format!("asks {fields} question{}", if fields == 1 { "" } else { "s" }));
    }
    if !checks.is_empty() {
        parts.push(format!("checks: {}", checks.join(", ")));
    }
    if !moves.is_empty() {
        parts.push(format!("moves data: {}", moves.join(", ")));
    }
    if parts.is_empty() { "has nothing to ask or run".into() } else { parts.join(" · ") }
}

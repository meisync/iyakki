//! The data of a flow's webhooks as the UI shows it (moved out of `iyakki_agent::flows::hookenv` for F-SPLIT-03).


use crate::hookstore::Delivery;


/// What a person (or the assistant) may choose when making a webhook.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HookOpts {
    pub signed: bool,
    pub methods: Vec<String>,
    pub max_body: usize,
    pub content_types: Vec<String>,
    pub rate_per_min: u32,
}

/// What making or renewing a hook gives back: shown once on the screen, never to the assistant. Over the core socket the secret does
/// not cross (D-SPLIT-03-SECRETS, D-S5-02): the wire form drops it.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Created {
    pub id: String,
    pub flow: String,
    /// `/hooks/<id>/<token>`.
    pub path: String,
    #[serde(skip)]
    pub secret: Option<String>,
}

impl std::fmt::Debug for Created {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Created({}, hidden)", self.id)
    }
}

/// A hook as the screen and the assistant see it: nothing secret.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

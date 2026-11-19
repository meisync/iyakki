//! What happens to one member of an install bundle (data only). The plan and the apply stay in `iyakki_agent::store::bundle`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberState {
    Install,
    /// Not installed; the text is on the card ("not supported by this IYAKKI", "needs a newer IYAKKI").
    Skip(String),
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use iyakki_core_api::appcaps::CardLine;

/// One group of the install card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCard {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub optional: bool,
    pub state: MemberState,
    pub lines: Vec<CardLine>,
    /// One plain sentence about what the member is or does on this computer, shown on the install card (the command line of an mcp server).
    pub note: String,
}

impl MemberCard {
    pub fn name(&self) -> &str {
        if self.title.is_empty() { &self.id } else { &self.title }
    }
}

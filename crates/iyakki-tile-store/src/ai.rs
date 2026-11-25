//! The AI hardware profile as the Setup tab reads it (alpha.10 AP3, D-AP3-03 (1)): a plain `Vis` (the same five answers as the agent's
//! profile, with the same `shown` and `reason` rules) and the port the tab calls. The real port is `AgentAi` in
//! `crates/iyakki-apps/src/apps/store_port.rs`.

/// What the profile says about one AI tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vis {
    Show,
    /// Shown, and the reason is that the cluster has the GPU this machine lacks.
    ShownBecause(String),
    /// Shown with a star and the reason.
    Recommended(String),
    /// Shown, with the reason it is not a good idea here.
    NotRecommended(String),
    /// Not shown (unless "Show all AI tools" is on), with the reason.
    Hidden(String),
}

impl Vis {
    pub fn shown(&self) -> bool {
        !matches!(self, Vis::Hidden(_))
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Vis::Show => None,
            Vis::ShownBecause(r) | Vis::Recommended(r) | Vis::NotRecommended(r) | Vis::Hidden(r) => Some(r),
        }
    }
}

/// The profile of this machine, as the Setup tab asks it.
pub trait AiProfilePort: Send + Sync + std::fmt::Debug {
    /// The facts line ("Apple M2 Pro, 32 GB unified memory ...").
    fn summary(&self) -> String;
    /// What the profile says about one install item (`item_id` is the item's id); `show_all` is the administrator's switch.
    fn visibility(&self, item_id: &str, show_all: bool) -> Vis;
    /// The AI tools hidden on this profile, each with its reason: (item id, reason).
    fn hidden_items(&self) -> Vec<(String, String)>;
}

/// Two profiles are the same when they say the same facts line and hide the same tools (the Setup tab compares machines on this).
impl PartialEq for dyn AiProfilePort {
    fn eq(&self, other: &Self) -> bool {
        self.summary() == other.summary() && self.hidden_items() == other.hidden_items()
    }
}

/// A profile with canned answers (tests): `summary`, the visibility of each item id (`Show` when not listed), and the hidden list.
#[derive(Default, Debug)]
pub struct FakeAiProfile {
    pub summary: String,
    pub vis: Vec<(String, Vis)>,
    pub hidden: Vec<(String, String)>,
}

impl AiProfilePort for FakeAiProfile {
    fn summary(&self) -> String {
        self.summary.clone()
    }
    fn visibility(&self, item_id: &str, show_all: bool) -> Vis {
        match self.vis.iter().find(|(i, _)| i == item_id).map(|(_, v)| v.clone()) {
            Some(Vis::Hidden(r)) if show_all => Vis::NotRecommended(r),
            Some(v) => v,
            None => Vis::Show,
        }
    }
    fn hidden_items(&self) -> Vec<(String, String)> {
        self.hidden.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_and_reason_follow_the_profile_rules() {
        assert!(Vis::Show.shown() && Vis::Show.reason().is_none());
        assert!(Vis::ShownBecause("x".into()).shown() && Vis::ShownBecause("x".into()).reason() == Some("x"));
        assert!(!Vis::Hidden("why".into()).shown() && Vis::Hidden("why".into()).reason() == Some("why"));
    }

    #[test]
    fn show_all_turns_a_hidden_tool_into_not_recommended_and_unlisted_tools_show() {
        let p = FakeAiProfile { vis: vec![("ollama".into(), Vis::Hidden("no GPU".into()))], ..Default::default() };
        assert_eq!(p.visibility("ollama", false), Vis::Hidden("no GPU".into()));
        assert_eq!(p.visibility("ollama", true), Vis::NotRecommended("no GPU".into()));
        assert_eq!(p.visibility("unlisted", false), Vis::Show);
    }
}

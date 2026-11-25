//! The few fixed words and rules the chat transcript view shares with the agent: how a card summary is taken apart, which camera
//! tools take pictures, the note under a camera card and the mark of a spoken turn. Pure text, so the UI-layer chat view can name them
//! without the agent. `iyakki_shared::{grants, camtools, voiceassist}` re-export every item under the old paths.

/// The text a card appends to the summary of a call that can never be covered by a session grant.
pub const ALWAYS_ASKS: &str = "\nalways asks: ";

/// The text a card appends to the summary of a call that "Allow for this session" would cover, and what it would cover.
pub const SESSION_COVERS: &str = "\nallow for this session covers: ";

pub const ASK_NOTE: &str = "The camera is private: the assistant asks every time; Allow for 5 minutes is the longest it may keep using it.";

/// What a turn the person SPOKE starts with in the Voice conversation thread, so it can be told from one typed into the same thread
/// later (F-VOICE-40). A transcript can hold recognition mistakes; the model sees the mark too and may ask when a word looks wrong.
pub const SPOKEN_MARK: &str = "(spoken) ";

/// The tools that take pictures: asked every time unless a 5-minute grant is running.
pub fn takes_pictures(name: &str) -> bool {
    matches!(name, "camera_photo" | "camera_record_start" | "camera_passport")
}

/// Split a card summary into the question and the `always asks` reason, if there is one.
pub fn split_summary(s: &str) -> (&str, Option<&str>) {
    match s.split_once(ALWAYS_ASKS) {
        Some((q, why)) => (q, Some(why)),
        None => (s, None),
    }
}

/// A card summary taken apart: the question, why it always asks (no session button), or what a session grant would cover.
#[derive(Debug, PartialEq, Eq)]
pub struct Card<'a> {
    pub question: &'a str,
    pub always: Option<&'a str>,
    pub covers: Option<&'a str>,
}

pub fn split_card(s: &str) -> Card<'_> {
    if let (q, Some(why)) = split_summary(s) {
        return Card { question: q, always: Some(why), covers: None };
    }
    match s.split_once(SESSION_COVERS) {
        Some((q, c)) => Card { question: q, always: None, covers: Some(c) },
        None => Card { question: s, always: None, covers: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_split_three_ways() {
        assert_eq!(split_card("Run it?"), Card { question: "Run it?", always: None, covers: None });
        let a = format!("Run it?{ALWAYS_ASKS}it deletes");
        assert_eq!(split_card(&a), Card { question: "Run it?", always: Some("it deletes"), covers: None });
        let c = format!("Run it?{SESSION_COVERS}all git calls");
        assert_eq!(split_card(&c), Card { question: "Run it?", always: None, covers: Some("all git calls") });
        assert_eq!(split_summary(&a), ("Run it?", Some("it deletes")));
    }

    #[test]
    fn only_the_picture_taking_camera_tools_are_named() {
        assert!(takes_pictures("camera_photo") && takes_pictures("camera_record_start") && takes_pictures("camera_passport"));
        assert!(!takes_pictures("camera_list") && !takes_pictures("camera_record_stop"));
        assert!(SPOKEN_MARK.ends_with(' ') && ASK_NOTE.contains("5 minutes"));
    }
}

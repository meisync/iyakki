//! Types moved out of `iyakki_agent::skills` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const MAX_BODY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Scope {
    Shared,
    User(String),
}

impl Scope {
    pub fn ns(&self) -> String {
        match self {
            Scope::Shared => "skills/shared".into(),
            Scope::User(u) => format!("skills/user/{u}"),
        }
    }
    pub fn label(&self) -> &str {
        match self {
            Scope::Shared => "shared",
            Scope::User(_) => "mine",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Skill {
    pub scope: Scope,
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub body: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "SkillErrorWire")]
pub enum SkillError {
    #[error("{0}")]
    Invalid(String),
    #[error("you may not change {0} skills")]
    Forbidden(#[serde(skip_deserializing)] &'static str),
    #[error("no such skill")]
    NotFound,
    #[error("storage error: {0}")]
    Storage(String),
}

/// What a [`SkillError`] looks like on the wire (the `&'static str` of `Forbidden` arrives as text).
#[derive(serde::Deserialize)]
enum SkillErrorWire {
    Invalid(String),
    Forbidden(String),
    NotFound,
    Storage(String),
}

impl From<SkillErrorWire> for SkillError {
    fn from(w: SkillErrorWire) -> SkillError {
        match w {
            SkillErrorWire::Invalid(s) => SkillError::Invalid(s),
            SkillErrorWire::Forbidden(s) => SkillError::Forbidden(iyakki_serde_static::intern(&s)),
            SkillErrorWire::NotFound => SkillError::NotFound,
            SkillErrorWire::Storage(s) => SkillError::Storage(s),
        }
    }
}

pub fn valid_skill_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit()) && n.len() <= 48 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '-')
}

//! Types moved out of `iyakki_agent::skills` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const MAX_BODY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub scope: Scope,
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub body: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SkillError {
    #[error("{0}")]
    Invalid(String),
    #[error("you may not change {0} skills")]
    Forbidden(&'static str),
    #[error("no such skill")]
    NotFound,
    #[error("storage error: {0}")]
    Storage(String),
}

pub fn valid_skill_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit()) && n.len() <= 48 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '-')
}

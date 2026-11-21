
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn label(self) -> &'static str {
        match self {
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
        }
    }

    pub fn from_label(s: &str) -> Option<Bump> {
        [Bump::Patch, Bump::Minor, Bump::Major].into_iter().find(|b| b.label() == s.trim().to_lowercase())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    pub kind: String,
    pub name: String,
    pub version: String,
    /// Things to fix before anything can be published, each in plain words.
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
    /// For a tile app: the headless tests ran and passed (None: not run, e.g. not a tile app).
    pub tests: Option<bool>,
    /// `CHANGELOG.md` already has a line for this version.
    pub changelog_has_version: bool,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoreChoice {
    pub id: String,
    pub name: String,
    pub address: String,
    pub fingerprint: String,
    pub is_default: bool,
    /// A private store with a pinned key (the only kind that takes a publish).
    pub private: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Card {
    pub store: String,
    pub server: String,
    pub https: bool,
    pub http_warning: Option<String>,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub permissions: Vec<String>,
    /// The level the package will probably get, in words.
    pub trust: String,
    pub new_key: bool,
    pub key_warning: Option<String>,
    pub key_fingerprint: String,
    pub size: usize,
    pub files: Vec<String>,
    pub store_has: Option<String>,
    pub token_present: bool,
    pub yank_note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Stop {
    /// Ask for the token in the masked dialog, then try again.
    NeedsToken,
    /// There is no signing key: create one.
    NeedsKey,
    Refused(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::NeedsToken => write!(f, "no publish token for this store yet: enter it in the dialog (it goes into your vault)"),
            Stop::NeedsKey => write!(f, "you have no publisher signing key yet: create one first"),
            Stop::Refused(m) => write!(f, "{m}"),
        }
    }
}

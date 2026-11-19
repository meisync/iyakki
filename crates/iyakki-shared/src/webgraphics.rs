//! Types moved out of `iyakki_agent::webgraphics` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Automatic,
    Software,
    Hardware,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Mode::Automatic => "automatic",
            Mode::Software => "software",
            Mode::Hardware => "hardware",
        }
    }
    pub fn from_id(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "automatic" | "auto" => Some(Mode::Automatic),
            "software" | "software (safe)" | "safe" => Some(Mode::Software),
            "hardware" => Some(Mode::Hardware),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Automatic => "Automatic",
            Mode::Software => "Software (safe)",
            Mode::Hardware => "Hardware",
        }
    }
    /// The next choice in the order the Settings key cycles through.
    pub fn next(self) -> Mode {
        match self {
            Mode::Automatic => Mode::Software,
            Mode::Software => Mode::Hardware,
            Mode::Hardware => Mode::Automatic,
        }
    }
}

/// What Settings and the chat tool show (reads only; nothing is judged or written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub mode: Mode,
    pub software: bool,
    pub gpu: String,
    /// `Automatic: hardware`, `Automatic: software because ...`, `Software (safe)`, `Hardware`.
    pub line: String,
    pub detail: String,
    pub fallback: bool,
    pub offer: bool,
    pub read_only: Option<String>,
}

/// A sentence for the shell to show and record, queued by [`apply`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pending {
    /// The full sentence for the browser's status line.
    pub status: Option<String>,
    /// A short sentence for the toast (the toast is one line).
    pub toast: Option<String>,
    pub audit: Option<String>,
}


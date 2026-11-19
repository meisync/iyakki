use crate::settings::ModelChoice;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// On this computer.
    Local,
    /// On another machine of the network (a private address or a `.local` name).
    Network,
    /// A service on the internet.
    Cloud,
}

impl Place {
    /// Where the model is, with the host of its provider: the words used wherever the person is told where a picture goes.
    pub fn describe(self, host: &str) -> String {
        match self {
            Place::Local => "on this computer".into(),
            Place::Network => format!("on another computer on your network ({host})"),
            Place::Cloud => format!("a CLOUD service ({host})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Own,
    Setting,
    Chat,
    Auto,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub provider: String,
    pub model: String,
    pub place: Place,
    /// The host of the provider's address (what the person sees next to "on another computer on your network").
    pub host: String,
    pub how: How,
}

impl Pick {
    pub fn choice(&self) -> ModelChoice {
        ModelChoice { provider: self.provider.clone(), model: self.model.clone() }
    }
    /// "Using qwen2.5-vl-7b on lemonade-local: on this computer" (shown in the log and the permission text).
    pub fn line(&self) -> String {
        format!("Using {} on {}: {}{}.", self.model, self.provider, self.place.describe(&self.host), match self.how {
            How::Setting => " (your Vision model setting)",
            How::Chat => " (the chat model)",
            How::Auto => " (picked automatically)",
            How::Own => "",
        })
    }
}

/// Why no vision model could be chosen; both are "needs your choice" failures (the fixing agent is not asked).
#[derive(Clone, Debug, PartialEq)]
pub enum VisionError {
    NoneFound(String),
    CloudOnly(String),
}

impl VisionError {
    pub fn message(&self) -> &str {
        match self {
            VisionError::NoneFound(m) | VisionError::CloudOnly(m) => m,
        }
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// A model that can see pictures, as a provider's list says.
#[derive(Clone, Debug, PartialEq)]
pub struct Cand {
    pub provider: String,
    pub model: String,
    pub place: Place,
    pub host: String,
    /// The server itself labels it vision (not only its name).
    pub labelled: bool,
    pub size_gb: Option<f64>,
    /// In the server's memory now (otherwise the first call has to load it, which takes time and memory).
    pub loaded: bool,
}

impl Cand {
    /// What the server says comes first (its `vision` label is trusted even when the name shows nothing, `Qwen3.5-4B-MTP-GGUF`,
    /// `Gemma-4-E2B-it-GGUF`): 0 labelled Qwen, 1 other labelled, 2 name-only Qwen vision, 3 other names.
    pub fn group(&self) -> u8 {
        let m = self.model.to_ascii_lowercase();
        match (self.labelled, m.contains("qwen")) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        }
    }
    pub fn label(&self) -> String {
        format!("{}/{}  ({})", self.provider, self.model, self.place.describe(&self.host))
    }
}

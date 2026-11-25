//! The plain types of a Lemonade server's model list: what a model does and one listed model, with the words the screens print.
//! `iyakki_shared::lemonade` re-exports them (alpha.10 Q3, D-P6-03).

use crate::ImageDefaults;

/// What a model does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ModelType {
    Chat,
    Vision,
    Embeddings,
    Reranking,
    Image,
    SpeechToText,
    TextToSpeech,
    /// Anything else Lemonade serves (music, 3D, classification): listed, not used.
    Other,
}

impl ModelType {
    /// The sections of Settings → Models, in order.
    pub const SECTIONS: [ModelType; 8] = [ModelType::Chat, ModelType::Vision, ModelType::Embeddings, ModelType::Reranking, ModelType::Image, ModelType::SpeechToText, ModelType::TextToSpeech, ModelType::Other];

    pub fn label(self) -> &'static str {
        match self {
            ModelType::Chat => "Chat",
            ModelType::Vision => "Vision",
            ModelType::Embeddings => "Embeddings",
            ModelType::Reranking => "Reranking",
            ModelType::Image => "Image",
            ModelType::SpeechToText => "Speech-to-text",
            ModelType::TextToSpeech => "Text-to-speech",
            ModelType::Other => "Other",
        }
    }

    /// The word saved in `ProviderCfg::model_kinds`.
    pub fn key(self) -> &'static str {
        match self {
            ModelType::Chat => "chat",
            ModelType::Vision => "vision",
            ModelType::Embeddings => "embeddings",
            ModelType::Reranking => "reranking",
            ModelType::Image => "image",
            ModelType::SpeechToText => "stt",
            ModelType::TextToSpeech => "tts",
            ModelType::Other => "other",
        }
    }

    pub fn from_key(k: &str) -> Option<ModelType> {
        Self::SECTIONS.into_iter().find(|t| t.key() == k)
    }

    /// Can answer the chat (a vision model can, and also sees pictures).
    pub fn chats(self) -> bool {
        matches!(self, ModelType::Chat | ModelType::Vision)
    }
}


/// One model of a Lemonade server.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: String,
    pub kind: ModelType,
    pub recipe: String,
    pub checkpoint: String,
    pub labels: Vec<String>,
    /// On the server's disk (it can be used); otherwise it has to be pulled first.
    pub downloaded: bool,
    /// In memory now (the first request is fast).
    pub loaded: bool,
    pub suggested: bool,
    /// Download size in GB, when the list says.
    pub size_gb: Option<f64>,
    pub image_defaults: Option<ImageDefaults>,
}

impl Model {
    /// "loaded", "available" or "not downloaded".
    pub fn status(&self) -> &'static str {
        if self.loaded {
            "loaded"
        } else if self.downloaded {
            "available"
        } else {
            "not downloaded"
        }
    }

    /// "6.9 GB", or "size unknown".
    pub fn size_text(&self) -> String {
        match self.size_gb {
            Some(g) if g >= 1.0 => format!("{g:.1} GB"),
            Some(g) if g > 0.0 => format!("{} MB", (g * 1024.0).round() as u64),
            _ => "size unknown".into(),
        }
    }
}


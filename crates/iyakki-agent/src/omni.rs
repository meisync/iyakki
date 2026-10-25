//! "Use all parts" of a Lemonade collection (`LMX-Omni-5.5B-Lite`: a chat and vision model, a picture model, speech in and
//! speech out, served together). One step sets the chat model, the TALES picture model, speech-to-text and the reading voice
//! to the parts, all or none, and remembers what they were for Undo. Missing parts are listed with their sizes and installed
//! with the ordinary pull (streamed progress).

use std::sync::Arc;

use crate::lemonade::{self, Collection, Model, UsePlan};
use crate::services::Services;
use crate::settings::ModelChoice;
use crate::story::StoryStore;
use crate::voice::SpeechEngine;

/// What "Use all parts" replaced.
#[derive(Clone, Debug, PartialEq)]
pub struct AllPartsUndo {
    pub provider: String,
    pub collection: String,
    chat: Option<ModelChoice>,
    default_model: Option<String>,
    stt: Option<SpeechEngine>,
    tts: Option<SpeechEngine>,
    picture: Option<ModelChoice>,
}

/// Where a collection stands before it can be used.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub collection: Model,
    pub plan: UsePlan,
    /// The collection or some part is not on the server (or the server lists no parts yet): install first.
    pub needs_install: bool,
}

impl Review {
    /// The parts that are missing, one per line: "pictures SD-Turbo: missing (2.6 GB)".
    pub fn missing_lines(&self) -> Vec<String> {
        if self.plan.parts.is_empty() {
            return vec![format!("{}: the parts are listed once it is downloaded ({})", self.collection.id, self.collection.size_text())];
        }
        self.plan.missing().iter().map(|p| format!("{}: {}", p.title(), p.state())).collect()
    }
}

impl Services {
    /// The collection `collection` of the Lemonade provider `provider` (from the last list read), with its parts resolved.
    pub fn collection_review(&self, provider: &str, collection: &str) -> Result<Review, String> {
        let cfg = self.settings.lock().unwrap().get().provider(provider).cloned().ok_or("unknown provider")?;
        if cfg.kind != crate::providers::Kind::Lemonade {
            return Err(format!("{provider} is not a Lemonade server"));
        }
        let base = lemonade::api_base(&cfg.base()?);
        let models = self.lemonade_cached(provider);
        let model = models.iter().find(|m| m.id == collection && lemonade::is_collection(m)).cloned().ok_or_else(|| format!("{collection} is not a collection on {provider}"))?;
        let c = lemonade::collections_cached(&base).into_iter().find(|c| c.id == collection).unwrap_or(Collection { id: collection.into(), recipe: model.recipe.clone(), ..Default::default() });
        let plan = UsePlan::new(&c, &models);
        let needs_install = !model.downloaded || plan.parts.is_empty() || !plan.missing().is_empty();
        Ok(Review { collection: model, plan, needs_install })
    }

    /// Download what is missing: the collection itself (Lemonade fetches its parts), or, when that download failed before,
    /// the missing parts one by one. Progress shows in `pull_status`. Returns the ids being downloaded.
    pub fn install_missing_parts(self: &Arc<Self>, provider: &str, collection: &str) -> Result<Vec<String>, String> {
        let r = self.collection_review(provider, collection)?;
        if !r.needs_install {
            return Err("every part is installed already".into());
        }
        let failed = self.pull_status().into_iter().any(|p| p.provider == provider && p.model == collection && p.error.is_some());
        let ids = if failed && !r.plan.parts.is_empty() { r.plan.missing_ids() } else { vec![collection.to_string()] };
        for id in &ids {
            self.pull_model(provider, id)?;
        }
        Ok(ids)
    }

    /// Set the chat model, the TALES picture model, speech-to-text and the reading voice to the parts of `collection`: all four
    /// or none (a step that fails puts back the steps before it and names the part). The speech server must already be
    /// allowed when it is on another machine. `store` is the signed-in user's TALES store.
    ///
    /// How a collection is called is not established from this code (Lemonade may serve chat through the collection's own id,
    /// routing tool calls to its parts); chat is set to the collection's chat part, which is a plain model every Lemonade
    /// serves, so the choice works either way.
    pub fn use_all_parts(&self, provider: &str, collection: &str, store: &StoryStore) -> Result<String, String> {
        let r = self.collection_review(provider, collection)?;
        if r.needs_install {
            return Err(format!("not every part is installed on {provider}: {}", r.missing_lines().join("; ")));
        }
        let lacking = r.plan.lacking();
        if !lacking.is_empty() {
            return Err(format!("{collection} has no part for {}; nothing was changed", lacking.join(", ")));
        }
        let (chat, image, stt, tts) = (r.plan.chat.clone().unwrap(), r.plan.image.clone().unwrap(), r.plan.stt.clone().unwrap(), r.plan.tts.clone().unwrap());
        let cfg = self.settings.lock().unwrap().get().provider(provider).cloned().ok_or("unknown provider")?;
        let base = cfg.base()?;
        if !self.speech_server_allowed(&base) {
            return Err(format!("{provider} is {}: speech would be sent there. Say yes to that first; nothing was changed.", crate::speechtime::locality(&base).label()));
        }
        let before = self.settings.lock().unwrap().get().clone();
        let prefs = store.load_prefs();
        let undo = AllPartsUndo { provider: provider.into(), collection: collection.into(), chat: before.chat.clone(), default_model: cfg.default_model.clone(), stt: before.voice.stt.clone(), tts: before.voice.tts.clone(), picture: prefs.picture_model.clone() };
        let choice = |m: &str| ModelChoice { provider: provider.into(), model: m.into() };
        let step = |what: &str, f: &mut dyn FnMut() -> Result<(), String>| -> Result<(), String> {
            f().map_err(|e| {
                let _ = self.restore(&undo, store);
                format!("{what} ({}) could not be set: {e}. Nothing was changed.", match what { "chat" => &chat.id, "pictures" => &image.id, "speech in" => &stt.id, _ => &tts.id })
            })
        };
        let upd = |f: &mut dyn FnMut(&mut crate::settings::Settings)| self.settings.lock().unwrap().update(|s| f(s)).map_err(|e| e.to_string());
        step("chat", &mut || {
            upd(&mut |s| {
                s.chat = Some(choice(&chat.id));
                if let Some(pr) = s.providers.iter_mut().find(|x| x.id == provider) {
                    pr.default_model = Some(chat.id.clone());
                }
            })
        })?;
        step("speech in", &mut || upd(&mut |s| s.voice.stt = Some(lemonade::stt_engine(&base, &stt.id, cfg.key_ref.clone()))))?;
        step("speech out", &mut || upd(&mut |s| s.voice.tts = Some(lemonade::tts_engine(&base, &tts.id, None, cfg.key_ref.clone()))))?;
        step("pictures", &mut || {
            let mut p = store.load_prefs();
            p.picture_model = Some(choice(&image.id));
            store.save_prefs(&p)
        })?;
        *self.parts_undo.lock().unwrap() = Some(undo);
        Ok(format!("{collection} on {provider}: chat {}, pictures {}, speech in {}, speech out {}.", chat.id, image.id, stt.id, tts.id))
    }

    fn restore(&self, u: &AllPartsUndo, store: &StoryStore) -> Result<(), String> {
        self.settings
            .lock()
            .unwrap()
            .update(|s| {
                s.chat = u.chat.clone();
                s.voice.stt = u.stt.clone();
                s.voice.tts = u.tts.clone();
                if let Some(pr) = s.providers.iter_mut().find(|x| x.id == u.provider) {
                    pr.default_model = u.default_model.clone();
                }
            })
            .map_err(|e| e.to_string())?;
        let mut p = store.load_prefs();
        p.picture_model = u.picture.clone();
        store.save_prefs(&p)
    }

    /// What the last "Use all parts" replaced, if Undo is still possible.
    pub fn parts_undo_available(&self) -> Option<(String, String)> {
        self.parts_undo.lock().unwrap().as_ref().map(|u| (u.provider.clone(), u.collection.clone()))
    }

    /// Put back the chat model, speech engines and picture model that "Use all parts" replaced.
    pub fn undo_all_parts(&self, store: &StoryStore) -> Result<String, String> {
        let u = self.parts_undo.lock().unwrap().clone().ok_or("there is nothing to undo")?;
        self.restore(&u, store)?;
        *self.parts_undo.lock().unwrap() = None;
        Ok(format!("Put back the chat model, pictures, speech-to-text and reading voice that were set before {} was used.", u.collection))
    }
}

//! Settings → Models for AMD's Lemonade Server: Detect (Ollama and Lemonade on this machine), the Auto-detect server type of
//! the provider form, and under a chosen Lemonade provider its models by type (Chat, Vision, Embeddings, Reranking, Image,
//! Speech-to-text, Text-to-speech, and Video: none) with their state (loaded, available, not downloaded). A click on a model
//! offers what fits its type: use it as the chat model, for embeddings, as the TALES picture model, as the reading voice or
//! for speech-to-text, and Load / Pull, which ask first on a card naming the model and its size.

use crate::safeclamp::SafeClamp;
use super::*;
use iyakki_agent::lemonade::{self, Model, ModelType};
use iyakki_agent::speechtime;

/// What the Lemonade pickers are choosing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LemPick {
    /// A model of the provider.
    Model(String),
    /// What to do with (provider, model).
    Action(String, String),
    /// The reading voice of (provider, model).
    Voice(String, String),
}

/// What the approval card is for: (provider, model).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LemAct {
    Pull(String, String),
    Load(String, String),
    /// Yes to sending what is read aloud to a server on another machine, then (provider, model, voice) reads.
    RemoteVoice(String, String, String),
    /// Yes to sending microphone audio and made speech to a server on another machine, then (provider, model) listens.
    RemoteStt(String, String),
    /// Yes to "Use all parts" of the collection (provider, collection); for a server on another machine this is also the yes to
    /// sending speech there.
    UseAll(String, String),
    /// Yes to downloading what a collection is missing (provider, collection).
    InstallParts(String, String),
}

/// The hit id of the search box over the model list (`Event::Focus` becomes `FocusAt`).
pub(super) const LEM_SEARCH_ID: usize = 901;

const USE_CHAT: &str = "Use as chat model";
const USE_EMBED: &str = "Use for embeddings (RAG)";
const USE_PICTURES: &str = "Use as TALES picture model";
const USE_VOICE: &str = "Use as reading voice…";
const USE_STT: &str = "Use as speech-to-text";
const USE_ALL: &str = "Use all parts…";
const UNDO_ALL: &str = "Undo Use all parts";
const INSTALL_PARTS: &str = "Install missing parts";
const LOAD: &str = "Load into memory…";
const UNLOAD: &str = "Unload from memory";

/// "Image · SD-Turbo · available · 2.6 GB": a line of the model picker (the id is the second part).
fn model_line(m: &Model) -> String {
    format!("{} · {} · {} · {}", m.kind.label(), m.id, m.status(), m.size_text())
}

fn status_color(m: &Model) -> [u8; 4] {
    if m.loaded {
        OK
    } else if m.downloaded {
        TEXT
    } else {
        DIM
    }
}

impl SettingsApp {
    /// Detect: look for Ollama and Lemonade on this machine and add what answers (never changing a provider already there).
    pub(super) fn detect_servers(&mut self, cx: &AppCx) {
        let svc = Self::svc(cx).clone();
        cx.audit.record(&cx.session.user, "settings.provider.detect", "this machine");
        self.start("looking for Ollama and Lemonade on this machine", move || {
            let found = svc.detect_local_servers();
            JobOut::Msg(if found.is_empty() {
                Err("No Ollama or Lemonade server answered on this machine (ports 11434, 13305 and 8000). Start one, or add a server on another machine with Add and the type Auto-detect.".into())
            } else {
                Ok(found.join(" · "))
            })
        });
    }

    /// The provider form with the type Auto-detect: the address is asked (GET requests only) what it is, then saved as that.
    pub(super) fn save_provider_auto(&mut self, cx: &AppCx, form: &Form, editing: Option<String>) -> Result<(), String> {
        let id = editing.clone().unwrap_or_else(|| form.value(0));
        if !valid_provider_id(&id) {
            return Err("provider id: lowercase letters, digits, - and _; must start with a letter".into());
        }
        if editing.is_none() && Self::providers(cx).iter().any(|p| p.id == id) {
            return Err(format!("A provider named `{id}` already exists."));
        }
        let url = form.value(2);
        if lemonade::origin(&url).is_none() {
            return Err("Type the server's address (for example http://192.168.1.20:13305) so its type can be detected.".into());
        }
        let (key, model) = (form.raw(3).to_string(), form.value(4));
        let existing = Self::providers(cx).into_iter().find(|p| Some(&p.id) == editing.as_ref());
        let svc = Self::svc(cx).clone();
        cx.audit.record(&cx.session.user, "settings.provider.detect", &format!("{id} {url}"));
        self.start("detecting the server type", move || {
            let headers: Vec<(String, String)> = if key.is_empty() { vec![] } else { vec![("Authorization".into(), format!("Bearer {key}"))] };
            let server = match lemonade::detect(&url, svc.quick_upstream().as_ref(), &headers) {
                Ok(s) => s,
                Err(e) => return JobOut::Msg(Err(e)),
            };
            let mut cfg = existing.unwrap_or_else(|| ProviderCfg::new(&id, server.kind()));
            cfg.kind = server.kind();
            cfg.base_url = Some(server.base().to_string());
            if !model.is_empty() {
                cfg.default_model = Some(model);
            }
            if !key.is_empty() {
                cfg.key_ref = Some(format!("vault:{id}-key"));
            }
            if let Err(e) = cfg.validate() {
                return JobOut::Msg(Err(e));
            }
            if !key.is_empty() {
                if let Err(e) = svc.system_set(&format!("{id}-key"), &key) {
                    return JobOut::Msg(Err(e.to_string()));
                }
            }
            if let Err(e) = svc.settings.lock().unwrap().upsert_provider(cfg) {
                return JobOut::Msg(Err(e.to_string()));
            }
            let extra = if server.kind() == Kind::Lemonade {
                match svc.refresh_lemonade(&id) {
                    Ok(list) => format!(" {} models read (press l on it to see them by type).", list.len()),
                    Err(e) => format!(" Its model list could not be read yet: {e}"),
                }
            } else {
                String::new()
            };
            JobOut::Msg(Ok(format!("{}; saved as {id}.{extra}", server.found())))
        });
        Ok(())
    }

    /// The models of a Lemonade provider in a picker (read from the server first when nothing is known yet).
    pub(super) fn lem_open(&mut self, cx: &AppCx, id: &str) {
        let list = Self::svc(cx).lemonade_cached(id);
        if list.is_empty() {
            let (svc, id) = (Self::svc(cx).clone(), id.to_string());
            self.start("reading the Lemonade models", move || {
                JobOut::Msg(svc.refresh_lemonade(&id).map(|l| format!("{id}: {} models read; press l again to choose one.", l.len())).map_err(|e| format!("{id}: {e}")))
            });
            return;
        }
        let items: Vec<String> = list.iter().map(model_line).collect();
        self.modal = Some(Modal::Pick(PickKind::Lemonade(LemPick::Model(id.to_string())), Picker::new(&format!("Models on {id}"), items)));
    }

    /// A model chip of the panel was clicked: what can be done with it.
    pub(super) fn lem_chip(&mut self, i: usize, cx: &AppCx) {
        let Some(p) = Self::providers(cx).get(self.sel()).filter(|p| p.kind == Kind::Lemonade).cloned() else { return };
        let Some(m) = Self::svc(cx).lemonade_cached(&p.id).get(i).cloned() else { return };
        self.lem_actions_for(cx, &p.id, &m);
    }

    fn lem_actions_for(&mut self, cx: &AppCx, provider: &str, m: &Model) {
        if !self.admin(cx) {
            return self.fail(format!("{} is {} on {provider}. Only an administrator can change what it is used for.", m.id, m.status()));
        }
        let mut items: Vec<String> = Vec::new();
        if lemonade::is_collection(m) {
            return self.lem_collection_actions(cx, provider, m);
        }
        if !m.downloaded {
            items.push(format!("Pull (download {})…", m.size_text()));
        } else {
            match m.kind {
                ModelType::Chat | ModelType::Vision => items.push(USE_CHAT.into()),
                ModelType::Embeddings => items.push(USE_EMBED.into()),
                ModelType::Image => items.push(USE_PICTURES.into()),
                ModelType::SpeechToText => items.push(USE_STT.into()),
                ModelType::TextToSpeech => items.push(USE_VOICE.into()),
                ModelType::Reranking | ModelType::Other => {}
            }
            items.push(if m.loaded { UNLOAD.into() } else { LOAD.into() });
        }
        let title = format!("{} ({}, {})", m.id, m.kind.label(), m.status());
        self.modal = Some(Modal::Pick(PickKind::Lemonade(LemPick::Action(provider.to_string(), m.id.clone())), Picker::new(&title, items)));
    }

    /// "this computer" / "the local network" / "the internet", with the host: where a provider's answers, pictures and voice come from.
    fn lem_where(cx: &AppCx, provider: &str) -> String {
        let place = Self::lem_base(cx, provider).map(|b| speechtime::locality(&b).label()).unwrap_or("");
        format!("{provider} ({}, {place})", Self::lem_host(cx, provider))
    }

    /// A collection: its parts with their state, then Use all parts, Install missing parts and Undo.
    fn lem_collection_actions(&mut self, cx: &AppCx, provider: &str, m: &Model) {
        let Ok(r) = Self::svc(cx).collection_review(provider, &m.id) else { return self.fail(format!("{} is no longer listed on {provider}.", m.id)) };
        let mut items: Vec<String> = Vec::new();
        if r.plan.parts.is_empty() {
            items.push(format!("  parts: listed once it is downloaded ({})", m.size_text()));
        }
        items.extend(r.plan.parts.iter().map(|p| format!("  {} · {}", p.title(), p.state())));
        items.push(USE_ALL.into());
        if r.needs_install {
            items.push(format!("{INSTALL_PARTS} ({})…", r.plan.missing_gb().map_or_else(|| m.size_text(), |g| format!("{g:.1} GB"))));
        }
        if Self::svc(cx).parts_undo_available().is_some_and(|(p, c)| p == provider && c == m.id) {
            items.push(UNDO_ALL.into());
        }
        let title = format!("{} (Omni collection, {})", m.id, m.status());
        self.modal = Some(Modal::Pick(PickKind::Lemonade(LemPick::Action(provider.to_string(), m.id.clone())), Picker::new(&title, items)));
    }

    fn lem_collection_pick(&mut self, cx: &AppCx, provider: &str, collection: &str, item: &str) {
        let place = Self::lem_where(cx, provider);
        let Ok(r) = Self::svc(cx).collection_review(provider, collection) else { return self.fail(format!("{collection} is no longer listed on {provider}.")) };
        if item == UNDO_ALL {
            let store = st::StoryStore::open(&cx.session.policy, &cx.session.user, cx.services.dir());
            cx.audit.record(&cx.session.user, "settings.lemonade.use_all_parts_undo", &format!("{provider}/{collection}"));
            return match Self::svc(cx).undo_all_parts(&store) {
                Ok(m) => self.ok(m),
                Err(e) => self.fail(e),
            };
        }
        if item == USE_ALL || item.starts_with(INSTALL_PARTS) {
            let missing = r.missing_lines();
            if r.needs_install {
                let gb = r.plan.missing_gb().map_or_else(|| r.collection.size_text(), |g| format!("{g:.1} GB"));
                let text = format!("{collection} is not complete on {place}. Missing: {}. Download them ({gb}) into that server? Lemonade fetches the collection and its parts; progress shows on this page. Nothing is changed until every part is installed and you press Use all parts again.", missing.join("; "));
                self.modal = Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::InstallParts(provider.to_string(), collection.to_string())), text));
                return;
            }
            if item != USE_ALL {
                return self.fail("every part is installed already".to_string());
            }
            let lacking = r.plan.lacking();
            if !lacking.is_empty() {
                return self.fail(format!("{collection} has no part for {}, so it cannot be used for all four. Nothing was changed.", lacking.join(", ")));
            }
            let text = format!("Use all parts of {collection} (Omni collection) on {place}: {}. Chat, pictures, what the microphone hears and what is read aloud then go to {}. Everything is set together or nothing is, and Undo puts the old choices back. Use it?", r.plan.parts_text(), Self::lem_where(cx, provider));
            self.modal = Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::UseAll(provider.to_string(), collection.to_string())), text));
        }
        // a line of the parts list: information only
    }

    fn lem_model(cx: &AppCx, provider: &str, model: &str) -> Option<Model> {
        Self::svc(cx).lemonade_cached(provider).into_iter().find(|m| m.id == model)
    }

    fn lem_base(cx: &AppCx, provider: &str) -> Option<String> {
        Self::providers(cx).into_iter().find(|p| p.id == provider).and_then(|p| p.base().ok())
    }

    /// A server on another machine gets text or sound only after a yes on a card that says where it is (once per server).
    fn lem_needs_yes(cx: &AppCx, provider: &str) -> Option<(String, &'static str)> {
        let base = Self::lem_base(cx, provider)?;
        (!Self::svc(cx).speech_server_allowed(&base)).then(|| (iyakki_sec::url_host(&base).unwrap_or_default(), speechtime::locality(&base).label()))
    }

    fn lem_host(cx: &AppCx, provider: &str) -> String {
        Self::providers(cx).into_iter().find(|p| p.id == provider).and_then(|p| p.base().ok()).and_then(|b| iyakki_sec::url_host(&b)).unwrap_or_default()
    }

    pub(super) fn lem_pick(&mut self, cx: &AppCx, p: LemPick, item: &str) {
        match p {
            LemPick::Model(provider) => {
                let id = item.split(" · ").nth(1).unwrap_or("");
                if let Some(m) = Self::lem_model(cx, &provider, id) {
                    self.lem_actions_for(cx, &provider, &m);
                }
            }
            LemPick::Voice(provider, model) if Self::lem_needs_yes(cx, &provider).is_some() => {
                let (host, place) = Self::lem_needs_yes(cx, &provider).unwrap_or_default();
                let text = format!("{provider} ({host}) is {place}. Reading aloud with it sends the text being read (TALES stories, chat replies) to that machine, and its sound comes back from there. Send text to {host} from now on?");
                self.modal = Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::RemoteVoice(provider, model, item.to_string())), text));
            }
            LemPick::Voice(provider, model) => match Self::svc(cx).use_lemonade_voice(&provider, &model, Some(item)) {
                Ok(m) => {
                    cx.audit.record(&cx.session.user, "settings.voice", &format!("tts lemonade {provider}/{model} {item}"));
                    self.ok(format!("{m} TALES and chat replies are read with it."));
                }
                Err(e) => self.fail(e),
            },
            LemPick::Action(provider, model) => {
                let Some(m) = Self::lem_model(cx, &provider, &model) else { return self.fail(format!("{model} is no longer listed on {provider}.")) };
                if lemonade::is_collection(&m) {
                    return self.lem_collection_pick(cx, &provider, &model, item);
                }
                let host = Self::lem_host(cx, &provider);
                let r: Result<String, String> = match item {
                    USE_CHAT => {
                        let set = Self::svc(cx).settings.lock().unwrap().update(|s| {
                            s.chat = Some(ModelChoice { provider: provider.clone(), model: model.clone() });
                            if let Some(pr) = s.providers.iter_mut().find(|x| x.id == provider) {
                                pr.default_model = Some(model.clone());
                            }
                        });
                        set.map(|_| {
                            cx.audit.record(&cx.session.user, "settings.chat_model", &format!("{provider}/{model}"));
                            format!("chat model: {provider} · {model}.")
                        })
                        .map_err(|e| e.to_string())
                    }
                    USE_EMBED => {
                        let set = Self::svc(cx).settings.lock().unwrap().set_embedding(Some(ModelChoice { provider: provider.clone(), model: model.clone() }));
                        set.map(|_| {
                            cx.audit.record(&cx.session.user, "settings.embedding_model", &format!("{provider}/{model}"));
                            let extra = if Self::svc(cx).reindex_stale_in_background() { " Existing documents are being re-embedded in the background." } else { "" };
                            format!("embedding model: {provider} · {model}.{extra}")
                        })
                        .map_err(|e| e.to_string())
                    }
                    USE_PICTURES => {
                        let store = st::StoryStore::open(&cx.session.policy, &cx.session.user, cx.services.dir());
                        let mut prefs = store.load_prefs();
                        prefs.picture_model = Some(ModelChoice { provider: provider.clone(), model: model.clone() });
                        store.save_prefs(&prefs).map(|_| {
                            cx.audit.record(&cx.session.user, "story.picture_model", &format!("{provider} {model}"));
                            format!("TALES pictures are drawn with {model} on {provider} (your choice; Lemonade loads it on the first picture, which can take a while).")
                        })
                    }
                    USE_STT if Self::lem_needs_yes(cx, &provider).is_some() => {
                        let (host, place) = Self::lem_needs_yes(cx, &provider).unwrap_or_default();
                        let text = format!("{provider} ({host}) is {place}. Speech-to-text with it sends what the microphone hears, and speech made for reading aloud (to time the lit word), to that machine. Send sound to {host} from now on?");
                        self.modal = Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::RemoteStt(provider, model)), text));
                        return;
                    }
                    USE_STT => Self::svc(cx).use_lemonade_stt(&provider, &model).map(|m| {
                        cx.audit.record(&cx.session.user, "settings.voice", &format!("stt lemonade {provider}/{model}"));
                        m
                    }),
                    USE_VOICE => {
                        let voices: Vec<String> = lemonade::voices_for(&model).into_iter().map(str::to_string).collect();
                        self.modal = Some(Modal::Pick(PickKind::Lemonade(LemPick::Voice(provider, model.clone())), Picker::new(&format!("Voice of {model}"), voices)));
                        return;
                    }
                    LOAD => {
                        let text = format!("Load {model} ({}) into memory on the Lemonade server {provider} ({host})? It uses that machine's memory until it is unloaded. Lemonade also loads a model by itself on its first use.", m.size_text());
                        self.modal = Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::Load(provider, model)), text));
                        return;
                    }
                    UNLOAD => {
                        let svc = Self::svc(cx).clone();
                        cx.audit.record(&cx.session.user, "settings.lemonade.unload", &format!("{provider}/{model}"));
                        self.start(&format!("unloading {model}"), move || JobOut::Msg(svc.lemonade_load(&provider, &model, false)));
                        return;
                    }
                    pull if pull.starts_with("Pull") => {
                        let text = format!("Download {model} ({}) into the Lemonade server {provider} ({host})? Lemonade fetches it from its model source and keeps it on that machine. Progress shows on this page.", m.size_text());
                        self.modal = Some(Modal::Confirm(ConfirmKind::Lemonade(LemAct::Pull(provider, model)), text));
                        return;
                    }
                    _ => return,
                };
                match r {
                    Ok(m) => self.ok(m),
                    Err(e) => self.fail(e),
                }
            }
        }
    }

    pub(super) fn lem_confirm(&mut self, cx: &AppCx, act: LemAct) {
        if !self.admin(cx) {
            return;
        }
        match act {
            LemAct::Pull(provider, model) => {
                cx.audit.record(&cx.session.user, "settings.lemonade.pull", &format!("{provider}/{model}"));
                match Self::svc(cx).pull_model(&provider, &model) {
                    Ok(()) => self.ok(format!("Downloading {model} into {provider}; the progress shows below.")),
                    Err(e) => self.fail(e),
                }
            }
            LemAct::Load(provider, model) => {
                cx.audit.record(&cx.session.user, "settings.lemonade.load", &format!("{provider}/{model}"));
                let svc = Self::svc(cx).clone();
                self.start(&format!("loading {model} (a large model takes a while)"), move || JobOut::Msg(svc.lemonade_load(&provider, &model, true)));
            }
            LemAct::InstallParts(provider, collection) => {
                cx.audit.record(&cx.session.user, "settings.lemonade.pull", &format!("{provider}/{collection}"));
                match Self::svc(cx).install_missing_parts(&provider, &collection) {
                    Ok(ids) => self.ok(format!("Downloading {} into {provider}; the progress shows below. Press Use all parts again when it is done.", ids.join(", "))),
                    Err(e) => self.fail(e),
                }
            }
            LemAct::UseAll(provider, collection) => {
                let Some(base) = Self::lem_base(cx, &provider) else { return };
                // the card said where this runs: its yes also allows speech to go to a server on another machine
                let r = if Self::svc(cx).speech_server_allowed(&base) {
                    Ok(())
                } else {
                    cx.audit.record(&cx.session.user, "settings.voice.remote_ok", &speechtime::server_of(&base));
                    Self::svc(cx).allow_speech_server(&base)
                };
                let store = st::StoryStore::open(&cx.session.policy, &cx.session.user, cx.services.dir());
                match r.and_then(|_| Self::svc(cx).use_all_parts(&provider, &collection, &store)) {
                    Ok(m) => {
                        cx.audit.record(&cx.session.user, "settings.lemonade.use_all_parts", &format!("{provider}/{collection}"));
                        self.ok(m)
                    }
                    Err(e) => self.fail(e),
                }
            }
            LemAct::RemoteVoice(..) | LemAct::RemoteStt(..) => {
                let (provider, model, voice) = match act {
                    LemAct::RemoteVoice(p, m, v) => (p, m, Some(v)),
                    LemAct::RemoteStt(p, m) => (p, m, None),
                    _ => return,
                };
                let Some(base) = Self::lem_base(cx, &provider) else { return };
                cx.audit.record(&cx.session.user, "settings.voice.remote_ok", &speechtime::server_of(&base));
                let r = Self::svc(cx).allow_speech_server(&base).and_then(|_| match voice.as_deref() {
                    Some(v) => Self::svc(cx).use_lemonade_voice(&provider, &model, Some(v)).map(|m| format!("{m} TALES and chat replies are read with it.")),
                    None => Self::svc(cx).use_lemonade_stt(&provider, &model),
                });
                match r {
                    Ok(m) => self.ok(m),
                    Err(e) => self.fail(e),
                }
            }
        }
    }

    /// The chip click of model `i` of the cached list. Index 0..255 travels as `Sub(i, 'L')`; a bigger catalog adds the
    /// high byte in a private-use character, so no model is ever out of reach.
    pub(super) fn lem_chip_event(i: usize) -> Event {
        if i < 256 {
            Event::Sub(i as u8, Key::Char('L'))
        } else {
            Event::Sub((i & 255) as u8, Key::Char(char::from_u32(0xE000 + (i >> 8) as u32).unwrap_or('L')))
        }
    }

    /// The model index a chip event carries (the inverse of [`Self::lem_chip_event`]).
    pub(super) fn lem_chip_index(lo: u8, c: char) -> Option<usize> {
        match c {
            'L' => Some(lo as usize),
            '\u{E000}'..='\u{E0FF}' => Some(((c as usize - 0xE000) << 8) | lo as usize),
            _ => None,
        }
    }

    /// Does the model answer to the search text (lowercase)? Every word must be in its id, type (word or label), checkpoint or labels.
    pub(super) fn lem_matches(m: &Model, q: &str) -> bool {
        let hay = format!("{} {} {} {} {}", m.id, m.kind.key(), m.kind.label(), m.checkpoint, m.labels.join(" ")).to_lowercase();
        q.split_whitespace().all(|w| hay.contains(w))
    }

    /// A key typed while the model search box has the keyboard.
    pub(super) fn lem_search_key(&mut self, k: &Key) {
        match k {
            Key::Esc => {
                if self.lem_query.value.is_empty() {
                    self.lem_search = false;
                } else {
                    self.lem_query.set("");
                }
            }
            Key::Enter | Key::Tab | Key::Down => self.lem_search = false,
            other => {
                self.lem_query.handle(other);
            }
        }
        self.lem_scroll = 0.0;
    }

    /// Scroll the model list by `d` pixels (positive = down), clamped to its length.
    pub(super) fn lem_scroll_by(&mut self, d: f32) {
        self.lem_scroll = (self.lem_scroll + d).clamp_safe(0.0, self.lem_max);
    }

    /// A key for the model list: PageUp/PageDown a screen, Home/End the ends, Up/Down a line once the list has the focus.
    /// True when the key was used.
    pub(super) fn lem_key(&mut self, k: &Key) -> bool {
        if self.lem_max <= 0.0 {
            self.lem_focus = false;
            return false;
        }
        let page = (self.lem_view_h * 0.9).max(28.0);
        match k {
            Key::PageDown => self.lem_scroll_by(page),
            Key::PageUp => self.lem_scroll_by(-page),
            Key::Home if self.lem_focus => self.lem_scroll = 0.0,
            Key::End if self.lem_focus => self.lem_scroll = self.lem_max,
            Key::Down if self.lem_focus && self.lem_scroll < self.lem_max => self.lem_scroll_by(28.0 * self.lem_s),
            Key::Up if self.lem_focus && self.lem_scroll > 0.0 => self.lem_scroll_by(-28.0 * self.lem_s),
            Key::Up | Key::Down => {
                // at the end of the list the arrows go back to choosing a provider
                self.lem_focus = false;
                return false;
            }
            _ => return false,
        }
        self.lem_focus = !matches!(k, Key::Home);
        true
    }

    /// The models of a Lemonade provider: Installed first (every downloaded or loaded model of any type, its type on the chip),
    /// then by type only what is not downloaded yet (a "pull" chip), then Video (none) and downloads. The part below the heading
    /// scrolls (wheel, PageUp/PageDown, arrows) so every section can be reached.
    pub(super) fn render_lemonade(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx, p: &ProviderCfg) {
        let list = Self::svc(cx).lemonade_cached(&p.id);
        let line_h = 28.0 * s;
        self.lem_rect = a;
        self.lem_s = s;
        if a.h < line_h {
            self.lem_max = 0.0;
            return;
        }
        let host = p.base().ok().and_then(|b| iyakki_sec::url_host(&b)).unwrap_or_default();
        let place = p.base().map(|b| speechtime::locality(&b).label()).unwrap_or("");
        let q = self.lem_query.value.trim().to_lowercase();
        let shown = |m: &Model| q.is_empty() || Self::lem_matches(m, &q);
        let n_installed = list.iter().filter(|m| m.downloaded || m.loaded).count();
        let n_shown = list.iter().filter(|m| shown(m)).count();
        let sbox_w = (a.w * 0.3).clamp_safe(150.0 * s, 280.0 * s).min(a.w * 0.5);
        let head = if list.is_empty() {
            format!("Lemonade on {host} ({place}): models not read yet; press Test (t) or All models (l)")
        } else {
            let counts = if q.is_empty() { format!("{n_installed} installed · {} available", list.len() - n_installed) } else { format!("{n_shown} of {} shown · {n_installed} installed", list.len()) };
            let hint = if self.lem_max > 0.0 { " · wheel or PageUp/PageDown scrolls" } else { "" };
            format!("{counts} · Lemonade on {host} ({place}) · click a model to use it{hint}")
        };
        self.lem_texts = vec![head.clone()];
        let head = g.fit(14.0 * s, &head, Face::Bold, a.w - sbox_w - 12.0 * s);
        g.text(a.x + 4.0 * s, a.y, 14.0 * s, TEXT, &head, Face::Bold);
        if !list.is_empty() {
            let blink_on = super::blink(self.ticks);
            self.lem_query.render(g, R::new(a.x + a.w - sbox_w, a.y - 4.0 * s, sbox_w, line_h - 2.0 * s), self.lem_search, s, blink_on, Face::Sans, LEM_SEARCH_ID);
        }
        let view = R::new(a.x, a.y + line_h, a.w, (a.h - line_h).max(0.0));
        self.lem_view_h = view.h;
        let label_w = (130.0 * s).min(a.w * 0.35);
        // Installed first, then the types with only what still has to be pulled
        struct Sec<'a> {
            label: String,
            models: Vec<(usize, &'a Model)>,
            installed: bool,
            note: Option<&'static str>,
        }
        let mut sections: Vec<Sec> = vec![Sec { label: "Installed".into(), models: list.iter().enumerate().filter(|(_, m)| (m.downloaded || m.loaded) && shown(m)).collect(), installed: true, note: None }];
        let no_chat = !list.is_empty() && q.is_empty() && !list.iter().any(|m| (m.downloaded || m.loaded) && m.kind.chats());
        for t in ModelType::SECTIONS {
            let todo: Vec<(usize, &Model)> = list.iter().enumerate().filter(|(_, m)| m.kind == t && !m.downloaded && !m.loaded && shown(m)).collect();
            let note = (t == ModelType::Chat && no_chat).then_some("no chat model installed: pick one below to pull");
            if !todo.is_empty() || note.is_some() {
                sections.push(Sec { label: format!("{} (pull)", t.label()), models: todo, installed: false, note });
            }
        }
        if q.is_empty() {
            sections.push(Sec { label: "Video".into(), models: vec![], installed: false, note: None });
        }
        let nothing = !list.is_empty() && !q.is_empty() && n_shown == 0;
        if nothing {
            self.lem_texts.push(format!("no model matches “{}”", self.lem_query.value.trim()));
        }
        self.lem_texts.extend(sections.iter().filter_map(|x| x.note).map(str::to_string));
        let top = view.y - self.lem_scroll;
        let pulls: Vec<_> = Self::svc(cx).pull_status().into_iter().filter(|x| x.provider == p.id).collect();
        let end = g.clipped(view, |g| {
            let mut y = top;
            let mut pulls_done = false;
            let draw_pulls = |g: &mut Gfx, y: &mut f32| {
                for st in &pulls {
                    let (t, col) = match (&st.error, st.done) {
                        (Some(e), _) => (format!("{}: {e}", st.model), ERR),
                        (None, true) => (format!("{}: downloaded", st.model), OK),
                        (None, false) if st.total > 0 => (format!("{}: {} {}%", st.model, st.status, st.completed * 100 / st.total), TEXT),
                        (None, false) => (format!("{}: {}", st.model, st.status), TEXT),
                    };
                    let t = g.fit(13.0 * s, &t, Face::Sans, a.w - 8.0 * s);
                    g.text(a.x + 4.0 * s, *y + 4.0 * s, 13.0 * s, col, &t, Face::Sans);
                    *y += 20.0 * s;
                }
            };
            if nothing {
                let t = g.fit(13.0 * s, &format!("no model matches “{}”", self.lem_query.value.trim()), Face::Sans, a.w - 8.0 * s);
                g.text(a.x + 4.0 * s, y + 4.0 * s, 13.0 * s, DIM, &t, Face::Sans);
                return line_h;
            }
            for (n, sec) in sections.iter().enumerate() {
                let (label, models, is_installed) = (&sec.label, &sec.models, &sec.installed);
                g.text(a.x + 4.0 * s, y + 4.0 * s, 13.0 * s, if *is_installed { TEXT } else { MUTED }, label, Face::Sans);
                let mut x = a.x + label_w;
                if let Some(note) = sec.note {
                    g.text(x, y + 4.0 * s, 13.0 * s, ERR, note, Face::Sans);
                    y += line_h;
                }
                if models.is_empty() && sec.note.is_some() {
                    // the note is the whole section
                } else if models.is_empty() {
                    let none = if label == "Video" { lemonade::NO_VIDEO } else if list.is_empty() { "nothing read yet" } else { "none installed yet" };
                    let t = g.fit(13.0 * s, none, Face::Sans, a.x + a.w - x - 8.0 * s);
                    g.text(x, y + 4.0 * s, 13.0 * s, DIM, &t, Face::Sans);
                    y += line_h;
                } else {
                    for (i, m) in models {
                        let text = if *is_installed {
                            format!("{} · {}{}", m.id, if lemonade::is_collection(m) { "omni collection" } else { m.kind.key() }, if m.loaded { " · loaded" } else { "" })
                        } else {
                            format!("{} · pull {}", m.id, m.size_text())
                        };
                        let cw = g.text_width(12.0 * s, &text, Face::Bold) + 12.0 * s * 1.4;
                        if x + cw > a.x + a.w && x > a.x + label_w {
                            x = a.x + label_w;
                            y += line_h;
                        }
                        // only what is (partly) in view is drawn and clickable
                        if y + line_h >= view.y && y <= view.y + view.h {
                            let text = g.fit(12.0 * s, &text, Face::Bold, a.x + a.w - x - 12.0 * s * 1.4 - 8.0 * s);
                            let w = crate::fx::chip(g, x, y, s, &text, status_color(m), m.loaded);
                            g.hit(R::new(x, y, w, 12.0 * s * 1.9), Self::lem_chip_event(*i), &m.id);
                            x += w + 6.0 * s;
                        } else {
                            x += cw + 6.0 * s;
                        }
                    }
                    y += line_h;
                }
                // downloads in progress sit right under Installed
                if n == 0 && !pulls_done {
                    pulls_done = true;
                    draw_pulls(g, &mut y);
                }
            }
            y - top
        });
        self.lem_max = (end + 6.0 * s - view.h).max(0.0);
        self.lem_scroll = self.lem_scroll.clamp_safe(0.0, self.lem_max);
        if self.lem_max > 0.0 {
            // the scroll indicator: a thin track on the right with a thumb
            let (tx, tw) = (a.x + a.w - 5.0 * s, 4.0 * s);
            g.rrect(tx, view.y, tw, view.h, 2.0 * s, rgb(40, 48, 74));
            let total = view.h + self.lem_max;
            let th = (view.h * view.h / total).max(18.0 * s).min(view.h);
            let ty = view.y + (view.h - th) * (self.lem_scroll / self.lem_max);
            g.rrect(tx, ty, tw, th, 2.0 * s, MUTED);
        }
    }
}

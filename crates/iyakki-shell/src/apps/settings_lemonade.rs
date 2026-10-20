//! Settings → Models for AMD's Lemonade Server: Detect (Ollama and Lemonade on this machine), the Auto-detect server type of
//! the provider form, and under a chosen Lemonade provider its models by type (Chat, Vision, Embeddings, Reranking, Image,
//! Speech-to-text, Text-to-speech, and Video: none) with their state (loaded, available, not downloaded). A click on a model
//! offers what fits its type: use it as the chat model, for embeddings, as the TALES picture model, as the reading voice or
//! for speech-to-text, and Load / Pull, which ask first on a card naming the model and its size.

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
}

const USE_CHAT: &str = "Use as chat model";
const USE_EMBED: &str = "Use for embeddings (RAG)";
const USE_PICTURES: &str = "Use as TALES picture model";
const USE_VOICE: &str = "Use as reading voice…";
const USE_STT: &str = "Use as speech-to-text";
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

    /// The models of a Lemonade provider by type, each a chip that opens its actions, then Video (none) and downloads.
    pub(super) fn render_lemonade(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx, p: &ProviderCfg) {
        let list = Self::svc(cx).lemonade_cached(&p.id);
        let bottom = a.y + a.h;
        let line_h = 28.0 * s;
        if a.h < line_h {
            return;
        }
        let host = p.base().ok().and_then(|b| iyakki_sec::url_host(&b)).unwrap_or_default();
        let place = p.base().map(|b| speechtime::locality(&b).label()).unwrap_or("");
        let head = if list.is_empty() { format!("Lemonade on {host} ({place}): press Test (t) or All models (l) to read what it has") } else { format!("Lemonade on {host} ({place}) · {} models · click one to use it", list.len()) };
        let head = g.fit(14.0 * s, &head, Face::Bold, a.w);
        g.text(a.x + 4.0 * s, a.y, 14.0 * s, TEXT, &head, Face::Bold);
        let mut y = a.y + line_h;
        let label_w = (130.0 * s).min(a.w * 0.35);
        let mut sections: Vec<(String, Vec<(usize, &Model)>)> = ModelType::SECTIONS.iter().filter(|t| **t != ModelType::Other || list.iter().any(|m| m.kind == **t)).map(|t| (t.label().to_string(), list.iter().enumerate().filter(|(_, m)| m.kind == *t).collect())).collect();
        sections.push(("Video".into(), vec![]));
        for (label, models) in &sections {
            if y + line_h > bottom {
                break;
            }
            g.text(a.x + 4.0 * s, y + 4.0 * s, 13.0 * s, MUTED, label, Face::Sans);
            let mut x = a.x + label_w;
            if models.is_empty() {
                let none = if label == "Video" { lemonade::NO_VIDEO } else { "none" };
                let t = g.fit(13.0 * s, none, Face::Sans, a.x + a.w - x);
                g.text(x, y + 4.0 * s, 13.0 * s, DIM, &t, Face::Sans);
                y += line_h;
                continue;
            }
            for (i, m) in models {
                let text = if m.loaded { format!("{} · loaded", m.id) } else if m.downloaded { m.id.clone() } else { format!("{} · pull {}", m.id, m.size_text()) };
                let cw = g.text_width(12.0 * s, &text, Face::Bold) + 12.0 * s * 1.4;
                if x + cw > a.x + a.w && x > a.x + label_w {
                    x = a.x + label_w;
                    y += line_h;
                }
                if y + line_h > bottom {
                    break;
                }
                let text = g.fit(12.0 * s, &text, Face::Bold, a.x + a.w - x - 12.0 * s * 1.4);
                let w = crate::fx::chip(g, x, y, s, &text, status_color(m), m.loaded);
                if *i < 256 {
                    g.hit(R::new(x, y, w, 12.0 * s * 1.9), Event::Sub(*i as u8, Key::Char('L')), &m.id);
                }
                x += w + 6.0 * s;
            }
            y += line_h;
        }
        // downloads into this server
        for st in Self::svc(cx).pull_status().into_iter().filter(|x| x.provider == p.id) {
            if y + 18.0 * s > bottom {
                break;
            }
            let (t, col) = match (&st.error, st.done) {
                (Some(e), _) => (format!("{}: {e}", st.model), ERR),
                (None, true) => (format!("{}: downloaded", st.model), OK),
                (None, false) if st.total > 0 => (format!("{}: {} {}%", st.model, st.status, st.completed * 100 / st.total), TEXT),
                (None, false) => (format!("{}: {}", st.model, st.status), TEXT),
            };
            let t = g.fit(13.0 * s, &t, Face::Sans, a.w);
            g.text(a.x + 4.0 * s, y, 13.0 * s, col, &t, Face::Sans);
            y += 20.0 * s;
        }
    }
}

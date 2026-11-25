//! Settings -> Models: the providers (Ollama, OpenAI, Claude, Gemini, NVIDIA NIM, Lemonade, ...), the chat, embedding, vision and tidy models,
//! the token price and tool routing. The page reaches the settings, the vault, the services and the agent only through `ModelsPort`
//! (docs/development/SETTINGS-EXTRACTION-PLAN.md, lane ST-B).

use super::*;
use super::apps_port::{store_secret, ModelsPort};

impl SettingsApp {
    pub(super) fn test_provider(&mut self, p: &dyn ModelsPort, id: &str) {
        let ai = p.ai();
        let plain = p.plain_error();
        let id = id.to_string();
        match ai.provider_check(&id) {
            Err(e) => self.fail(e),
            Ok(()) => self.start(&format!("testing {id}"), move || match ai.provider_test(&id) {
                // a Lemonade server's typed list is kept too (pictures, voices, embeddings): the facet refreshed it
                Ok(r) => JobOut::Models { provider: id, models: r.models, ms: r.latency_ms },
                Err(e) => JobOut::Msg(Err(format!("{id}: {}", plain(&id, &e)))),
            }),
        }
    }

    pub(super) fn test_chat(&mut self, p: &dyn ModelsPort) {
        let ai = p.ai();
        let Some(choice) = p.settings_get().chat.clone() else {
            self.fail("Choose a chat model first (press m on a provider).");
            return;
        };
        let plain = p.plain_error();
        self.start("asking the model", move || {
            let r = ai.provider_chat(&choice.provider, &choice.model, &[iyakki_shared::chat::Message::user("Reply with the single word: OK")]);
            JobOut::Msg(match r {
                Ok(t) => Ok(format!("{} · {} replied: {}", choice.provider, choice.model, t.chars().take(80).collect::<String>())),
                Err(e) => Err(format!("{}: {}", choice.provider, plain(&choice.provider, &e))),
            })
        });
    }

    pub(super) fn provider_form(p: &dyn ModelsPort, editing: Option<&ProviderCfg>) -> Form {
        let _ = p;
        let kind = editing.map_or(0, |pv| Kind::ALL.iter().position(|k| *k == pv.kind).unwrap_or(0));
        let default_url = editing.and_then(|pv| pv.base_url.clone()).or_else(|| Kind::ALL[kind].default_base().map(str::to_string)).unwrap_or_default();
        Form::new(if editing.is_some() { "Edit model provider" } else { "Add model provider" })
            .with_note("Keys are stored in the system vault. For NVIDIA NIM (free) press Esc, then N: it shows how to get the key.")
            .text("Name (lowercase id)", editing.map_or("", |pv| pv.id.as_str()), "e.g. local-ollama, openai, claude")
            .choice("Type", &kind_labels(), kind)
            .text("Base URL", &default_url, "http://localhost:11434")
            .secret("API key", if editing.is_some() { "leave empty to keep the current key" } else { "stored in the system vault, never in settings" })
            .text("Default model (optional)", editing.and_then(|pv| pv.default_model.as_deref()).unwrap_or(""), "pick one later with m")
    }

    /// The guided NVIDIA NIM setup: check the key looks like one, store it in the vault, add (or update) the provider `nvidia` and test it by
    /// listing the models; the person then chooses a chat model with m.
    pub(super) fn save_nvidia(&mut self, p: &dyn ModelsPort, key: &str) -> Result<String, String> {
        use iyakki_shared::providers::NVIDIA_KEY_PREFIX;
        if key.is_empty() {
            return Err("Paste the key first. Steps: build.nvidia.com, sign in, open a model, Get API Key.".into());
        }
        if !key.starts_with(NVIDIA_KEY_PREFIX) || key.len() < 20 || key.chars().any(char::is_whitespace) {
            return Err(format!("An NVIDIA key starts with {NVIDIA_KEY_PREFIX} and has no spaces. Copy the whole key again from build.nvidia.com."));
        }
        let mut cfg = p.settings_get().providers.into_iter().find(|pv| pv.id == "nvidia").unwrap_or_else(|| ProviderCfg::new("nvidia", Kind::Nvidia));
        cfg.kind = Kind::Nvidia;
        cfg.enabled = true;
        cfg.key_ref = Some("vault:nvidia-key".into());
        cfg.validate()?; // before touching the vault
        store_secret(p, "nvidia-key", key, true)?;
        p.settings_update(&mut |s| s.put_provider(cfg.clone())).map_err(|e| e.message())?;
        p.audit("settings.provider.save", "nvidia (NVIDIA NIM)");
        self.test_provider(p, "nvidia");
        Ok("NVIDIA NIM added. Testing the key and listing the models; then press m to choose a chat model.".into())
    }

    pub(super) fn save_provider(&mut self, p: &dyn ModelsPort, form: &Form, editing: Option<String>) -> Result<(), String> {
        let id = form.value(0);
        let Some(&kind) = Kind::ALL.get(form.choice_idx(1)) else { return self.save_provider_auto(p, form, editing) };
        let url = form.value(2);
        let key = form.raw(3);
        let model = form.value(4);
        let existing = p.settings_get().providers.into_iter().find(|pv| Some(&pv.id) == editing.as_ref());
        if editing.is_none() && p.settings_get().providers.iter().any(|pv| pv.id == id) {
            return Err(format!("A provider named `{id}` already exists."));
        }
        let mut cfg = existing.clone().unwrap_or_else(|| ProviderCfg::new(&id, kind));
        if editing.is_none() {
            cfg.id = id.clone();
        }
        cfg.kind = kind;
        cfg.base_url = if url.is_empty() { None } else { Some(url) };
        cfg.default_model = if model.is_empty() { None } else { Some(model) };
        let key_name = format!("{}-key", cfg.id);
        if !key.is_empty() {
            cfg.key_ref = Some(format!("vault:{key_name}"));
        } else if existing.is_none() && !kind.needs_key() {
            cfg.key_ref = None;
        }
        cfg.validate()?; // before touching the vault: a rejected form stores nothing
        if !key.is_empty() {
            store_secret(p, &key_name, &key, true)?;
        }
        p.settings_update(&mut |s| s.put_provider(cfg.clone())).map_err(|e| e.message())?;
        p.audit("settings.provider.save", &format!("{} ({})", cfg.id, cfg.kind.label()));
        Ok(())
    }

    pub(super) fn external_form(editing: Option<&ExtEntry>) -> Form {
        let (kind, id, url, coll, cred) = match editing.map(|e| &e.backend) {
            Some(ExternalRag::Qdrant { id, url, collection, key_ref }) => (0, id.as_str(), url.as_str(), collection.as_str(), key_ref.is_some()),
            Some(ExternalRag::Chroma { id, url, collection, token_ref, .. }) => (1, id.as_str(), url.as_str(), collection.as_str(), token_ref.is_some()),
            Some(ExternalRag::Http { id, url, key_ref }) => (2, id.as_str(), url.as_str(), "", key_ref.is_some()),
            None => (0, "", "", "", false),
        };
        Form::new(if editing.is_some() { "Edit external RAG" } else { "Add external RAG" })
            .text("Name (lowercase id)", id, "e.g. team-qdrant")
            .choice("Type", &["Qdrant", "Chroma", "HTTP search endpoint"], kind)
            .text("URL", url, "http://host:6333  /  http://host:8000  /  https://search.corp/api")
            .text("Collection (Qdrant / Chroma)", coll, "collection name")
            .secret("API key / token (optional)", if cred { "leave empty to keep the current credential" } else { "stored in the system vault" })
    }

    pub(super) fn save_external(&mut self, p: &dyn ModelsPort, form: &Form, editing: Option<String>) -> Result<(), String> {
        let (id, url, coll, key) = (form.value(0), form.value(2), form.value(3), form.raw(4));
        let prev = p.settings_get().external_rag.into_iter().find(|e| Some(e.backend.id()) == editing.as_deref());
        if editing.is_none() && p.settings_get().external_rag.iter().any(|e| e.backend.id() == id) {
            return Err(format!("A backend named `{id}` already exists."));
        }
        let key_name = format!("{id}-key");
        let prev_ref = prev.as_ref().and_then(|pv| match &pv.backend {
            ExternalRag::Qdrant { key_ref, .. } | ExternalRag::Http { key_ref, .. } => key_ref.clone(),
            ExternalRag::Chroma { token_ref, .. } => token_ref.clone(),
        });
        let cred = if !key.is_empty() { Some(format!("vault:{key_name}")) } else { prev_ref };
        let backend = match form.choice_idx(1) {
            0 => ExternalRag::Qdrant { id: id.clone(), url, collection: coll, key_ref: cred },
            1 => ExternalRag::Chroma { id: id.clone(), url, tenant: "default_tenant".into(), database: "default_database".into(), collection: coll, token_ref: cred },
            _ => ExternalRag::Http { id: id.clone(), url, key_ref: cred },
        };
        backend.validate()?;
        if !key.is_empty() {
            store_secret(p, &key_name, &key, true)?;
        }
        let entry = ExtEntry { enabled: prev.map_or(true, |pv| pv.enabled), backend };
        p.settings_update(&mut |s| s.put_external(entry.clone())).map_err(|e| e.message())?;
        p.audit("settings.external.save", &id);
        Ok(())
    }

    pub(super) fn handle_models(&mut self, k: &Key, p: &dyn ModelsPort) {
        let provs = p.settings_get().providers;
        let cur = provs.get(self.sel()).cloned();
        match k {
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End if cur.as_ref().is_some_and(|pv| pv.kind == Kind::Lemonade) && self.lem_key(k) => {}
            Key::Up => self.move_sel(-1, provs.len()),
            Key::Down => self.move_sel(1, provs.len()),
            Key::Char('a') if self.admin(p) => {
                let f = Self::provider_form(p, None);
                self.open_form(FormKind::Provider(None), f);
            }
            Key::Char('N') if self.admin(p) => {
                let f = Form::new("Add NVIDIA NIM (free)").with_note(iyakki_shared::providers::NVIDIA_STEPS).secret("NVIDIA API key", "paste the key that starts with nvapi-");
                self.open_form(FormKind::NvidiaQuick, f);
            }
            Key::Char('v') if self.admin(p) => {
                let mut items = vec!["(automatic: the chat model if it can see pictures, else one picked)".to_string()];
                items.extend(p.lemonade().vision_candidates().iter().map(|c| c.label()));
                if items.len() == 1 {
                    return self.fail("No provider lists a vision model yet. Install one (for example a Qwen vision model in Lemonade), then press t on the provider to refresh its list.");
                }
                self.modal = Some(Modal::Pick(PickKind::Vision, Picker::new("Vision model", items)));
            }
            Key::Char('y') if self.admin(p) => {
                let mut items = vec!["(the chat model: nothing separate for tidying)".to_string()];
                items.extend(Self::tidy_candidates(p));
                if items.len() == 1 {
                    return self.fail("No provider lists a chat model yet. Add a provider and press t on it to refresh its list.");
                }
                self.modal = Some(Modal::Pick(PickKind::Tidy, Picker::new("Tidy model", items)));
            }
            Key::Char('p') if self.admin(p) => {
                let cur_price = p.settings_get().token_price_cents_per_mtok;
                let f = Form::new("Agent token price").text("US cents per million tokens", &cur_price.to_string(), "0 = free or untracked; used for agent cost budgets");
                self.open_form(FormKind::TokenPrice, f);
            }
            Key::Char('T') if self.admin(p) => {
                let r = p.settings_update(&mut |s| s.no_tool_routing = !s.no_tool_routing);
                let off = p.settings_get().no_tool_routing;
                match r {
                    Ok(()) => {
                        p.audit("settings.tool_routing", if off { "off" } else { "on" });
                        self.ok(format!("Tool routing {}.", if off { "off: the model is offered every tool (more tokens per request)" } else { "on: the model is offered the tools the chat needs" }))
                    }
                    Err(e) => self.fail(e.message()),
                }
            }
            Key::Char('r') => {
                // each person's own switch (off by default: no override); the machine-wide one stays the administrator's (T)
                let dir = p.data_dir();
                let off = !p.agent().toolrouting_person_off(&dir, &p.user());
                match p.agent().toolrouting_set_person_off(&dir, &p.user(), off) {
                    Ok(()) => {
                        p.audit("settings.tool_routing_own", if off { "off" } else { "on" });
                        let machine_off = p.settings_get().no_tool_routing;
                        self.ok(p.agent().toolrouting_summary(&dir, &p.user(), machine_off));
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('f') if self.admin(p) => self.detect_servers(p),
            // the chosen provider's models write PDF files themselves: the assistant's pdf_create is not offered (docs/PDF.md)
            Key::Char('P') if self.admin(p) && cur.is_some() => {
                let id = cur.as_ref().map(|pv| pv.id.clone()).unwrap_or_default();
                let to = !cur.as_ref().is_some_and(|pv| pv.native_pdf_output);
                let r = p.settings_update(&mut |s| {
                    if let Some(pv) = s.providers.iter_mut().find(|pv| pv.id == id) {
                        pv.native_pdf_output = to;
                    }
                });
                match r {
                    Ok(()) => {
                        p.audit("settings.native_pdf_output", &format!("{id} {}", if to { "on" } else { "off" }));
                        self.ok(if to { format!("{id} is marked as writing PDF files itself: the assistant's pdf_create is not offered with it.") } else { format!("{id} is offered the assistant's pdf_create again.") });
                    }
                    Err(e) => self.fail(e.message()),
                }
            }
            Key::Char(c @ ('L' | 'E')) if self.admin(p) && cur.as_ref().is_some_and(|pv| pv.kind != Kind::Lemonade) => {
                let pv = cur.expect("checked");
                self.mo_list(p, &pv, *c == 'E');
            }
            Key::Char('o') if cur.as_ref().is_some_and(|pv| pv.kind != Kind::Lemonade) => {
                let pv = cur.expect("checked");
                self.mo_options(p, &pv);
            }
            Key::Char('K') if self.admin(p) && cur.as_ref().is_some_and(|pv| pv.kind == Kind::Nvidia) => {
                let pv = cur.expect("checked");
                self.modal = Some(Modal::Confirm(ConfirmKind::CheckModels(pv.id.clone()), format!("Check my models on {}? NVIDIA lists models your key cannot use. This sends one tiny request to each chat model (up to {}, about {} seconds), which uses part of your rate limit, and stops if NVIDIA says you are sending too many. Models your key cannot use are then hidden from the model list for 24 hours.", pv.id, iyakki_shared::modelavail::CHECK_CAP, iyakki_shared::modelavail::CHECK_CAP as u64 * iyakki_shared::modelavail::CHECK_GAP_MS / 1000)));
            }
            Key::Char('g') if self.admin(p) => self.gem_open_setup(p),
            Key::Char('C') => self.gem_consent_key(p),
            Key::Char('t') if cur.as_ref().is_some_and(|pv| pv.id == iyakki_shared::gemini::PROVIDER_ID) => self.gem_test(p),
            Key::Char('H') if self.admin(p) => self.mh_open(p, true),
            Key::Char('G') if self.admin(p) && cur.as_ref().is_some_and(|pv| pv.kind == Kind::Lemonade) => {
                let be = p.lemonade().lemonade_compute_backend(&p.user());
                let next = if be.current() == iyakki_shared::gpu_steps::Compute::Gpu { iyakki_shared::gpu_steps::Compute::Cpu } else { iyakki_shared::gpu_steps::Compute::Gpu };
                p.audit("settings.lemonade.compute", next.word());
                self.start(if next == iyakki_shared::gpu_steps::Compute::Gpu { "moving Lemonade to the GPU" } else { "moving Lemonade to the CPU" }, move || JobOut::Msg(be.set(next)));
            }
            Key::Char('l') if cur.as_ref().is_some_and(|pv| pv.kind == Kind::Lemonade) => {
                let id = cur.as_ref().expect("checked").id.clone();
                self.lem_open(p, &id);
            }
            Key::Char(c) if cur.is_some() && "edkmbxtc".contains(*c) => {
                let pv = cur.expect("checked");
                match *c {
                    't' => self.test_provider(p, &pv.id),
                    'c' => self.test_chat(p),
                    _ if !self.admin(p) => {}
                    'e' => {
                        let f = Self::provider_form(p, Some(&pv));
                        self.open_form(FormKind::Provider(Some(pv.id.clone())), f);
                    }
                    'd' => self.modal = Some(Modal::Confirm(ConfirmKind::Provider(pv.id.clone()), format!("Remove provider “{}”?", pv.id))),
                    'k' => {
                        let f = Form::new(&format!("API key for {}", pv.id)).secret("API key", "stored in the system vault");
                        self.open_form(FormKind::ProviderKey(pv.id.clone()), f);
                    }
                    'x' => {
                        let id = pv.id.clone();
                        let r = p.settings_update(&mut |s| {
                            if let Some(x) = s.providers.iter_mut().find(|x| x.id == id) {
                                x.enabled = !x.enabled;
                            }
                        });
                        match r {
                            Ok(()) => self.ok(format!("{} {}.", pv.id, if pv.enabled { "disabled" } else { "enabled" })),
                            Err(e) => self.fail(e.message()),
                        }
                    }
                    'b' if pv.kind == Kind::Lemonade && !pv.model_kinds.is_empty() => {
                        // a Lemonade server says which of its models make embeddings
                        let mut items: Vec<String> = pv.model_kinds.iter().filter(|(_, k)| k.as_str() == "embeddings").map(|(m, _)| m.clone()).collect();
                        items.insert(0, "(built-in hashing embeddings)".into());
                        self.modal = Some(Modal::Pick(PickKind::Embed(pv.id.clone()), Picker::new(&format!("Embedding model on {}", pv.id), items)));
                    }
                    'm' | 'b' => {
                        if pv.models.is_empty() {
                            self.fail("No model list yet: press t to test the connection and fetch it.");
                        } else {
                            let mut items = pv.models.clone();
                            if *c == 'b' {
                                items.insert(0, "(built-in hashing embeddings)".into());
                            }
                            let title = if *c == 'm' { format!("Chat model on {}", pv.id) } else { format!("Embedding model on {}", pv.id) };
                            let kind = if *c == 'm' { PickKind::Chat(pv.id.clone()) } else { PickKind::Embed(pv.id.clone()) };
                            self.modal = Some(Modal::Pick(kind, Picker::new(&title, items)));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_models(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn ModelsPort) {
        let (chat, emb, provs, tidy) = {
            let st = p.settings_get();
            (st.chat.clone(), st.embedding.clone(), st.providers.clone(), st.tidy.clone())
        };
        let admin = p.is_admin();
        let fmt = |c: &Option<ModelChoice>, none: &str| c.as_ref().map_or(none.to_string(), |c| format!("{} · {}", c.provider, c.model));
        g.text(a.x, a.y, 15.0 * s, MUTED, "Chat model", Face::Sans);
        // the chat model's name keeps to the room right of the label (a long name or a narrow window cut it with an ellipsis)
        let chat_text = g.fit(15.0 * s, &fmt(&chat, "none selected"), Face::Bold, (a.w - 150.0 * s).max(40.0 * s));
        g.text(a.x + 150.0 * s, a.y, 15.0 * s, if chat.is_some() { OK } else { ERR }, &chat_text, Face::Bold);
        // the tidy model (key y) sits on the chat model's row, only when one is set: the page keeps its height
        if let Some(t) = &tidy {
            let cw = g.text_width(15.0 * s, &chat_text, Face::Bold);
            let tx = a.x + 150.0 * s + cw + 18.0 * s;
            let line = g.fit(15.0 * s, &format!("tidy: {} · {}  [y]", t.provider, t.model), Face::Bold, (a.x + a.w - tx).max(40.0));
            g.text(tx, a.y, 15.0 * s, MUTED, &line, Face::Bold);
            g.hit(R::new(tx, a.y - 2.0 * s, (a.x + a.w - tx).max(40.0), 22.0 * s), Event::Tick, &format!("Tidy model: {} · {}", t.provider, t.model));
        }
        g.text(a.x, a.y + 24.0 * s, 15.0 * s, MUTED, "Embeddings", Face::Sans);
        let emb_text = g.fit(15.0 * s, &fmt(&emb, "built-in (hashing, no model needed)"), Face::Bold, (a.w - 150.0 * s).max(40.0 * s));
        g.text(a.x + 150.0 * s, a.y + 24.0 * s, 15.0 * s, TEXT, &emb_text, Face::Bold);
        let price = p.settings_get().token_price_cents_per_mtok;
        g.text(a.x, a.y + 48.0 * s, 15.0 * s, MUTED, "Agent price", Face::Sans);
        let price_text = g.fit(15.0 * s, &if price == 0 { "not set (cost budgets are not enforced)".to_string() } else { format!("{price} cents per million tokens  [p]") }, Face::Bold, (a.w - 150.0 * s).max(40.0 * s));
        g.text(a.x + 150.0 * s, a.y + 48.0 * s, 15.0 * s, TEXT, &price_text, Face::Bold);
        let vision_line = {
            let set = p.settings_get().vision.clone();
            match (p.lemonade().vision_model(None), set) {
                (Ok(pv), Some(_)) => (pv.line(), OK),
                (Ok(pv), None) => (format!("automatic  ·  {}", pv.line()), OK),
                (Err(e), _) => (format!("automatic  ·  {}", e.message()), crate::fx::AMBER),
            }
        };
        g.text(a.x, a.y + 120.0 * s, 15.0 * s, MUTED, "Vision model", Face::Sans);
        let fitted = g.fit(15.0 * s, &format!("{}  [v]", vision_line.0), Face::Bold, (a.w - 150.0 * s).max(40.0));
        g.text(a.x + 150.0 * s, a.y + 120.0 * s, 15.0 * s, vision_line.1, &fitted, Face::Bold);
        g.hit(R::new(a.x, a.y + 118.0 * s, a.w, 22.0 * s), Event::Tick, &format!("Vision model: {}", vision_line.0));
        let routing_off = p.settings_get().no_tool_routing;
        g.text(a.x, a.y + 96.0 * s, 15.0 * s, MUTED, "Tool routing", Face::Sans);
        let mine_off = p.agent().toolrouting_person_off(&p.data_dir(), &p.user());
        let routing_text = match (routing_off, mine_off) {
            (true, _) => "off for everyone: every tool is offered  [T]",
            (false, true) => "off for you: every tool is offered  [r] you  [T] machine",
            (false, false) => "on: only the tools the chat needs  [r] you  [T] machine",
        };
        let routing_fit = g.fit(15.0 * s, routing_text, Face::Bold, (a.w - 150.0 * s).max(40.0));
        g.text(a.x + 150.0 * s, a.y + 96.0 * s, 15.0 * s, TEXT, &routing_fit, Face::Bold);
        // local models that fit this machine (docs/AI-HARDWARE.md): the profile's own words, cut to the width
        let ai = p.agent().ai_profile().this_machine();
        g.text(a.x, a.y + 72.0 * s, 15.0 * s, MUTED, "This machine", Face::Sans);
        let full = format!("{} ({})", ai.fits_line(), ai.tier_words());
        let fits = g.fit(15.0 * s, &full, Face::Bold, (a.w - 150.0 * s).max(40.0));
        g.text(a.x + 150.0 * s, a.y + 72.0 * s, 15.0 * s, TEXT, &fits, Face::Bold);
        // a no-op region carrying the whole line (what a screen reader says when the drawn text is cut to the width)
        g.hit(R::new(a.x, a.y + 70.0 * s, a.w, 22.0 * s), Event::Tick, &format!("This machine: {full}"));
        let mut y = a.y + 152.0 * s;
        if provs.is_empty() {
            let msg = if admin { "No model providers yet. Press a to add Ollama, OpenAI, Claude, Gemini, OpenRouter or any OpenAI-compatible server, or N for NVIDIA NIM (a free key, with the steps shown)." } else { "No model providers configured. Ask an administrator." };
            crate::fx::empty_state(g, R::new(a.x, y, a.w, 220.0 * s), s, Icon::Settings, "No model providers yet", msg, ACCENT);
        }
        for (i, pv) in provs.iter().enumerate() {
            // a window too short for the list gets the page's scroll bar (the generic one), not a list cut short: the chosen provider's
            // buttons and the option buttons under it stay reachable at any size and with any font
            if i >= 40 {
                break;
            }
            let key_ok = match &pv.key_ref {
                None => ("no key", DIM),
                Some(r) if r.starts_with("ext:") => ("key: provider", MUTED),
                Some(r) => {
                    if p.secret_has_ref(r) { ("key ✓", OK) } else { ("key ✗ locked/missing", ERR) }
                }
            };
            let url = if admin { pv.base_url.clone().unwrap_or_else(|| pv.kind.default_base().unwrap_or("").to_string()) } else { String::new() };
            let sub = format!("{}{}{}", url, if url.is_empty() { "" } else { "  ·  " }, Self::models_summary(p, pv));
            let active = chat.as_ref().is_some_and(|c| c.provider == pv.id);
            let chips: Vec<(&str, [u8; 4])> = [Some((pv.kind.label(), ACCENT)), Some(key_ok), if active { Some(("chat", OK)) } else { None }, if pv.enabled { None } else { Some(("off", ERR)) }, if pv.native_pdf_output { Some(("writes PDFs", DIM)) } else { None }].into_iter().flatten().collect();
            let mut chips = chips;
            if pv.id == iyakki_shared::gemini::PROVIDER_ID {
                chips.push(("experimental", ERR));
                chips.push(Self::gem_chip(p));
            }
            let selected = i == self.sel();
            let rh = if selected { 108.0 * s } else { 62.0 * s };
            row(g, &mut self.tip, i, a.x, y, a.w, rh, s, selected, &pv.id, &sub, &chips);
            if selected {
                // the actions of the chosen provider, right on its row
                let mut bx = a.x + 18.0 * s;
                let by = y + 62.0 * s;
                let mut acts: Vec<(&str, [u8; 4], char)> = if admin { vec![("Edit", TEXT, 'e'), ("Delete", ERR, 'd'), ("Models", ACCENT, 'm'), ("Test", TEXT, 't'), ("Key", TEXT, 'k'), (if pv.enabled { "Turn off" } else { "Turn on" }, TEXT, 'x')] } else { vec![("Test", ACCENT, 't')] };
                if pv.kind == Kind::Lemonade {
                    acts.insert(if admin { 2 } else { 1 }, ("All models", ACCENT, 'l'));
                }
                if pv.id == iyakki_shared::gemini::PROVIDER_ID {
                    if admin {
                        acts.push(("Gemini setup", ACCENT, 'g'));
                    }
                    acts.push(("Consent", ACCENT, 'C'));
                }
                for (label, col, key) in acts {
                    let bw = g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.7;
                    if bx + bw > a.x + a.w - 12.0 * s {
                        break;
                    }
                    bx += g.button(bx, by, 13.0 * s, label, col, Event::Key(Key::Char(key))) + 8.0 * s;
                }
            }
            y += rh + 8.0 * s;
        }
        if let Some(pv) = provs.get(self.sel()).filter(|pv| pv.kind == Kind::Lemonade) {
            // what this Lemonade server has, by type, with a quick action on each model
            let top = y + 4.0 * s;
            self.render_lemonade(g, R::new(a.x, top, a.w, (a.y + a.h - top).max(0.0)), s, p, pv);
        } else if let Some(pv) = provs.get(self.sel()).cloned() {
            let top = y + 2.0 * s;
            self.mo_render(g, R::new(a.x, top, a.w, (a.y + a.h - top).max(0.0)), s, p, &pv, admin);
        }
    }

    /// The provider card's last words: what its models are (never a bare "0 models").
    pub(super) fn models_summary(p: &dyn ModelsPort, pv: &ProviderCfg) -> String {
        if pv.kind == Kind::Lemonade {
            let list = p.lemonade().lemonade_cached(&pv.id);
            if list.is_empty() {
                return "models not read yet: press Test or All models".into();
            }
            let installed = list.iter().filter(|m| m.downloaded || m.loaded).count();
            return format!("{installed} installed · {} available", list.len() - installed);
        }
        match pv.models.len() {
            0 => "no chat models yet (press Models)".into(),
            1 => "1 chat model saved".into(),
            n => format!("{n} chat models saved"),
        }
    }

    /// The chat models the providers list, as `provider/model  (provider)`, for the tidy model picker (small and fast ones first is the person's call).
    pub(super) fn tidy_candidates(p: &dyn ModelsPort) -> Vec<String> {
        let provs = p.settings_get().providers.clone();
        let mut out: Vec<String> = Vec::new();
        for pv in provs.iter().filter(|pv| pv.enabled) {
            let mut models: Vec<String> = pv.models.clone();
            models.extend(pv.default_model.clone());
            models.extend(p.agent().modelopts_cached_models(&pv.id).unwrap_or_default());
            models.sort();
            models.dedup();
            out.extend(models.into_iter().map(|m| format!("{}/{}  ({})", pv.id, m, pv.kind.label())));
        }
        out
    }
}

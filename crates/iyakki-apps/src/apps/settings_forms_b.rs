//! Lane ST-B: the submit arms of its forms (moved out of settings.rs, S2 of SETTINGS-EXTRACTION-PLAN.md; no behaviour change).

use super::*;

impl SettingsApp {
    /// The forms of lane ST-B: NotMine when the kind is another lane's.
    pub(super) fn submit_form_b(&mut self, cx: &AppCx, kind: &FormKind, form: &mut Form) -> FormOutcome {
        match kind {
            FormKind::GeminiSetup => FormOutcome::Early(self.gem_submit(cx, form)),
            FormKind::GeminiConsent => {
                self.gem_consent_submit(cx, form);
                return FormOutcome::Early(None);
            }
            FormKind::RouterBudget | FormKind::RouterScript => {
                let r = if matches!(kind, FormKind::RouterBudget) { self.router_budget_submit(cx, form) } else { self.router_script_submit(cx, form) };
                return FormOutcome::Early(match r {
                    Ok(_) => None,
                    Err(e) => {
                        form.error = Some(e);
                        Some(Modal::Form(kind.clone(), form.clone()))
                    }
                });
            }
            FormKind::GeminiLiveConsent => {
                self.gv_consent_submit(cx, form);
                return FormOutcome::Early(None);
            }
            FormKind::AlexaConsent => {
                self.alexa_consent_submit(cx, form);
                return FormOutcome::Early(None);
            }
            FormKind::Provider(editing) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                self.save_provider(cx, form, editing.clone()).map(|_| format!("Saved provider `{}`.", if let Some(e) = editing { e.clone() } else { form.value(0) }))
            }),
            FormKind::NvidiaQuick => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                self.save_nvidia(cx, form.raw(0).trim())
            }),
            FormKind::ProviderKey(id) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let key = form.raw(0);
                if key.is_empty() {
                    Err("Enter the key.".into())
                } else {
                    let name = format!("{id}-key");
                    self.store_key(cx, &name, &key, true).and_then(|_| {
                        let id2 = id.clone();
                        Self::svc(cx)
                            .settings()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .update(|s| {
                                if let Some(p) = s.providers.iter_mut().find(|p| p.id == id2) {
                                    p.key_ref = Some(format!("vault:{name}"));
                                }
                            })
                            .map_err(|e| e.to_string())
                    })
                    .map(|_| format!("Key for `{id}` stored in the system vault."))
                }
            }),
            FormKind::External(editing) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                self.save_external(cx, form, editing.clone()).map(|_| "Saved external RAG backend.".to_string())
            }),
            FormKind::VoiceStt | FormKind::VoiceTts => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let stt = *kind == FormKind::VoiceStt;
                self.save_speech_engine(cx, form, stt).map(|_| format!("Saved the {} engine.", if stt { "speech-to-text" } else { "text-to-speech" }))
            }),
            FormKind::VoiceCmds => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let (rec, play) = (form.value(0), form.value(1));
                cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).update(|st| {
                    st.voice.record_cmd = rec;
                    st.voice.play_cmd = play;
                }).map(|_| {
                    cx.audit.record(&cx.session.user, "settings.voice.commands", "");
                    "Saved the microphone and speaker commands.".to_string()
                }).map_err(|e| e.to_string())
            }),
            FormKind::TokenPrice => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                match form.value(0).trim().parse::<u32>() {
                    Err(_) => Err("enter a whole number of cents (0 or more)".to_string()),
                    Ok(v) => cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).update(|s| s.token_price_cents_per_mtok = v).map(|_| {
                        cx.audit.record(&cx.session.user, "settings.token_price", &v.to_string());
                        format!("Agent token price: {v} cents per million tokens.")
                    }).map_err(|e| e.to_string()),
                }
            }),
            FormKind::AppAccessLimit(i, c) => FormOutcome::Result(self.appaccess_limit_submit(cx, &i, &c, form)),
            FormKind::ModelHost => FormOutcome::Result(self.mh_submit(form)),
            FormKind::ModelOpt(f) => FormOutcome::Result(self.mo_submit(cx, &f, form)),
            FormKind::RagRerankModel => FormOutcome::Result(self.rag_rerank_submit(cx, form)),
            FormKind::AgentModel(_) | FormKind::AgentCustom(_) => FormOutcome::Result(match self.agents_submit(cx, &kind, form) {
                Some(r) => r,
                None => return FormOutcome::Early(None),
            }),
            FormKind::NewSkill => FormOutcome::Result({
                let md = format!("---\nname: {}\ndescription: {}\ntriggers: {}\n---\n{}\n", form.value(0), form.value(1), form.value(2), form.value(3));
                self.add_skill(cx, &md, form.choice_idx(4) == 1)
            }),
            FormKind::ImportSkill => FormOutcome::Result({
                let path = form.value(0);
                let shared = form.choice_idx(1) == 1;
                Self::read_skill_file(cx, &path).and_then(|md| self.add_skill(cx, &md, shared))
            }),
            _ => FormOutcome::NotMine,
        }
    }
}

impl SettingsApp {
    pub(super) fn apply_pick_b(&mut self, cx: &AppCx, kind: PickKind, item: &str) -> bool {
        if matches!(kind, PickKind::RouterStrategy | PickKind::RouterRemove | PickKind::AgentModel(_) | PickKind::AgentKey(_) | PickKind::Options(_) | PickKind::Lemonade(_) | PickKind::Vision | PickKind::Tidy | PickKind::Chat(_) | PickKind::Embed(_)) && !self.admin(cx) {
            return true;
        }
        if matches!(kind, PickKind::RouterStrategy | PickKind::RouterRemove) {
            self.router_pick(cx, kind, item);
            return true;
        }
        if self.agents_pick(cx, &kind, item) {
            return true;
        }
        if let PickKind::Options(p) = &kind {
            self.mo_pick(cx, p, item);
            return true;
        }
        if let PickKind::Lemonade(p) = kind {
            self.lem_pick(cx, p, item);
            return true;
        }
        if let PickKind::Vision = &kind {
            let choice = if item.starts_with("(automatic") { None } else { item.split_once("  (").and_then(|x| x.0.split_once('/')).map(|(p, m)| ModelChoice { provider: p.into(), model: m.into() }) };
            let r = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).update(|s| s.vision = choice.clone());
            Self::svc(cx).clear_vision_cache();
            match r {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.vision_model", &choice.as_ref().map_or("automatic".to_string(), |c| format!("{}/{}", c.provider, c.model)));
                    self.ok(match &choice {
                        Some(c) => format!("Vision model: {} · {}. Pictures shown to a model go there.", c.provider, c.model),
                        None => "Vision model: automatic (the chat model if it can see pictures, else one picked from the providers).".to_string(),
                    });
                }
                Err(e) => self.fail(e.to_string()),
            }
            return true;
        }
        if let PickKind::Tidy = &kind {
            let choice = if item.starts_with("(the chat model") { None } else { item.split_once("  (").and_then(|x| x.0.split_once('/')).map(|(p, m)| ModelChoice { provider: p.into(), model: m.into() }) };
            let r = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).update(|s| s.tidy = choice.clone());
            match r {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.tidy_model", &choice.as_ref().map_or("chat model".to_string(), |c| format!("{}/{}", c.provider, c.model)));
                    self.ok(match &choice {
                        Some(c) => format!("Tidy model: {} · {}. Dictated wording and the Assistant pane go there (text on another machine still asks first).", c.provider, c.model),
                        None => "Tidy model: the chat model.".to_string(),
                    });
                }
                Err(e) => self.fail(e.to_string()),
            }
            return true;
        }
        let (r, what, provider) = match &kind {
            PickKind::Chat(p) => (
                cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).update(|s| {
                    s.chat = Some(ModelChoice { provider: p.clone(), model: item.into() });
                    if let Some(pr) = s.providers.iter_mut().find(|x| x.id == *p) {
                        pr.default_model = Some(item.into());
                    }
                }),
                "chat",
                p,
            ),
            PickKind::Embed(p) => {
                let choice = if item.starts_with("(built-in") { None } else { Some(ModelChoice { provider: p.clone(), model: item.into() }) };
                (cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).set_embedding(choice), "embedding", p)
            }
            _ => return false,
        };
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, &format!("settings.{what}_model"), &format!("{provider}/{item}"));
                let extra = if what == "embedding" && Self::svc(cx).reindex_stale_in_background() { " Existing documents are being re-embedded in the background." } else { "" };
                self.ok(format!("{what} model: {provider} · {item}.{extra}"));
            }
            Err(e) => self.fail(e.to_string()),
        }
        true
    }
}

impl SettingsApp {
    pub(super) fn apply_confirm_b(&mut self, cx: &AppCx, kind: ConfirmKind) -> bool {
        if let ConfirmKind::Router(a) = kind {
            self.router_confirm(cx, a);
            return true;
        }
        if matches!(kind, ConfirmKind::GeminiRevoke) {
            self.gem_revoke(cx);
            return true;
        }
        if let ConfirmKind::Lemonade(act) = kind {
            self.lem_confirm(cx, act);
            return true;
        }
        if self.admin(cx) && self.agents_confirm(cx, &kind) {
            return true;
        }
        let r: Result<String, String> = match &kind {
            ConfirmKind::Provider(id) if self.admin(cx) => cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove_provider(id).map(|_| format!("Removed provider `{id}`.")).map_err(|e| e.to_string()),
            ConfirmKind::External(id) if self.admin(cx) => cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove_external(id).map(|_| format!("Removed `{id}`.")).map_err(|e| e.to_string()),
            ConfirmKind::Skill(scope, name) => {
                let p = &cx.session.policy;
                Self::svc(cx).skills().remove(scope, name, p.manage_skills, p.skills).map(|_| {
                    cx.audit.record(&cx.session.user, "skills.delete", &format!("{}:{name}", scope.label()));
                    format!("Deleted skill `{name}`.")
                }).map_err(|e| e.to_string())
            }
            _ => return false,
        };
        self.confirm_done(cx, r);
        true
    }
}

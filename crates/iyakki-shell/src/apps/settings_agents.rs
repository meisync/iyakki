//! Settings → Agents: Claude Code, GitHub Copilot and custom commands that can answer the chat next to the built-in models.
//! Look at them (installed, version, signed in), install with the exact command shown and asked first, API key from the vault,
//! default model, Auto on or off for each, and a test the person starts. IYAKKI never handles a vendor password: signing in is
//! the vendor's own command, run in a terminal by the person.

use super::*;
use iyakki_agent::extagent::{self, CustomCfg, ExternalAgent};

impl SettingsApp {
    fn agents_cfg(cx: &AppCx) -> extagent::AgentsCfg {
        Self::svc(cx).agents_cfg()
    }

    /// The agents in list order: the ids and a snapshot of each one's look.
    pub(super) fn agent_ids(cx: &AppCx) -> Vec<String> {
        Self::agents_cfg(cx).all().iter().map(|a| a.id()).collect()
    }

    fn agent_by_id(cx: &AppCx, id: &str) -> Option<Box<dyn ExternalAgent>> {
        Self::agents_cfg(cx).agent(id)
    }

    fn set_agents(&mut self, cx: &AppCx, f: impl FnOnce(&mut extagent::AgentsCfg), done: String) {
        let r = Self::svc(cx).settings.lock().unwrap().update(|s| f(&mut s.agents));
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.agents", &done);
                self.ok(done);
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// The workspace whose project context the two switches below change: the one last opened in the AI IDE.
    fn context_workspace(cx: &AppCx) -> Option<(String, std::path::PathBuf)> {
        let reg = iyakki_agent::workspace::Registry::load(cx.services.dir(), &cx.session.user);
        let id = reg.last.clone().or_else(|| reg.ordered().first().map(|w| w.id.clone()))?;
        let w = reg.get(&id)?;
        std::path::Path::new(&w.root).is_dir().then(|| (w.name.clone(), std::path::PathBuf::from(&w.root)))
    }

    fn context_setting(&mut self, k: &Key, cx: &AppCx) {
        let Some((name, root)) = Self::context_workspace(cx) else { return self.fail("Open a workspace in the AI IDE first: the project context belongs to a workspace.") };
        let r = iyakki_agent::notes::edit(&root, |n| {
            if matches!(k, Key::Char('c')) {
                n.settings.agents_may_add = n.settings.agents_may_add.next();
                Ok(format!("Agents may add to the project context of {name}: {}. Everyone who clones the project gets this setting.", n.settings.agents_may_add.label()))
            } else {
                n.settings.share_external = !n.settings.share_external;
                Ok(format!("{} the context of {name} in CLAUDE.md / AGENTS.md (between the iyakki:context markers). Everyone who clones the project gets this setting.", if n.settings.share_external { "Sharing" } else { "No longer sharing" }))
            }
        });
        match r {
            Ok(m) => {
                cx.audit.record(&cx.session.user, "context.setting", &name);
                self.ok(m)
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn handle_agents(&mut self, k: &Key, cx: &AppCx) {
        let ids = Self::agent_ids(cx);
        match k {
            Key::Up => return self.move_sel(-1, ids.len()),
            Key::Down => return self.move_sel(1, ids.len()),
            _ => {}
        }
        // the project context of the workspace last opened (shared with everyone who clones it): not an administrator's setting
        if matches!(k, Key::Char('c') | Key::Char('g')) {
            return self.context_setting(k, cx);
        }
        // signing in is personal (the login is kept in the person's own home folder): not an administrator's setting
        if matches!(k, Key::Char('l') | Key::Char('o')) {
            let Some(id) = ids.get(self.sel()) else { return };
            match crate::agent_login::AGENTS.iter().find(|a| **a == id.as_str()) {
                Some(a) if Self::svc(cx).ext_status.get(id).is_some_and(|s| s.installed) => {
                    crate::agent_login::request(a, *k == Key::Char('o'));
                }
                Some(_) => self.fail(format!("{} is not installed yet; press i to install it first.", crate::agent_login::name_of(id))),
                None => self.fail("A custom agent signs in the way its own command does; there is no Login button for it."),
            }
            return;
        }
        if !self.admin(cx) {
            return;
        }
        let cfg = Self::agents_cfg(cx);
        let Some(id) = ids.get(self.sel()).cloned() else { return };
        let Some(agent) = cfg.agent(&id) else { return };
        let label = agent.label();
        match k {
            Key::Char('d') | Key::Char('r') => {
                Self::svc(cx).ext_status.refresh(cfg);
                self.ok("Looking at the agents (their --version and sign-in status; no model is called)…");
            }
            Key::Char('i') => {
                if Self::svc(cx).ext_status.get(&id).is_some_and(|s| s.installed) {
                    return self.ok(format!("{label} is already installed."));
                }
                // the same catalogue entry and the same install as the CLI tile: one place that knows what is installed
                let Some(cli) = cli_id(&id) else { return self.fail(format!("{label} is a command you installed yourself; install it the vendor's way, then press d.")) };
                match Self::svc(cx).clis.install_plan(cli) {
                    Some(plan) => self.modal = Some(Modal::Confirm(ConfirmKind::AgentInstall(id.clone()), format!("Install {label}? {plan}"))),
                    None => self.fail(format!("{label} cannot be installed from here; use the CLI tile.")),
                }
            }
            Key::Char('k') => match Self::svc(cx).system_names() {
                Ok(names) => {
                    let mut items = vec!["(none: use the program's own sign-in)".to_string()];
                    items.extend(names);
                    self.modal = Some(Modal::Pick(PickKind::AgentKey(id.clone()), Picker::new(&format!("API key for {label} (from the system vault)"), items)));
                }
                Err(_) => self.fail("The system vault is locked or empty. Add the key in Settings → Vault first."),
            },
            Key::Char('m') => {
                let items = model_items(Self::svc(cx), agent.as_ref());
                self.modal = Some(Modal::Pick(PickKind::AgentModel(id.clone()), Picker::new(&format!("Model for {label}"), items)));
            }
            Key::Char('a') => {
                let on = cfg.cfg_of(&id).is_some_and(|c| c.0);
                if on {
                    self.set_auto(cx, &id, false);
                } else {
                    self.modal = Some(Modal::Confirm(ConfirmKind::AgentAuto(id.clone()), format!("Allow the approval mode Auto for {label}? Its edits in the workspace and ordinary commands then run without asking. Deleting things, secrets, anything outside the workspace and cluster teardown still ask.")));
                }
            }
            Key::Char('v') if id == extagent::COPILOT => {
                let to = !cfg.copilot.advisor_only;
                self.set_agents(
                    cx,
                    |a| a.copilot.advisor_only = to,
                    if to { "GitHub Copilot is an advisor now: read-only, no IYAKKI tools; you apply its edits.".to_string() } else { "GitHub Copilot works through IYAKKI's tools and approvals again (its own write, shell and web tools stay off).".to_string() },
                );
            }
            Key::Char('u') => self.set_agents(cx, |a| a.default = Some(id.clone()), format!("New chats start with {label}.")),
            Key::Char('U') => self.set_agents(cx, |a| a.default = None, "New chats start with the built-in model.".to_string()),
            Key::Char('x') => {
                let (enabled, built_in) = match id.as_str() {
                    extagent::CLAUDE_CODE => (cfg.claude.enabled, true),
                    extagent::COPILOT => (cfg.copilot.enabled, true),
                    _ => (true, false),
                };
                if !built_in {
                    return self.fail("A custom agent is removed with r (or edited with e).");
                }
                self.set_agents(
                    cx,
                    |a| {
                        let c = if id == extagent::CLAUDE_CODE { &mut a.claude } else { &mut a.copilot };
                        c.enabled = !enabled;
                    },
                    format!("{label} {} in the model list.", if enabled { "hidden" } else { "shown" }),
                );
            }
            Key::Char('t') => {
                self.modal = Some(Modal::Confirm(ConfirmKind::AgentTest(id.clone()), format!("Test {label}? This runs its version and sign-in check, then sends ONE tiny request (“Reply with OK”) to the vendor, which uses a few tokens of your plan or key. Nothing is sent unless you say yes.")));
            }
            Key::Char('n') => self.open_form(FormKind::AgentCustom(None), Self::custom_form(&CustomCfg::default())),
            Key::Char('e') => match cfg.custom.iter().find(|c| format!("custom:{}", c.id) == id) {
                Some(c) => self.open_form(FormKind::AgentCustom(Some(c.id.clone())), Self::custom_form(c)),
                None => self.fail("Only a custom agent has a command to edit; use m for the model, k for the key."),
            },
            Key::Char('R') => match cfg.custom.iter().find(|c| format!("custom:{}", c.id) == id) {
                Some(c) => self.modal = Some(Modal::Confirm(ConfirmKind::AgentRemove(c.id.clone()), format!("Remove the custom agent “{}”?", c.id))),
                None => self.fail("Built-in agents are hidden with x, not removed."),
            },
            _ => {}
        }
    }

    /// Choose a model from the list (empty id: the agent's own default).
    fn save_model(&mut self, cx: &AppCx, id: &str, model: &str) {
        match self.write_model(cx, id, model, false) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.agents", &format!("{id} model {model}"));
                self.ok(if model.is_empty() { "The agent chooses its own model.".to_string() } else { format!("Model: {model}.") })
            }
            Err(e) => self.fail(e),
        }
    }

    fn write_model(&mut self, cx: &AppCx, id: &str, model: &str, remember: bool) -> Result<(), String> {
        let m = model.to_string();
        Self::svc(cx)
            .settings
            .lock()
            .unwrap()
            .update(|s| {
                let (cur, list) = match id {
                    extagent::CLAUDE_CODE => (&mut s.agents.claude.model, Some(&mut s.agents.claude.custom_models)),
                    extagent::COPILOT => (&mut s.agents.copilot.model, Some(&mut s.agents.copilot.custom_models)),
                    other => match other.strip_prefix("custom:").and_then(|c| s.agents.custom.iter_mut().find(|x| x.id == c)) {
                        Some(c) => (&mut c.model, Some(&mut c.custom_models)),
                        None => return,
                    },
                };
                *cur = m.clone();
                if let (true, Some(l)) = (remember && !m.is_empty(), list) {
                    if !l.contains(&m) && l.len() < 12 {
                        l.push(m);
                    }
                }
            })
            .map_err(|e| e.to_string())
    }

    fn set_auto(&mut self, cx: &AppCx, id: &str, on: bool) {
        let label = Self::agent_by_id(cx, id).map_or(id.to_string(), |a| a.label());
        self.set_agents(
            cx,
            |a| match id {
                extagent::CLAUDE_CODE => a.claude.allow_auto = on,
                extagent::COPILOT => a.copilot.allow_auto = on,
                other => {
                    if let Some(c) = other.strip_prefix("custom:").and_then(|c| a.custom.iter_mut().find(|x| x.id == c)) {
                        c.allow_auto = on;
                    }
                }
            },
            format!("Auto is {} for {label}.", if on { "allowed" } else { "off" }),
        );
    }

    fn custom_form(c: &CustomCfg) -> Form {
        Form::new(if c.id.is_empty() { "Add a custom agent" } else { "Edit the custom agent" })
            .text("Id", &c.id, "letters, digits, dashes; e.g. aider")
            .text("Name", &c.name, "shown in the model list")
            .text("Command", &c.command, "e.g. aider --yes --message {prompt}  (no {prompt}: the request goes to standard input)")
            .text("Key variable", &c.key_env, "optional, e.g. OPENAI_API_KEY (the key comes from the vault)")
            .text("Models command", &c.models_command, "optional: a command that prints its models, one per line (run only when you press Detect)")
    }

    /// Saving the forms of this page. `Err` keeps the form open with the message.
    pub(super) fn agents_submit(&mut self, cx: &AppCx, kind: &FormKind, form: &Form) -> Option<Result<String, String>> {
        match kind {
            FormKind::AgentModel(id) => {
                let m = form.value(0).trim().to_string();
                if !extagent::valid_model_id(&m) {
                    return Some(Err("A model id uses letters, digits and . _ - : / @ + [ ] only: no spaces, quotes or shell characters.".into()));
                }
                Some(self.write_model(cx, id, &m, true).map(|_| format!("Model set to {m} (remembered for this agent).")))
            }
            FormKind::AgentCustom(editing) => {
                let c = CustomCfg { id: form.value(0).trim().to_string(), name: form.value(1).trim().to_string(), command: form.value(2).trim().to_string(), key_env: form.value(3).trim().to_string(), models_command: form.value(4).trim().to_string(), ..Default::default() };
                let r = Self::svc(cx).settings.lock().unwrap().update(|s| {
                    if let Some(old) = s.agents.custom.iter_mut().find(|x| Some(&x.id) == editing.as_ref()) {
                        let keep = old.clone();
                        *old = CustomCfg { model: keep.model, custom_models: keep.custom_models, key_ref: keep.key_ref, allow_auto: keep.allow_auto, ..c.clone() };
                    } else {
                        s.agents.custom.push(c.clone());
                    }
                });
                Some(r.map(|_| format!("Saved the custom agent `{}`. Press d to check it.", c.id)).map_err(|e| e.to_string()))
            }
            _ => None,
        }
    }

    pub(super) fn agents_pick(&mut self, cx: &AppCx, kind: &PickKind, item: &str) -> bool {
        if let PickKind::AgentModel(id) = kind {
            let Some(agent) = Self::agent_by_id(cx, id) else { return true };
            if item.starts_with("Custom model id") {
                let f = Form::new(&format!("Custom model id for {}", agent.label())).text("Model id", "", "letters, digits and . _ - : / @ + [ ] only, e.g. claude-sonnet-5");
                self.open_form(FormKind::AgentModel(id.clone()), f);
                return true;
            }
            let Some(mi) = Self::svc(cx).ext_status.models_for(agent.as_ref()).into_iter().find(|m| model_item(m) == item) else { return true };
            self.save_model(cx, id, &mi.id);
            return true;
        }
        let PickKind::AgentKey(id) = kind else { return false };
        let key_ref = if item.starts_with('(') { String::new() } else { format!("vault:{item}") };
        self.set_agents(
            cx,
            |a| match id.as_str() {
                extagent::CLAUDE_CODE => a.claude.key_ref = key_ref.clone(),
                extagent::COPILOT => a.copilot.key_ref = key_ref.clone(),
                other => {
                    if let Some(c) = other.strip_prefix("custom:").and_then(|c| a.custom.iter_mut().find(|x| x.id == c)) {
                        c.key_ref = key_ref.clone();
                    }
                }
            },
            if key_ref.is_empty() { "No API key: the program's own sign-in is used.".to_string() } else { format!("The key `{item}` from the vault is passed to the program as an environment variable (never on the command line).") },
        );
        true
    }

    pub(super) fn agents_confirm(&mut self, cx: &AppCx, kind: &ConfirmKind) -> bool {
        match kind {
            ConfirmKind::AgentAuto(id) => self.set_auto(cx, id, true),
            ConfirmKind::AgentRemove(id) => {
                let id = id.clone();
                self.set_agents(
                    cx,
                    |a| {
                        a.custom.retain(|c| c.id != id);
                        if a.default.as_deref() == Some(&format!("custom:{id}")) {
                            a.default = None;
                        }
                    },
                    format!("Removed `{id}`."),
                )
            }
            ConfirmKind::AgentInstall(id) => {
                let Some(cli) = cli_id(id) else { return true };
                cx.audit.record(&cx.session.user, "cli.install", cli);
                match Self::svc(cx).clis.install(cli, &cx.session.user) {
                    Ok(()) => self.ok(format!("Installing {cli} (progress is in the CLI tile; this page updates when it is done).")),
                    Err(e) => self.fail(e),
                }
            }
            ConfirmKind::AgentTest(id) => {
                let Some(agent) = Self::agent_by_id(cx, id) else { return true };
                let (svc, cfg, id) = (Self::svc(cx).clone(), Self::agents_cfg(cx), id.clone());
                cx.audit.record(&cx.session.user, "agents.test", &id);
                let key = cfg.cfg_of(&id).filter(|c| !c.1.is_empty()).and_then(|c| svc.secrets.resolve(&c.1, None).ok());
                let agent: std::sync::Arc<dyn ExternalAgent> = std::sync::Arc::from(agent);
                let ws = std::env::temp_dir();
                self.start("testing", move || JobOut::Msg(test_agent(&svc, agent, &ws, key)));
            }
            _ => return false,
        }
        true
    }

    pub(super) fn render_agents(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let cfg = Self::agents_cfg(cx);
        let cache = &Self::svc(cx).ext_status;
        let all = cfg.all();
        g.text(a.x, a.y, 14.0 * s, MUTED, "Coding agents you installed and signed into. They answer the chat next to the built-in models (Ctrl+P in the chat) and obey IYAKKI's approvals.", Face::Sans);
        let mut y = a.y + 30.0 * s;
        let sel = self.sel().min(all.len().saturating_sub(1));
        for (i, ag) in all.iter().enumerate() {
            let id = ag.id();
            let st = cache.get(&id);
            let (chip, chip_col) = match &st {
                Some(st) if st.ready() => ("ready", OK),
                Some(st) => (if st.installed { "sign in" } else { "not installed" }, ERR),
                None => ("checking…", DIM),
            };
            let auto = cfg.cfg_of(&id).is_some_and(|c| c.0);
            let key = !ag.cfg_key_ref().is_empty();
            let model_name = if ag.cfg_model().is_empty() { "default".to_string() } else { ag.cfg_model() };
            let sub = format!(
                "{} · model {} · {} · Auto {}{}",
                st.as_ref().map_or(String::new(), |s| if s.version.is_empty() { "version unknown".to_string() } else { s.version.clone() }),
                model_name.as_str(),
                ag.mode().label(),
                if auto { "allowed" } else { "off" },
                if key { " · API key from the vault" } else { "" }
            );
            let default = cfg.default.as_deref() == Some(id.as_str());
            let mut chips: Vec<(&str, [u8; 4])> = vec![(chip, chip_col)];
            if default {
                chips.push(("default", ACCENT));
            }
            let enabled = match id.as_str() {
                extagent::CLAUDE_CODE => cfg.claude.enabled,
                extagent::COPILOT => cfg.copilot.enabled,
                _ => true,
            };
            if !enabled {
                chips.push(("hidden", DIM));
            }
            let sub_fit = g.fit(13.0 * s, &sub, Face::Sans, a.w - 200.0 * s);
            row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, i == sel, &ag.label(), &sub_fit, &chips);
            y += 66.0 * s;
        }
        // the selected agent's details: what to do, and the rules it runs under
        if let Some(ag) = all.get(sel) {
            let id = ag.id();
            y += 6.0 * s;
            let fix = cache.get(&id).map(|s| s.fix).unwrap_or_default();
            let cur = ag.cfg_model();
            let models = cache.models_for(ag.as_ref());
            let src = models.first().map_or("", |m| m.source.label());
            let list = models.iter().map(|m| if m.id == cur { format!("[{}]", m.label.split(" (").next().unwrap_or(&m.label)) } else { m.label.split(" (").next().unwrap_or(&m.label).to_string() }).collect::<Vec<_>>().join(" · ");
            let t = g.fit(13.0 * s, &format!("Models ({src}, press m): {list}"), Face::Sans, a.w);
            g.text(a.x, y, 13.0 * s, TEXT, &t, Face::Sans);
            y += 20.0 * s;
            let lines: Vec<(String, [u8; 4])> = vec![
                (if fix.is_empty() { format!("{} is ready.", ag.label()) } else { fix }, if cache.get(&id).is_some_and(|s| s.ready()) { OK } else { ERR }),
                ("Works in the workspace folder only; nothing outside it is shared. Raw kubectl, k3s, kubeadm and docker rm are refused: cluster work goes through IYAKKI's tools.".to_string(), MUTED),
                (if ag.mode() == extagent::Mode::Bridged { "IYAKKI's tools (board, deploy, k8s, history…) and its approvals are offered to it over a private local socket; every action is audited.".to_string() + if id == extagent::COPILOT { " Its own write, shell and web tools are off. v: make it a read-only advisor." } else { "" } } else { "Advisor mode: it may read the workspace and answers in the chat; you apply any edits yourself. IYAKKI cannot hold it to an approval policy, so files it changes anyway are reported.".to_string() + if id == extagent::COPILOT { " v: let it use IYAKKI's tools." } else { "" } }, MUTED),
                ("Login (l) runs the vendor's own sign-in and shows its address, code and a QR code; Logout (o) signs out. IYAKKI never handles a password; an API key only comes from the vault, as an environment variable.".to_string(), MUTED),
            ];
            for (t, col) in lines {
                let t = g.fit(13.0 * s, &t, Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, col, &t, Face::Sans);
                y += 20.0 * s;
            }
            // the sign-in: who is signed in, the Login button, and "Sign in first" while it is not
            if let (Some(aid), true) = (crate::agent_login::AGENTS.iter().find(|a| **a == id.as_str()), cache.get(&id).is_some_and(|s| s.installed)) {
                let st = crate::agent_login::state(cx, aid, true);
                let (chip_text, col) = crate::agent_login::chip(&st);
                let head = if st == crate::agent_login::State::SignedOut { "Sign in first: ".to_string() } else { "Sign-in: ".to_string() };
                let hw = g.text_width(13.0 * s, &head, Face::Bold);
                g.text(a.x, y + 8.0 * s, 13.0 * s, TEXT, &head, Face::Bold);
                g.text(a.x + hw, y + 8.0 * s, 13.0 * s, col, &chip_text, Face::Sans);
                let cw = g.text_width(13.0 * s, &chip_text, Face::Sans);
                crate::agent_login::buttons(g, a.x + hw + cw + 18.0 * s, y, 13.0 * s, aid, &st);
                y += 36.0 * s;
            }
        }
        // the project context's two switches, for the workspace last opened (they live in its committed .iyakki/context.json)
        y += 8.0 * s;
        match Self::context_workspace(cx) {
            Some((name, root)) => {
                let (n, _) = iyakki_agent::notes::load(&root);
                let head = g.fit(13.0 * s, &format!("Project context of {name} (shared with everyone who clones this project):"), Face::Bold, a.w);
                g.text(a.x, y, 13.0 * s, TEXT, &head, Face::Bold);
                y += 20.0 * s;
                let l1 = g.fit(13.0 * s, &format!("c  Agents may add to the context: {}", n.settings.agents_may_add.label()), Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, MUTED, &l1, Face::Sans);
                y += 20.0 * s;
                let l2 = g.fit(13.0 * s, &format!("g  Share the context with external agents via CLAUDE.md / AGENTS.md: {}", if n.settings.share_external { "on" } else { "off" }), Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, MUTED, &l2, Face::Sans);
            }
            None => {
                let t = g.fit(13.0 * s, "Project context: open a workspace in the AI IDE to change its settings here (agents may add, share via CLAUDE.md / AGENTS.md).", Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, DIM, &t, Face::Sans);
            }
        }
    }
}

/// One line of the model chooser: the label, the note, where the line comes from.
fn model_item(m: &extagent::ModelInfo) -> String {
    format!("{}{}  [{}]{}", m.label, if m.note.is_empty() { String::new() } else { format!(" — {}", m.note) }, m.source.label(), if m.available == Some(false) { " not available on your plan" } else { "" })
}

fn model_items(svc: &std::sync::Arc<iyakki_agent::services::Services>, agent: &dyn ExternalAgent) -> Vec<String> {
    let mut v: Vec<String> = svc.ext_status.models_for(agent).iter().map(model_item).collect();
    v.push("Custom model id…".into());
    v
}

/// The CLI tile's catalogue entry for a built-in agent.
fn cli_id(agent: &str) -> Option<&'static str> {
    match agent {
        extagent::CLAUDE_CODE => Some("claude-code"),
        extagent::COPILOT => Some("github-copilot"),
        _ => None,
    }
}

/// The test button: the version, the sign-in status, then one tiny request through the same machinery a chat uses.
fn test_agent(svc: &std::sync::Arc<iyakki_agent::services::Services>, agent: std::sync::Arc<dyn ExternalAgent>, ws: &std::path::Path, key: Option<extagent::Zeroizing<String>>) -> Result<String, String> {
    let st = extagent::detect(agent.as_ref(), &iyakki_agent::wsexec::System, ws);
    svc.ext_status.set(&agent.id(), st.clone());
    if !st.ready() {
        return Err(st.fix);
    }
    let req = extagent::run::RunRequest {
        agent: agent.clone(),
        workspace: ws.to_path_buf(),
        prompt: "Reply with the single word OK.".into(),
        system: std::sync::Arc::new(String::new),
        model: agent.cfg_model(),
        resume: None,
        tools: std::sync::Arc::new(extagent::NoTools),
        limits: iyakki_agent::agent::RunLimits { max_steps: 1, max_seconds: 90, ..Default::default() },
        allow_auto: false,
        key,
        audit: std::sync::Arc::new(|_, _| {}),
        exe: std::env::current_exe().unwrap_or_default(),
        on_session: std::sync::Arc::new(|_| {}),
        on_usage: std::sync::Arc::new(|_, _, _, _| {}),
        no_tools: false,
    };
    let h = extagent::run::start(req);
    while let Ok(e) = h.events.recv_timeout(std::time::Duration::from_secs(120)) {
        match e {
            iyakki_agent::agent::RunEvent::Final { text } => return Ok(format!("{} {}: answered “{}”.", agent.label(), st.version, text.trim().chars().take(40).collect::<String>())),
            iyakki_agent::agent::RunEvent::Failed { error } => return Err(error),
            iyakki_agent::agent::RunEvent::Stopped { reason } => return Err(reason),
            _ => {}
        }
    }
    Err("No answer within two minutes.".into())
}

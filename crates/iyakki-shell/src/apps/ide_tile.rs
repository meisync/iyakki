//! The IDE side of "an app for IYAKKI": the workspace's target and its header chip, "Change target", Run / Add tile / Validate for a
//! tile app, and the guided Publish flow (checks, version, store, signing key, token, review card, result, install back from the store).
//! The rules and the work are in `iyakki_agent::{target, store::publishflow, store::keys, store::tilehub}`; this file is the screens.
//! Everything that sends, installs or changes the cluster waits for a click on a review card, in every mode.

use std::path::PathBuf;

use iyakki_agent::store::catalog::{HttpFetch, SourceCfg};
use iyakki_agent::store::install::{Actor, Review};
use iyakki_agent::devmode as dm;
use iyakki_agent::store::{keys, privatepub, publishflow as pf, tile, tilehub, Store};
use iyakki_agent::target::{self as tg, Target as AppTarget};

use super::card::{Action, Card, Tone};
use super::ide::{FormKind, IdeApp, Modal};
use super::widgets::Form;
use super::AppCx;
use crate::gfx::rgb;
use crate::ui::Key;

const OKC: [u8; 4] = rgb(110, 224, 160);
const WARN: [u8; 4] = rgb(255, 196, 94);
const CYAN: [u8; 4] = rgb(100, 230, 255);
const MUTE: [u8; 4] = rgb(150, 160, 190);

/// What a card in the IDE does when one of its buttons is chosen.
pub enum CardAction {
    /// Just information: any button closes it.
    Info,
    /// Change the target after reading the plan.
    ChangeTarget(AppTarget),
    /// Ask the assistant to fix the problems (the message to send).
    Fix(String),
    /// The review card of a publish: Approve sends it.
    PublishReview,
    /// A private store deployed on this device, not yet added: Approve adds it with the key shown pinned, chooses it as the publishing
    /// target and goes on to the publish review.
    AddStoreThenPublish,
    /// The result of a publish; `Install it here to test` is offered.
    PublishDone,
    /// The install review of the app just published.
    InstallReview(Box<Review>),
    /// A decision of the git support (a new host key, a pull over local changes, removing a folder): see `ide_git`.
    Git(super::ide_git::GitCard),
}

pub struct CardModal {
    pub card: Card,
    pub action: CardAction,
}

/// A publish in progress: what the person chose, kept between the dialogs.
pub struct PublishState {
    pub dir: PathBuf,
    pub src: Option<SourceCfg>,
    pub key: Option<(keys::SigningKey, String)>,
    pub name: String,
    pub kind: String,
    pub text: Option<String>,
    pub card: Option<pf::Card>,
    /// The deployed-but-not-added private store the Store choice listed first (the choice list is shifted by one when it is there).
    pub unadded: Option<privatepub::Offer>,
    /// The key and address the person is looking at on the "add it" card.
    pub adding: Option<iyakki_agent::store::Probe>,
}

impl IdeApp {
    /// The project of the active workspace: the root when it holds an app (`iyakki-app.json` or `app.json` + `index.html`), else the
    /// first folder below it that does (the scaffold makes one).
    pub fn project_dir(&self) -> Option<PathBuf> {
        let root = self.active_ws()?.root();
        let is_app = |d: &std::path::Path| d.join("iyakki-app.json").is_file() || (d.join("app.json").is_file() && d.join("index.html").is_file());
        if is_app(&root) {
            return Some(root);
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
        subs.sort();
        subs.into_iter().find(|d| is_app(d))
    }

    pub fn workspace_target(&self) -> Option<AppTarget> {
        self.active_ws().and_then(|w| AppTarget::from_id(&w.cfg.target))
    }

    /// Re-read the target from the project file (the assistant's `tile_scaffold` may have written it).
    pub fn refresh_target(&mut self) {
        let hint = self.publish_hint();
        if hint != self.last_hint {
            self.last_hint = hint;
            self.redraw = true;
        }
        for w in &mut self.ws {
            let t = tg::read(&w.root()).map(|t| t.id().to_string()).unwrap_or_default();
            if w.cfg.target != t {
                w.cfg.target = t;
                self.redraw = true;
            }
        }
    }

    pub(super) fn card_modal(&mut self, card: Card, action: CardAction) {
        self.modal = Some(Modal::Card(Box::new(CardModal { card, action })));
    }

    fn accent(&self) -> [u8; 4] {
        self.active_ws().map_or(CYAN, |w| w.meta.color())
    }

    // ------------------------------------------------------------------------------------------------ target

    /// The header chip: the choice of target (and the plan of changing it).
    pub fn open_target_form(&mut self) {
        let Some(w) = self.active_ws() else {
            self.notice = Some(("Open a workspace first.".into(), false));
            return;
        };
        let cur = AppTarget::from_id(&w.cfg.target);
        let labels: Vec<&str> = AppTarget::ALL.iter().map(|t| t.label()).collect();
        let f = Form::new(&format!("What is {} for?", w.meta.name)).choice("Target", &labels, cur.and_then(|c| AppTarget::ALL.iter().position(|t| *t == c)).unwrap_or(0));
        self.modal = Some(Modal::Form(FormKind::Target, f));
    }

    pub fn submit_target_form(&mut self, f: Form) {
        let t = AppTarget::ALL[f.choice_idx(0).min(AppTarget::ALL.len() - 1)];
        let Some(w) = self.active_ws() else { return };
        if tg::read(&w.root()) == Some(t) {
            return;
        }
        let plan = tg::plan_change(&w.root(), &w.meta.name, t);
        let mut c = Card::new(&format!("Change target to {}?", t.label()), self.accent()).line(&plan.summary, Tone::Normal);
        if !plan.drop_files.is_empty() {
            c = c.line("", Tone::Normal).line("The assistant will propose removing", Tone::Bold);
            for f in &plan.drop_files {
                c = c.line(&format!("- {f}"), Tone::Normal);
            }
        }
        if !plan.drop_rows.is_empty() {
            c = c.line("", Tone::Normal).line("Board rows that become deprecated", Tone::Bold);
            for (id, title) in &plan.drop_rows {
                c = c.line(&format!("- {id}  {title}"), Tone::Normal);
            }
        }
        if let Some(ns) = &plan.teardown_namespace {
            c = c.line("", Tone::Normal).line(&format!("Afterwards it offers to remove the dev namespace {ns} from the cluster. That always asks first, and nothing else on the cluster is touched."), Tone::Warn);
        }
        let go = if t == AppTarget::IyakkiTile && plan.from.is_some_and(|f| f.deploys_to_k3s()) { "Change and convert" } else { "Change target" };
        c = c.action(Action::new(go, Key::Char('y'), OKC)).action(Action::new("Not now", Key::Esc, MUTE));
        self.card_modal(c, CardAction::ChangeTarget(t));
    }

    fn apply_target(&mut self, t: AppTarget, cx: &AppCx) {
        let Some(i) = self.active else { return };
        let (root, name) = (self.ws[i].root(), self.ws[i].meta.name.clone());
        if self.ws[i].ro.is_some() {
            self.notice = Some(("This workspace is read-only for you.".into(), false));
            return;
        }
        match tg::apply_change(&root, &name, t) {
            Ok((plan, msg)) => {
                let w = &mut self.ws[i];
                w.cfg = ProjectConfigExt::reload(&root, w.cfg.clone());
                cx.audit.record(&cx.session.user, "ide.target", &format!("{} -> {}", plan.from.map_or("none", |f| f.id()), t.id()));
                w.prefs.chat_open = true;
                w.set_focus(super::ide_layout::Pane::Chat);
                w.compact_pane = super::ide_layout::Pane::Chat;
                w.say(format!("Target is now {}.", t.label()), true);
                if t == AppTarget::IyakkiTile || plan.from.is_some_and(|f| f.deploys_to_k3s()) {
                    w.chat.ask(cx, &msg);
                }
            }
            Err(e) => self.notice = Some((e, false)),
        }
        self.redraw = true;
    }

    // ------------------------------------------------------------------------------------------------ tile app actions

    /// Run for a tile app: the app, live, as an unsigned dev app (Back returns here).
    pub fn run_tile_app(&mut self, cx: &AppCx) -> bool {
        if self.workspace_target() != Some(AppTarget::IyakkiTile) {
            return false;
        }
        let Some(dir) = self.project_dir() else {
            self.notice = Some(("There is no tile app in this workspace yet: ask the assistant to build one, or use New app.".into(), false));
            return true;
        };
        if !tilehub::allowed(&cx.session.policy) {
            self.notice = Some(("Your account may not run tile apps.".into(), false));
            return true;
        }
        cx.audit.record(&cx.session.user, "ide.tile.run", &dir.display().to_string());
        self.request = Some((format!("tile-preview:{}", dir.display()), String::new()));
        true
    }

    /// "Add tile": the app of this workspace on my launcher as an unsigned dev app (administrators).
    pub fn add_tile(&mut self, cx: &AppCx) {
        let Some(dir) = self.project_dir() else {
            self.notice = Some(("There is no tile app in this workspace yet.".into(), false));
            return;
        };
        match tilehub::add_dev(&cx.services, &cx.session.policy, &cx.session.user, &dir) {
            Ok(e) => {
                cx.audit.record(&cx.session.user, "ide.tile.add", &format!("{} (dev)", e.name));
                self.notice = Some((format!("“{}” is on your launcher under {} as an unsigned dev app. It reloads when you change the files.", e.title, e.group), true));
            }
            Err(m) => self.notice = Some((m, false)),
        }
    }

    /// "Validate": the checks of the Publish flow, and for a tile app the headless tests, in one card.
    pub fn validate_app(&mut self) {
        let Some(dir) = self.project_dir() else {
            self.notice = Some(("There is no app project in this workspace (no iyakki-app.json).".into(), false));
            return;
        };
        let p = pf::preflight(&dir);
        let mut c = Card::new(&format!("Check {} {}", if p.name.is_empty() { "the project" } else { &p.name }, p.version), self.accent());
        if p.ok() {
            c = c.line("Everything passes.", Tone::Good);
        }
        for x in &p.problems {
            c = c.line(&format!("PROBLEM: {x}"), Tone::Bad);
        }
        for x in &p.warnings {
            c = c.line(&format!("warning: {x}"), Tone::Warn);
        }
        if let Some(t) = p.tests {
            c = c.line(if t { "Headless tests: pass." } else { "Headless tests: do not pass." }, if t { Tone::Good } else { Tone::Bad });
        }
        let fix = format!("Please fix these problems in {}: {}", dir.display(), p.problems.join("; "));
        c = if p.ok() { c.action(Action::new("Close", Key::Esc, CYAN)) } else { c.action(Action::new("Ask the assistant to fix", Key::Char('a'), OKC)).action(Action::new("Close", Key::Esc, MUTE)) };
        self.card_modal(c, if p.ok() { CardAction::Info } else { CardAction::Fix(fix) });
    }

    // ------------------------------------------------------------------------------------------------ publish

    /// The header's Publish button: checks first, then the form.
    pub fn open_publish(&mut self, cx: &AppCx) {
        if self.ro_check_pub("publish") {
            return;
        }
        let Some(dir) = self.project_dir() else {
            self.notice = Some(("There is no app project in this workspace to publish. Use New app, or ask the assistant to build one.".into(), false));
            return;
        };
        let p = pf::preflight(&dir);
        if !p.ok() {
            let mut c = Card::new("Publish: fix these first", self.accent()).danger();
            for x in &p.problems {
                c = c.line(&format!("- {x}"), Tone::Bad);
            }
            for x in &p.warnings {
                c = c.line(&format!("warning: {x}"), Tone::Warn);
            }
            let fix = format!("Please fix these problems in {} so it can be published: {}", dir.display(), p.problems.join("; "));
            self.card_modal(c.action(Action::new("Ask the assistant to fix", Key::Char('a'), OKC)).action(Action::new("Close", Key::Esc, MUTE)), CardAction::Fix(fix));
            return;
        }
        let store = Store::for_services(&cx.services);
        let list = pf::stores(&store);
        let fetch = HttpFetch(cx.services.quick_upstream());
        // a private store deployed from this device and not yet added comes first: choosing it adds it (key checked) and publishes
        let offer = privatepub::offer(&cx.services, &cx.session.user, &store, &fetch);
        let deployed = privatepub::deployed_all(&cx.services, &cx.session.user, &store);
        let mut options: Vec<String> = offer.iter().map(privatepub::option_label).collect();
        options.extend(list.iter().map(|s| format!("{}{} · {}{}", if s.is_default { "★ " } else { "" }, s.name, s.address, if s.fingerprint.is_empty() { String::new() } else { format!(" · key {}", s.fingerprint.split(' ').take(2).collect::<Vec<_>>().join(" ")) })));
        options.push(match deployed.first() {
            Some(d) => format!("Deploy a new private store to k3s… (already deployed: {}, not redeployed)", d.instance),
            None => "Deploy a new private store to k3s…".into(),
        });
        options.push("Export package file (no store)".into());
        let off = usize::from(offer.is_some());
        let default_store = if offer.is_some() { 0 } else { list.iter().position(|s| s.is_default && s.private).or_else(|| list.iter().position(|s| s.private)).unwrap_or(options.len() - 2) };
        // developer mode: this device, and the devices this account paired with (the same install path as `iyakki dev install`)
        options.extend(dev_targets(cx).iter().map(DevTarget::label));
        // the version the store already has decides the suggested bump
        let remote = default_store.checked_sub(off).and_then(|i| list.get(i)).filter(|s| s.private).and_then(|s| pf::remote_version(&fetch, &s.address, &p.name));
        let has = remote.as_deref().map(|r| format!(" (the store has {r})")).unwrap_or_default();
        let ahead = remote.as_deref().is_none_or(|r| iyakki_agent::pkg::newer(&p.version, r));
        let b = |k| pf::bumped(&p.version, k).unwrap_or_default();
        let vopts = [format!("Keep {}", p.version), format!("Patch to {}", b(pf::Bump::Patch)), format!("Minor to {}", b(pf::Bump::Minor)), format!("Major to {}", b(pf::Bump::Major))];
        let klist = keys::list(&cx.services, &cx.session.user);
        let mut kopts: Vec<String> = klist.iter().map(|k| format!("{} · {}", k.label, k.fingerprint.split(' ').take(2).collect::<Vec<_>>().join(" "))).collect();
        kopts.push(if klist.is_empty() { "Create my publisher key".into() } else { "Create another key".into() });
        let f = Form::new(&format!("Publish {} {}", p.name, p.version))
            .choice("Store", &refs(&options), default_store)
            .choice(&format!("Version{has}"), &refs(&vopts), if ahead { 0 } else { 1 })
            .text("Change log line", "", if p.changelog_has_version { "optional: CHANGELOG.md already mentions this version" } else { "one sentence; required when you raise the version" })
            .choice("Signing key", &refs(&kopts), 0);
        self.publish = Some(PublishState { dir, src: None, key: None, name: p.name, kind: p.kind, text: None, card: None, unadded: offer, adding: None });
        self.modal = Some(Modal::Form(FormKind::Publish, f));
    }

    fn ro_check_pub(&mut self, what: &str) -> bool {
        if self.active_ws().is_some_and(|w| w.ro.is_some()) {
            self.notice = Some((format!("This workspace is read-only for you: you cannot {what}."), false));
            true
        } else {
            false
        }
    }

    pub fn submit_publish(&mut self, f: Form, cx: &AppCx) {
        let fail = |app: &mut IdeApp, mut f: Form, e: String| {
            f.error = Some(e);
            app.modal = Some(Modal::Form(FormKind::Publish, f));
        };
        let Some(st) = self.publish.as_ref() else { return };
        let dir = st.dir.clone();
        let offer = st.unadded.clone();
        let store = Store::for_services(&cx.services);
        let list = pf::stores(&store);
        // the deployed-but-not-added private store is the first row when the form listed it; the other rows are shifted by one
        let off = usize::from(offer.is_some());
        let raw = f.choice_idx(0);
        let add_sel = off == 1 && raw == 0;
        let sel = raw.saturating_sub(off);
        let user = cx.session.user.clone();
        // what cannot work is said before anything is written (a version bump, a key)
        if add_sel {
            if let Some(Err(why)) = offer.as_ref().map(|o| o.probe.clone()) {
                return fail(self, f, why);
            }
            let actor = Actor::from_policy(&user, &cx.session.policy);
            if !(actor.allowed && actor.admin) {
                return fail(self, f, privatepub::need_admin_text());
            }
        } else if sel == list.len() {
            if let Some(d) = privatepub::deployed_all(&cx.services, &user, &store).into_iter().next() {
                let hint = if d.added.is_some() { "it is already added: choose it in the list" } else { "choose it at the top of the list" };
                return fail(self, f, format!("A private store is already deployed on this device ({}, namespace {}): {hint}. To start over use STORE, Settings, Private store, Manage (Repair or Reset).", d.instance, d.namespace));
            }
        }
        // version first: a bump is written to the project and the change log
        let bump = [None, Some(pf::Bump::Patch), Some(pf::Bump::Minor), Some(pf::Bump::Major)][f.choice_idx(1).min(3)];
        let line = f.value(2);
        if let Some(bmp) = bump {
            if line.trim().is_empty() {
                return fail(self, f, "Write one line for the change log: what changed in this version?".into());
            }
            if let Err(e) = pf::apply_bump(&dir, bmp, &line, &iyakki_agent::clock::today()) {
                return fail(self, f, e);
            }
        } else if !line.trim().is_empty() {
            let v = iyakki_agent::store::vet::read_project(&dir).and_then(|v| v["version"].as_str().map(str::to_string)).unwrap_or_default();
            let _ = pf::add_changelog(&dir, &v, &line, &iyakki_agent::clock::today());
        }
        // the signing key: an existing one, or a new one made now (the private half stays in the person's folder of the data directory)
        let klist = keys::list(&cx.services, &user);
        let key = if f.choice_idx(3) < klist.len() {
            keys::load(&cx.services, &user, Some(&klist[f.choice_idx(3)].label))
        } else {
            let mut label = "publisher".to_string();
            let mut n = 1;
            while klist.iter().any(|k| k.label == label) {
                n += 1;
                label = format!("publisher-{n}");
            }
            match keys::create(&cx.services, &user, &label) {
                Ok(_) => keys::load(&cx.services, &user, Some(&label)),
                Err(e) => return fail(self, f, e),
            }
        };
        let Some(key) = key else { return fail(self, f, "The signing key could not be read.".into()) };
        cx.audit.record(&user, "ide.publish.key", &format!("signing key {}", key.1));
        if add_sel {
            // the person looks at the key before anything is trusted: the card shows the fingerprint, and Approve adds and goes on
            let Some(Ok(p)) = offer.map(|o| o.probe) else { return fail(self, f, "The private store cannot be read now.".into()) };
            if let Some(st) = self.publish.as_mut() {
                st.key = Some(key);
                st.adding = Some(p.clone());
            }
            let ns = privatepub::deployed_all(&cx.services, &user, &store).into_iter().next().map(|d| d.namespace).unwrap_or_default();
            let c = Card::new("Add this private store and publish?", self.accent())
                .line(&format!("Store: {}", p.name), Tone::Bold)
                .line(&format!("Address: {}", p.url.trim_end_matches("/catalog.json")), Tone::Normal)
                .line("Catalog key fingerprint", Tone::Bold)
                .line(&p.fingerprint, Tone::Bold)
                .line(&p.trust, Tone::Warn)
                .line(&format!("It was deployed from this device. The store printed this fingerprint when it started: compare it with the pod log (kubectl logs -n {ns} deploy/private-store) or the store's own page. If it differs, press Cancel."), Tone::Dim)
                .line("Approving adds it to your STORE with this key pinned, chooses it as the publishing target, and then shows the publish review. Nothing is published yet.", Tone::Dim)
                .action(Action::new("Add it and publish", Key::Char('y'), OKC))
                .action(Action::new("Cancel", Key::Esc, MUTE));
            self.card_modal(c, CardAction::AddStoreThenPublish);
            return;
        }
        if sel == list.len() {
            // a new private store first: the platform wizard with the private store ready, and back here afterwards
            self.publish = None;
            self.request = Some(("settings".into(), "platform:private-store".into()));
            self.notice = Some(("Deploy the private store in the wizard; when it is ready, press Publish again: it is listed first, and one approval adds it (with its key checked) and publishes.".into(), true));
            return;
        }
        if sel == list.len() + 1 {
            // export: a signed package file in the project folder, no store
            match iyakki_agent::store::author::pack_dir(&dir, Some(&key.0)) {
                Ok((text, notes)) => {
                    let name = iyakki_agent::pkg::parse(&text).map(|p| (p.manifest.name, p.manifest.version)).unwrap_or_default();
                    let out = dir.join(format!("{}-{}.iyp", name.0, name.1));
                    match std::fs::write(&out, &text) {
                        Ok(()) => {
                            self.publish = None;
                            let c = Card::new("Package file written", self.accent()).line(&out.display().to_string(), Tone::Bold).line(&format!("{} bytes, signed with your key {}. {}", text.len(), key.1, notes.join(" ")), Tone::Normal).line("Install it with STORE, Settings, install from a file, or publish it later with the assistant's store_publish.", Tone::Dim).action(Action::new("Close", Key::Esc, CYAN));
                            self.card_modal(c, CardAction::Info);
                        }
                        Err(e) => return fail(self, f, iyakki_agent::scope::explain_io(&e, &out, "write the package file to")),
                    }
                }
                Err(e) => return fail(self, f, e),
            }
            return;
        }
        if sel >= list.len() + 2 {
            let Some(t) = dev_targets(cx).into_iter().nth(sel - list.len() - 2) else { return fail(self, f, "That device is no longer available.".into()) };
            match deploy_dev(cx, &t, &dir, &key.0) {
                Ok((name, kind, said)) => {
                    self.publish = None;
                    cx.audit.record(&user, "ide.deploy.dev", &said);
                    let c = Card::new("Installed in developer mode", self.accent())
                        .line(&said, Tone::Good)
                        .line("Labelled Developer (local): not reviewed, never updated from a store. Remove it from the STORE, or with iyakki dev uninstall.", Tone::Dim)
                        .action(Action::new("Close", Key::Esc, CYAN));
                    self.card_modal(c, CardAction::Info);
                    if kind == "tile" && matches!(t, DevTarget::ThisDevice) {
                        self.request = Some((format!("tile-open:{name}"), String::new()));
                    }
                }
                Err(e) => return fail(self, f, e),
            }
            return;
        }
        let Some(src) = list.get(sel).and_then(|s| store.config().sources.into_iter().find(|x| x.id == s.id)) else { return fail(self, f, "Choose a store.".into()) };
        if !list[sel].private {
            return fail(self, f, format!("{} is not a private store with a pinned key: only those can be published to.", src.name));
        }
        if let Some(st) = self.publish.as_mut() {
            st.src = Some(src.clone());
            st.key = Some(key);
        }
        self.token_or_review(cx);
    }

    /// With the store and key chosen: ask for the publish token when there is none in the vault (a store deployed here has one), else the review.
    fn token_or_review(&mut self, cx: &AppCx) {
        let Some(src) = self.publish.as_ref().and_then(|p| p.src.clone()) else { return };
        if !pf::token_present(&cx.services, &cx.session.user, &src.id) {
            let f = Form::new(&format!("Publish token for {}", src.name)).secret("Token", "from the store's administrator; it goes into your vault and is never shown to the assistant");
            self.modal = Some(Modal::Form(FormKind::PublishToken, f));
            return;
        }
        self.show_publish_review(cx);
    }

    /// "Add it and publish" was approved on the card with the fingerprint: add the store with that key pinned, make it the target, go on.
    fn add_store_then_publish(&mut self, cx: &AppCx) {
        let Some(p) = self.publish.as_ref().and_then(|s| s.adding.clone()) else { return };
        let store = Store::for_services(&cx.services);
        let actor = Actor::from_policy(&cx.session.user, &cx.session.policy);
        let fetch = HttpFetch(cx.services.quick_upstream());
        match privatepub::add_and_target(&store, &actor, &fetch, &p, &p.name) {
            Ok(src) => {
                cx.audit.record(&cx.session.user, "store.pin", &format!("{} {}", p.url, p.fingerprint));
                cx.audit.record(&cx.session.user, "ide.publish.addstore", &format!("{} added from Publish and chosen as the target", src.name));
                if let Some(st) = self.publish.as_mut() {
                    st.src = Some(src);
                    st.unadded = None;
                    st.adding = None;
                }
                self.token_or_review(cx);
            }
            Err(e) => {
                self.publish = None;
                let c = Card::new("The store was not added", self.accent()).danger().line(&e, Tone::Bad).line("Nothing was changed.", Tone::Dim).action(Action::new("Close", Key::Esc, MUTE));
                self.card_modal(c, CardAction::Info);
            }
        }
    }

    pub fn submit_publish_token(&mut self, f: Form, cx: &AppCx) {
        let Some(src) = self.publish.as_ref().and_then(|p| p.src.clone()) else { return };
        match pf::save_token(&cx.services, &cx.session.user, &src.id, &f.value(0)) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "ide.publish.token", &format!("stored for {}", src.name));
                self.show_publish_review(cx);
            }
            Err(e) => {
                let mut f = f;
                f.error = Some(e);
                self.modal = Some(Modal::Form(FormKind::PublishToken, f));
            }
        }
    }

    fn show_publish_review(&mut self, cx: &AppCx) {
        let Some(st) = self.publish.as_ref() else { return };
        let (Some(src), Some(key)) = (st.src.clone(), st.key.clone()) else { return };
        let fetch = HttpFetch(cx.services.quick_upstream());
        match pf::card(&cx.services, &cx.session.user, &st.dir, &src, &key, &fetch) {
            Ok((c, text)) => {
                let mut card = Card::new(&format!("Publish {} {} to {}?", c.name, c.version, c.store), self.accent());
                card = card.line(&format!("Server: {}", c.server), Tone::Bold);
                if let Some(w) = &c.http_warning {
                    card = card.line(w, Tone::Warn);
                }
                card = card.line(&format!("Package: {} {} ({}), {} bytes", c.name, c.version, c.kind, c.size), Tone::Normal);
                if let Some(v) = &c.store_has {
                    card = card.line(&format!("The store has {v} now; this is higher."), Tone::Dim);
                }
                card = card.line("Permissions", Tone::Bold);
                if c.permissions.is_empty() {
                    card = card.line("- none", Tone::Normal);
                }
                for p in &c.permissions {
                    card = card.line(&format!("- {p}"), Tone::Normal);
                }
                card = card.line(&format!("Trust level it will get: {}", c.trust), if c.new_key { Tone::Warn } else { Tone::Normal });
                card = card.line(&format!("Signed with your key {}", c.key_fingerprint), Tone::Normal);
                if let Some(w) = &c.key_warning {
                    card = card.line(w, Tone::Warn);
                }
                card = card.line("Files", Tone::Bold);
                for f in c.files.iter().take(12) {
                    card = card.line(&format!("- {f}"), Tone::Dim);
                }
                if c.files.len() > 12 {
                    card = card.line(&format!("- … {} more", c.files.len() - 12), Tone::Dim);
                }
                card = card.line("Everyone who uses that store can then install it. Your token is read from your vault; nobody else sees it.", Tone::Dim);
                card = card.action(Action::new("Approve and publish", Key::Char('y'), OKC));
                if c.new_key {
                    card = card.action(Action::new("Copy key for the store admin", Key::Char('k'), WARN));
                }
                card = card.action(Action::new("Cancel", Key::Esc, MUTE));
                if let Some(s) = self.publish.as_mut() {
                    s.text = Some(text);
                    s.card = Some(c);
                }
                self.card_modal(card, CardAction::PublishReview);
            }
            Err(e) => {
                self.publish = None;
                let c = Card::new("Not ready to publish", self.accent()).danger().line(&e, Tone::Bad).action(Action::new("Close", Key::Esc, MUTE));
                self.card_modal(c, CardAction::Info);
            }
        }
    }

    /// A button of a card was chosen.
    pub fn card_action(&mut self, cm: Box<CardModal>, key: Key, cx: &AppCx) {
        let CardModal { card, action } = *cm;
        match (action, key) {
            (CardAction::Info, _) => {}
            (CardAction::ChangeTarget(t), Key::Char('y')) => self.apply_target(t, cx),
            (CardAction::Fix(msg), Key::Char('a')) => {
                if let Some(w) = self.active_mut() {
                    w.prefs.chat_open = true;
                    w.set_focus(super::ide_layout::Pane::Chat);
                    w.compact_pane = super::ide_layout::Pane::Chat;
                    w.chat.ask(cx, &msg);
                }
            }
            (CardAction::PublishReview, Key::Char('k')) => {
                // the key line goes on the clipboard; the card stays
                if let Some(st) = &self.publish {
                    if let Some(k) = st.key.as_ref().and_then(|k| keys::list(&cx.services, &cx.session.user).into_iter().find(|i| i.public == iyakki_agent::pkg::public_key_hex(&k.0))) {
                        crate::ui::clipboard::copy(&k.admin_line());
                        self.notice = Some(("Copied: send this line to the store's administrator for trusted-keys.txt.".into(), true));
                    }
                }
                self.card_modal(card, CardAction::PublishReview);
            }
            (CardAction::AddStoreThenPublish, Key::Char('y')) => self.add_store_then_publish(cx),
            (CardAction::AddStoreThenPublish, Key::Esc) => self.publish = None,
            (CardAction::AddStoreThenPublish, _) => self.card_modal(card, CardAction::AddStoreThenPublish),
            (CardAction::PublishReview, Key::Char('y')) => self.do_publish(cx),
            (CardAction::PublishReview, Key::Esc) => self.publish = None,
            (CardAction::PublishReview, _) => self.card_modal(card, CardAction::PublishReview),
            (CardAction::PublishDone, Key::Char('i')) => self.review_install_published(cx),
            (CardAction::PublishDone, _) => self.publish = None,
            (CardAction::InstallReview(r), Key::Char('y')) => {
                let store = Store::for_services(&cx.services);
                let actor = Actor::from_policy(&cx.session.user, &cx.session.policy);
                match pf::install_reviewed(&cx.services, &store, &actor, &r) {
                    Ok(said) => {
                        cx.audit.record(&cx.session.user, "ide.publish.install", &said);
                        let (name, kind) = self.publish.as_ref().map(|p| (p.name.clone(), p.kind.clone())).unwrap_or_default();
                        self.publish = None;
                        self.notice = Some((said, true));
                        if kind == "tile" {
                            self.request = Some((format!("tile-open:{name}"), String::new()));
                        }
                    }
                    Err(e) => self.notice = Some((format!("Not installed: {e}"), false)),
                }
            }
            (CardAction::InstallReview(_), _) => {}
            (CardAction::Git(c), k) => self.git_card_action(c, k, cx),
            _ => {}
        }
        self.redraw = true;
    }

    fn do_publish(&mut self, cx: &AppCx) {
        let Some(st) = self.publish.as_ref() else { return };
        let (Some(src), key, dir) = (st.src.clone(), st.key.clone(), st.dir.clone()) else { return };
        let fetch = HttpFetch(cx.services.quick_upstream());
        let user = cx.session.user.clone();
        match pf::publish(&cx.services, &user, &dir, &src, key.as_ref(), &fetch, iyakki_agent::clock::now_unix()) {
            Ok(r) => {
                cx.audit.record(&user, "ide.publish", &format!("{} {} to {} ({})", r.name, r.version, src.name, r.trust));
                let c = Card::new("Published", self.accent())
                    .line(&format!("{} {} is listed in {} as {} (catalog number {}).", r.name, r.version, src.name, r.trust, r.serial), Tone::Good)
                    .line("People who added that store see it after they check for updates.", Tone::Normal)
                    .line("", Tone::Normal)
                    .line(pf::YANK_NOTE, Tone::Dim)
                    .action(Action::new("Install it here to test", Key::Char('i'), OKC))
                    .action(Action::new("Close", Key::Esc, MUTE));
                self.card_modal(c, CardAction::PublishDone);
            }
            Err(pf::Stop::NeedsToken) => {
                let f = Form::new(&format!("Publish token for {}", src.name)).secret("Token", "from the store's administrator; it goes into your vault and is never shown to the assistant");
                self.modal = Some(Modal::Form(FormKind::PublishToken, f));
            }
            Err(e) => {
                self.publish = None;
                let c = Card::new("Not published", self.accent()).danger().line(&e.to_string(), Tone::Bad).line("Nothing was changed in the store.", Tone::Dim).action(Action::new("Close", Key::Esc, MUTE));
                self.card_modal(c, CardAction::Info);
            }
        }
    }

    fn review_install_published(&mut self, cx: &AppCx) {
        let Some(st) = self.publish.as_ref() else { return };
        let name = st.name.clone();
        let store = Store::for_services(&cx.services);
        let actor = Actor::from_policy(&cx.session.user, &cx.session.policy);
        let fetch = HttpFetch(cx.services.quick_upstream());
        match pf::review_published(&cx.services, &store, &actor, &fetch, &name, actor.default_scope()) {
            Ok(r) => {
                let mut c = Card::new(&format!("Install {name} from the store?"), self.accent());
                for l in r.summary().lines() {
                    c = c.line(l, Tone::Normal);
                }
                if let Some(why) = &r.refusal {
                    c = c.line(why, Tone::Bad).action(Action::new("Close", Key::Esc, MUTE));
                } else {
                    c = c.action(Action::new("Install", Key::Char('y'), OKC)).action(Action::new("Cancel", Key::Esc, MUTE));
                }
                self.card_modal(c, CardAction::InstallReview(Box::new(r)));
            }
            Err(e) => {
                self.publish = None;
                self.notice = Some((e, false));
            }
        }
    }

    /// The header's line about publishing: `Published: 1.0.2 → store` and the hint when the workspace is ahead or behind.
    pub fn publish_hint(&self) -> Option<String> {
        pf::header_hint(&self.project_dir()?)
    }

    /// Run the headless tests for the tests button of a tile workspace: the card with each test.
    pub fn tile_tests_card(&mut self) -> bool {
        if self.workspace_target() != Some(AppTarget::IyakkiTile) {
            return false;
        }
        let Some(dir) = self.project_dir() else { return false };
        let mut c = Card::new("Tile app tests (headless)", self.accent());
        match tile::run_tests(&dir) {
            Ok(r) => {
                for l in r.text().lines() {
                    c = c.line(l, if l.starts_with("FAIL") { Tone::Bad } else if l.starts_with("ok") { Tone::Good } else { Tone::Normal });
                }
            }
            Err(e) => c = c.line(&e, Tone::Bad),
        }
        self.card_modal(c.action(Action::new("Close", Key::Esc, CYAN)), CardAction::Info);
        true
    }
}

/// Reload the project file into the in-memory copy, keeping what was there if it cannot be read.
struct ProjectConfigExt;
impl ProjectConfigExt {
    fn reload(root: &std::path::Path, old: iyakki_agent::workspace::ProjectConfig) -> iyakki_agent::workspace::ProjectConfig {
        iyakki_agent::workspace::ProjectConfig::load(root).unwrap_or(old)
    }
}

/// A deploy target of developer mode in the Publish dialog.
pub enum DevTarget {
    /// This device (developer mode on): installed here as Developer (local).
    ThisDevice,
    /// A device this account paired with (`iyakki dev pair`, the account's own folder of developer mode on this device).
    Paired(dm::client::Device),
}

impl DevTarget {
    fn label(&self) -> String {
        match self {
            DevTarget::ThisDevice => "This device (developer mode)".into(),
            DevTarget::Paired(d) => format!("Paired device {} · {}:{} (developer mode)", d.name, d.host, d.port),
        }
    }
}

/// Where this account keeps its own developer-mode client key and paired devices on this device.
pub fn dev_home(cx: &AppCx) -> dm::client::Home {
    dm::client::Home::at(&cx.services.dir().join("devmode").join("client").join(&cx.session.user))
}

/// The developer-mode targets this account has: this device while developer mode is on (for those who may use the STORE), and its
/// paired devices.
pub fn dev_targets(cx: &AppCx) -> Vec<DevTarget> {
    let mut v = Vec::new();
    if dm::is_on(cx.services.dir()) && Actor::from_policy(&cx.session.user, &cx.session.policy).allowed {
        v.push(DevTarget::ThisDevice);
    }
    v.extend(dev_home(cx).devices().into_iter().map(DevTarget::Paired));
    v
}

/// Pack the project (signed with the chosen key) and install it on the target through developer mode. Returns (name, kind, summary).
pub fn deploy_dev(cx: &AppCx, t: &DevTarget, dir: &std::path::Path, key: &keys::SigningKey) -> Result<(String, String, String), String> {
    let bytes = dm::ops::package_bytes(dir, key)?;
    match t {
        DevTarget::ThisDevice => {
            let d = dm::DevMode::open(cx.services.dir());
            if !d.is_on() {
                return Err("Developer mode is off on this device: an administrator switches it on in Settings, Developer mode.".into());
            }
            let store = Store::for_services(&cx.services);
            let actor = Actor::from_policy(&cx.session.user, &cx.session.policy);
            let who = dm::ops::Who { computer: "this-device-ide".into(), account: cx.session.user.clone() };
            let r = dm::ops::install(&cx.services, &store, &d, &actor, &who, bytes, false)?;
            Ok((r.name.clone(), r.kind.clone(), format!("{}:{} {} installed on this device for {}", r.kind, r.name, r.version, r.scope.trim_start_matches("user/"))))
        }
        DevTarget::Paired(dev) => {
            let c = dm::client::DevClient::new(&dev_home(cx), dev.clone())?;
            let v = c.install(&bytes, false)?;
            let (name, kind) = (v["name"].as_str().unwrap_or("").to_string(), v["kind"].as_str().unwrap_or("").to_string());
            if kind == "tile" {
                let _ = c.open(&name);
            }
            Ok((name.clone(), kind.clone(), format!("{kind}:{name} {} installed on {} ({}:{})", v["version"].as_str().unwrap_or(""), dev.name, dev.host, dev.port)))
        }
    }
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

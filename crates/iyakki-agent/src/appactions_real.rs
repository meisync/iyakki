//! `AppActions` on this machine (F-SDK-201): what an app's event handler may do, through the core. A note goes to the person's inbox under
//! the app's name; a flow starts by its name in the person's own flows engine (the engine's approval nodes still ask); a tool request waits in
//! the person's list until they answer a card (`ActionBook`: nothing runs here). Every action needs the capability the app holds
//! (`AgentScopes`: the built-in table or the person's approval record) and is logged by names, never the text of a note or an argument.

use crate::appevent_sources::AgentScopes;
use crate::services::Services;
use iyakki_core_api::appactions::{clean_note, ActionBook, AppActions, PendingAction};
use iyakki_core_api::appevents::ScopeCheck;
use std::sync::{Arc, Mutex};

pub struct AgentActions {
    services: Arc<Services>,
    scopes: AgentScopes,
    book: ActionBook,
    file: std::path::PathBuf,
    audit: Mutex<Vec<String>>,
}

impl AgentActions {
    pub fn new(services: Arc<Services>) -> AgentActions {
        let scopes = AgentScopes { dir: services.dir().to_path_buf() };
        let file = services.dir().join("appactions.json");
        let book = ActionBook::import(&std::fs::read_to_string(&file).unwrap_or_default(), now_ms());
        AgentActions { services, scopes, book, file, audit: Mutex::new(Vec::new()) }
    }

    /// Keep the waiting list (written through a temporary file; a failure is not an error of the request).
    fn save(&self) {
        let tmp = self.file.with_extension("tmp");
        if std::fs::write(&tmp, self.book.export()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }

    fn log(&self, user: &str, line: String) {
        self.services.browser.audit(user, "appactions", &line);
        let mut a = self.audit.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        a.push(line);
        if a.len() > 200 {
            a.remove(0);
        }
    }

    fn need(&self, user: &str, app: &str, cap: &str) -> Result<(), String> {
        if self.scopes.holds(user, app, cap) {
            Ok(())
        } else {
            self.log(user, format!("refuse {app}: {cap}"));
            Err(format!("{app} does not hold {cap}: the person grants it first"))
        }
    }
}

fn now_ms() -> u64 {
    (crate::clock::now_unix().max(0) as u64) * 1000
}

impl AppActions for AgentActions {
    fn notify(&self, user: &str, app: &str, text: &str) -> Result<(), String> {
        self.need(user, app, "notify")?;
        let t = clean_note(text)?;
        self.services.inbox.add(user, crate::inbox::Kind::Info, app, &t, app, None, crate::clock::now_unix())?;
        self.log(user, format!("notify {app}"));
        Ok(())
    }

    fn start_flow(&self, user: &str, app: &str, name: &str) -> Result<String, String> {
        self.need(user, app, "flows.start")?;
        let env = crate::flows::FlowEnv::existing(&self.services, user).ok_or("no flows engine is running for this person (it opens when they sign in)")?;
        let f = env.summaries().into_iter().find(|f| f.name == name && f.published > 0).ok_or_else(|| format!("there is no published flow called {name}"))?;
        env.engine.start_run(iyakki_flow::engine::StartRequest::new(&f.id).trigger("app", app)).map_err(|e| e.to_string())?;
        self.log(user, format!("flow {app}: {name}"));
        Ok(format!("started {name}"))
    }

    fn request_tool(&self, user: &str, app: &str, tool: &str, args: &str) -> Result<String, String> {
        self.need(user, app, "tools.request")?;
        let id = self.book.ask(user, app, tool, args, now_ms())?;
        self.save();
        self.log(user, format!("request {app}: {tool} {id}"));
        Ok(id)
    }

    fn pending(&self, user: &str) -> Vec<PendingAction> {
        self.book.pending(user, now_ms())
    }

    fn take(&self, user: &str, id: &str) -> Option<PendingAction> {
        let p = self.book.take(user, id)?;
        self.save();
        self.log(user, format!("answer {}: {}", p.app, p.tool));
        Some(p)
    }

    fn audit(&self) -> Vec<String> {
        self.audit.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

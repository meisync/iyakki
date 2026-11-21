//! The shell's side of a client's display-list mode (F-SDK-29, D-CLIENT-PROTO-DISPLAYLIST): a client that negotiated `display.list` asks for an
//! SDK app by name, the shell makes that app's own tile for the signed-in person (never the tile on the shell's screen), hands the client its
//! frames as display list text and applies the client's input lines. Everything goes through the same checks as opening the app on the machine:
//! the person's role (`allows_app`), the cloud mode's hidden apps and the audit log. An app that is not written against the UI SDK is never
//! made here: the set is fixed (`LIST_APPS`).

use super::*;

/// The apps a client may open in display-list mode: the SDK apps of the shell (their tiles implement `App::list_frame` and `App::list_apply`).
pub const LIST_APPS: [PApp; 2] = [PApp::Sample, PApp::Markdown];

/// Why a request of display-list mode was refused; `code` is the stable word of the client protocol's `app_err`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListError {
    pub code: &'static str,
    pub message: String,
}

fn refuse<T>(code: &'static str, message: &str) -> Result<T, ListError> {
    Err(ListError { code, message: message.into() })
}

impl Shell {
    /// Make the SDK app `id` for this person at a pane of `w` by `h` logical units and return its first frame. One app at a time: a second
    /// open replaces the first (the first is closed and audited).
    pub fn list_app_open(&mut self, id: &str, w: u32, h: u32) -> Result<String, ListError> {
        let Some(cx) = self.cx() else { return refuse("denied", "sign in first") };
        let Some(app) = LIST_APPS.iter().copied().find(|a| a.id() == id) else { return refuse("not_found", "that app cannot be shown in a client") };
        if self.cloud.hides_app(app) || !cx.session.policy.allows_app(app) {
            self.env.audit.record(&cx.session.user, "client.app.denied", app.id());
            return refuse("denied", &format!("your role ({}) cannot open {}", cx.session.role.name(), app.id().to_uppercase()));
        }
        if !(100..=4096).contains(&w) || !(100..=4096).contains(&h) {
            return refuse("bad_request", "the pane must be 100 to 4096 units each way");
        }
        self.list_app_close();
        let mut cx = cx;
        cx.os = self.osguard.os_for(app.id(), &cx.session.user);
        let mut tile: Box<dyn App> = match app {
            PApp::Sample => Box::new(crate::apps::sample::SampleTile::new()),
            _ => Box::new(crate::apps::markdown::MarkdownTile::new()),
        };
        let Some(first) = tile.list_frame(w as f32, h as f32, &cx) else { return refuse("unsupported", "that app has no display list") };
        self.env.audit.record(&cx.session.user, "client.app.open", app.id());
        self.list_app = Some((tile, app));
        Ok(first)
    }

    /// Apply a batch of input lines to the open app: the next frame and whether the person left the app (it is closed then).
    pub fn list_app_input(&mut self, batch: &str) -> Result<(String, bool), ListError> {
        let Some(cx) = self.cx() else { return refuse("denied", "sign in first") };
        let Some((tile, app)) = self.list_app.as_mut() else { return refuse("not_open", "no app is open") };
        let mut cx = cx;
        cx.os = self.osguard.os_for(app.id(), &cx.session.user);
        let Some(r) = tile.list_apply(batch, &cx) else { return refuse("unsupported", "that app has no display list") };
        match r {
            Ok((frame, left)) => {
                if left {
                    self.list_app_close();
                }
                Ok((frame, left))
            }
            Err(e) => Err(ListError { code: "bad_request", message: e }),
        }
    }

    /// Close the open app, if any. True when there was one.
    pub fn list_app_close(&mut self) -> bool {
        let Some((_, app)) = self.list_app.take() else { return false };
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        self.env.audit.record(&user, "client.app.close", app.id());
        true
    }

    /// The id of the app open in display-list mode.
    pub fn list_app_id(&self) -> Option<&'static str> {
        self.list_app.as_ref().map(|(_, a)| a.id())
    }
}

// ------------------------------------------------------------------------------------------------------------------ files for a client (F-CLIENT-01)

impl Shell {
    fn client_user(&self) -> Result<String, String> {
        self.session.as_ref().map(|s| s.user.clone()).ok_or_else(|| "sign in first".to_string())
    }

    /// A folder of the signed-in person's home, through the core's files facet (the checks of the facet apply: nothing outside the home, no links, no hidden names).
    pub fn client_files_list(&mut self, dir: &str) -> Result<iyakki_core_api::Listing, String> {
        let user = self.client_user()?;
        self.core.files().list(&user, dir)
    }

    /// The bytes of one file of the home (the facet's limit is 8 MB), audited by name.
    pub fn client_files_read(&mut self, path: &str) -> Result<Vec<u8>, String> {
        let user = self.client_user()?;
        let b = self.core.files().read_bytes(&user, path, iyakki_core_api::files::MAX_UPLOAD)?;
        self.env.audit.record(&user, "client.file.read", &format!("{} ({} bytes)", path, b.len()));
        Ok(b)
    }

    /// Save a file a client sent, through the core's change book: the facet asks (`change`) and the answer is the person's yes on the machine's
    /// card that came before (`decide`). Both steps are audited by the core.
    pub fn client_files_save(&mut self, path: &str, bytes: Vec<u8>) -> Result<String, String> {
        let user = self.client_user()?;
        let ask = self.core.files().change(&user, &iyakki_core_api::FileChange::SaveBytes { path: path.to_string(), bytes })?;
        self.core.files().decide(&user, &ask.call, true)
    }
}

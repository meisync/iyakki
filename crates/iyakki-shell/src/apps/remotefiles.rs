//! REMOTE FILES: the tile itself lives in the `iyakki-tile-remotefiles` crate (it does not depend on the agent). This file is the shell's
//! side: the person's `remotefiles::Manager` (sessions, host key pins, transfer queue; shared with the assistant's tools) behind the
//! tile's `RemoteSource`, the conversion between the agent's types and the facade's, the person's vault and file rules, and the adapter
//! that makes the tile crate's `App` an `App` of the shell.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent::remotefiles as agent;
use iyakki_tile_api::remote as api;
use iyakki_tile_api::{App as TileApp, TileCx};

use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub const ACCENT: [u8; 4] = iyakki_tile_remotefiles::ACCENT;

/// The person's manager: their vault for passwords and keys, their file rules for the paths on this computer.
pub fn manager_for(cx: &AppCx) -> Arc<agent::Manager> {
    let (user, policy, data) = (cx.session.user.clone(), cx.session.policy.clone(), cx.services.dir().to_path_buf());
    let home = iyakki_agent::scope::FileRoots::new(&policy, &user, &data).home;
    let downloads = home.join("Downloads");
    let _ = std::fs::create_dir_all(&downloads);
    let secrets = Arc::new(agent::VaultSecrets { svc: cx.services.clone(), user: user.clone() });
    let (p2, u2, d2) = (policy.clone(), user.clone(), data.clone());
    let local: agent::LocalCheck = Arc::new(move |text: &str, write: bool| {
        let roots = iyakki_agent::scope::FileRoots::new(&p2, &u2, &d2);
        let expanded = PathBuf::from(roots.expand(text));
        let path = if expanded.is_absolute() { expanded } else { roots.home.join(expanded) };
        if write {
            let name = path.file_name().ok_or("that path has no file name")?.to_owned();
            let parent = std::fs::canonicalize(path.parent().ok_or("that path has no folder")?).map_err(|_| "that folder does not exist or is not allowed".to_string())?;
            let real = parent.join(name);
            if roots.contains(&parent) && roots.write_ok(&p2, &u2, &real) { Ok(real) } else { Err("outside the folders your account may write to".to_string()) }
        } else {
            let real = std::fs::canonicalize(&path).map_err(|_| "not found, or not allowed".to_string())?;
            if roots.read_ok(&p2, &u2, &real) { Ok(real) } else { Err("outside the folders your account may read".to_string()) }
        }
    });
    let audit = cx.audit.clone();
    let who = user.clone();
    agent::shared(&data, &user, secrets, local, Arc::new(move |e, d| audit.record(&who, e, d)), &downloads.to_string_lossy())
}

/// The manager behind the tile's source trait.
pub struct ShellRemote(pub Arc<agent::Manager>);

fn proto_to_api(p: agent::Proto) -> api::Proto {
    match p {
        agent::Proto::Sftp => api::Proto::Sftp,
        agent::Proto::Ftps => api::Proto::Ftps,
        agent::Proto::Ftp => api::Proto::Ftp,
    }
}
fn proto_from_api(p: api::Proto) -> agent::Proto {
    match p {
        api::Proto::Sftp => agent::Proto::Sftp,
        api::Proto::Ftps => agent::Proto::Ftps,
        api::Proto::Ftp => agent::Proto::Ftp,
    }
}

fn conn_to_api(c: agent::Conn) -> api::Conn {
    api::Conn { name: c.name, host: c.host, port: c.port, user: c.user, proto: proto_to_api(c.proto), auth: if c.auth == agent::Auth::Key { api::Auth::Key } else { api::Auth::Password }, key: c.key, plain_ok: c.plain_ok, has_password: c.has_password }
}

fn conn_from_api(c: &api::Conn) -> agent::Conn {
    agent::Conn { name: c.name.clone(), host: c.host.clone(), port: c.port, user: c.user.clone(), proto: proto_from_api(c.proto), auth: if c.auth == api::Auth::Key { agent::Auth::Key } else { agent::Auth::Password }, key: c.key.clone(), plain_ok: c.plain_ok, has_password: false }
}

fn entry_to_api(e: agent::Entry) -> api::Entry {
    api::Entry { name: e.name, is_dir: e.is_dir, is_link: e.is_link, size: e.size, modified: e.modified, perms: e.perms }
}

fn card_to_api(c: agent::HostCard) -> api::HostCard {
    api::HostCard { host: c.host, port: c.port, kind: c.kind, fingerprint: c.fingerprint, pinned: c.pinned }
}

fn result_to_api(r: agent::JobResult) -> api::JobResult {
    match r {
        agent::JobResult::Opened(agent::Opened::Ready(id)) => api::JobResult::Opened(api::Opened::Ready(id)),
        agent::JobResult::Opened(agent::Opened::Unknown(c)) => api::JobResult::Opened(api::Opened::Unknown(card_to_api(c))),
        agent::JobResult::Opened(agent::Opened::Changed(c)) => api::JobResult::Opened(api::Opened::Changed(card_to_api(c))),
        agent::JobResult::Listing(p, e) => api::JobResult::Listing(p, e.into_iter().map(entry_to_api).collect()),
        agent::JobResult::Done(m) => api::JobResult::Done(m),
        agent::JobResult::Failed(m) => api::JobResult::Failed(m),
    }
}

fn job_from_api(j: api::Job) -> agent::Job {
    match j {
        api::Job::Open(n) => agent::Job::Open(n),
        api::Job::List(s, p) => agent::Job::List(s, p),
        api::Job::Mkdir(s, p) => agent::Job::Mkdir(s, p),
        api::Job::Rename(s, a, b) => agent::Job::Rename(s, a, b),
        api::Job::Delete(s, p, d) => agent::Job::Delete(s, p, d),
    }
}

fn info_to_api(t: agent::TransferInfo) -> api::TransferInfo {
    let state = match t.state {
        agent::State::Queued => api::State::Queued,
        agent::State::Running => api::State::Running,
        agent::State::Done => api::State::Done,
        agent::State::Failed => api::State::Failed,
        agent::State::Cancelled => api::State::Cancelled,
    };
    api::TransferInfo { id: t.id, download: t.download, remote: t.remote, local: t.local, done: t.done, total: t.total, state, note: t.note, rate: t.rate, resumed: t.resumed }
}

impl api::RemoteSource for ShellRemote {
    fn connections(&self) -> Vec<api::Conn> {
        self.0.connections().into_iter().map(conn_to_api).collect()
    }
    fn save(&self, conn: &api::Conn, password: Option<&str>, replacing: Option<&str>) -> Result<(), String> {
        self.0.save(&conn_from_api(conn), password, replacing)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.remove(name)
    }
    fn key_names(&self) -> Vec<String> {
        self.0.key_names()
    }
    fn submit(&self, job: api::Job) -> u64 {
        self.0.submit(job_from_api(job))
    }
    fn take(&self, ticket: u64) -> Option<api::JobResult> {
        self.0.take(ticket).map(result_to_api)
    }
    fn trust(&self, name: &str, fingerprint: &str) -> Result<(), String> {
        self.0.trust(name, fingerprint)
    }
    fn forget_host(&self, name: &str) -> Result<(), String> {
        self.0.forget_host(name)
    }
    fn close(&self, session: u64) {
        self.0.close(session)
    }
    fn start_transfer(&self, req: &api::TransferReq) -> Result<u64, String> {
        self.0.start_transfer(&agent::TransferReq { session: req.session, download: req.download, remote: req.remote.clone(), local: req.local.clone(), overwrite: req.overwrite })
    }
    fn transfers(&self) -> Vec<api::TransferInfo> {
        self.0.transfers().into_iter().map(info_to_api).collect()
    }
    fn cancel_transfer(&self, id: u64) {
        self.0.cancel_transfer(id)
    }
    fn retry_transfer(&self, id: u64) -> Result<u64, String> {
        self.0.retry_transfer(id)
    }
    fn clear_finished(&self) {
        self.0.clear_finished()
    }
    fn download_dir(&self) -> String {
        self.0.download_dir()
    }
}

/// The tile crate's app, as an app of the shell.
pub struct RemoteFilesApp(iyakki_tile_remotefiles::RemoteFilesApp);

impl RemoteFilesApp {
    pub fn new(_cx: &AppCx) -> RemoteFilesApp {
        RemoteFilesApp(iyakki_tile_remotefiles::RemoteFilesApp::new())
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        let src = ShellRemote(manager_for(cx));
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &super::usage::ShellUsage(&cx.services), calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &src, cameras: &iyakki_tile_api::cameras::NoCameras })
    }
}

impl App for RemoteFilesApp {
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match Self::with(cx, |t| self.0.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with(cx, |t| self.0.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
#[path = "remotefiles_tests.rs"]
mod tests;

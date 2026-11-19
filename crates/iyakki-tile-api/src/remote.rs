//! The remote-files facade: what the REMOTE FILES tile (an SFTP / FTPS client) asks of the shell. Plain data and one source trait; the
//! agent's `remotefiles` module has the real thing (sessions, host key pins, the vault, the transfer queue) and the shell converts nothing
//! (the agent implements this trait through a thin adapter). Calls that touch the network never block the tile: `submit` returns a ticket
//! and the tile polls `take` on every tick.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Proto {
    #[default]
    Sftp,
    /// FTP over explicit TLS (`AUTH TLS`).
    Ftps,
    /// Plain FTP: nothing is encrypted. Only behind an explicit warning toggle on the connection.
    Ftp,
}

impl Proto {
    pub const ALL: [Proto; 3] = [Proto::Sftp, Proto::Ftps, Proto::Ftp];
    pub fn label(self) -> &'static str {
        match self {
            Proto::Sftp => "SFTP",
            Proto::Ftps => "FTPS",
            Proto::Ftp => "FTP (plain)",
        }
    }
    pub fn default_port(self) -> u16 {
        match self {
            Proto::Sftp => 22,
            Proto::Ftps | Proto::Ftp => 21,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Auth {
    #[default]
    Password,
    /// A private key kept in the person's vault under `Conn::key`.
    Key,
}

/// A saved connection. No secret is in it: a password is in the vault under a name the source derives from `name`; a key is named in `key`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Conn {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub proto: Proto,
    pub auth: Auth,
    /// The vault name of the private key (`ssh.<x>`), for `Auth::Key`.
    pub key: String,
    /// The person has accepted that plain FTP sends the password and the files unencrypted (only read for `Proto::Ftp`).
    pub plain_ok: bool,
    /// A password is in the vault.
    pub has_password: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    /// Unix seconds, 0 when the server did not say.
    pub modified: i64,
    /// `rwxr-xr-x` or empty.
    pub perms: String,
}

/// A server's host key (SFTP) or certificate (FTPS) the person has to compare and confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCard {
    pub host: String,
    pub port: u16,
    /// `ssh-ed25519` or `tls-certificate`.
    pub kind: String,
    /// `SHA256:...`
    pub fingerprint: String,
    /// The fingerprint pinned earlier when it differs from the one now presented: the server changed or someone sits in between.
    pub pinned: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    Ready(u64),
    /// First contact: show the card; on the person's confirmation call `trust` and open again.
    Unknown(HostCard),
    /// The key is not the pinned one. Refused; there is no accept button, only a deliberate `forget_host` first.
    Changed(HostCard),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Open(String),
    List(u64, String),
    Mkdir(u64, String),
    Rename(u64, String, String),
    /// session, path, is a folder (an empty folder only)
    Delete(u64, String, bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobResult {
    Opened(Opened),
    /// The canonical path and its entries (unsorted).
    Listing(String, Vec<Entry>),
    Done(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferReq {
    pub session: u64,
    pub download: bool,
    pub remote: String,
    /// A path in the person's file scope: the folder or file name on this computer.
    pub local: String,
    /// Replace an existing target.
    pub overwrite: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn finished(self) -> bool {
        matches!(self, State::Done | State::Failed | State::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferInfo {
    pub id: u64,
    pub download: bool,
    pub remote: String,
    pub local: String,
    pub done: u64,
    /// 0 when unknown.
    pub total: u64,
    pub state: State,
    pub note: String,
    /// Bytes per second, a short average.
    pub rate: u64,
    /// The transfer continued a partial file.
    pub resumed: bool,
}

/// What the tile uses. Implemented for the signed-in person (the agent keeps one manager per person, shared with the chat tools).
pub trait RemoteSource {
    fn connections(&self) -> Vec<Conn>;
    /// Adds a connection, or (with `replacing`) changes the one called that. `password` goes to the vault and nowhere else (None keeps the
    /// stored one, Some("") removes it).
    fn save(&self, conn: &Conn, password: Option<&str>, replacing: Option<&str>) -> Result<(), String>;
    fn remove(&self, name: &str) -> Result<(), String>;
    /// The person's vault keys that can be used for `Auth::Key` (names only).
    fn key_names(&self) -> Vec<String>;
    fn submit(&self, job: Job) -> u64;
    fn take(&self, ticket: u64) -> Option<JobResult>;
    /// The person compared the card and confirmed it: pin this fingerprint for the connection (refused if the server now presents another).
    fn trust(&self, name: &str, fingerprint: &str) -> Result<(), String>;
    /// Drop the pin of a connection (after the person understood why the key changed).
    fn forget_host(&self, name: &str) -> Result<(), String>;
    fn close(&self, session: u64);
    fn start_transfer(&self, req: &TransferReq) -> Result<u64, String>;
    fn transfers(&self) -> Vec<TransferInfo>;
    fn cancel_transfer(&self, id: u64);
    /// Continue a failed or cancelled transfer from the bytes already there.
    fn retry_transfer(&self, id: u64) -> Result<u64, String>;
    fn clear_finished(&self);
    /// The folder on this computer that downloads go to unless the person picks another.
    fn download_dir(&self) -> String;
}

/// A source with nothing in it (the tile shows an empty list, and every call fails politely).
pub struct NoRemote;

impl RemoteSource for NoRemote {
    fn connections(&self) -> Vec<Conn> {
        Vec::new()
    }
    fn save(&self, _: &Conn, _: Option<&str>, _: Option<&str>) -> Result<(), String> {
        Err("remote files are not available here".into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err("remote files are not available here".into())
    }
    fn key_names(&self) -> Vec<String> {
        Vec::new()
    }
    fn submit(&self, _: Job) -> u64 {
        0
    }
    fn take(&self, _: u64) -> Option<JobResult> {
        Some(JobResult::Failed("remote files are not available here".into()))
    }
    fn trust(&self, _: &str, _: &str) -> Result<(), String> {
        Err("remote files are not available here".into())
    }
    fn forget_host(&self, _: &str) -> Result<(), String> {
        Err("remote files are not available here".into())
    }
    fn close(&self, _: u64) {}
    fn start_transfer(&self, _: &TransferReq) -> Result<u64, String> {
        Err("remote files are not available here".into())
    }
    fn transfers(&self) -> Vec<TransferInfo> {
        Vec::new()
    }
    fn cancel_transfer(&self, _: u64) {}
    fn retry_transfer(&self, _: u64) -> Result<u64, String> {
        Err("remote files are not available here".into())
    }
    fn clear_finished(&self) {}
    fn download_dir(&self) -> String {
        String::new()
    }
}

/// `/a/b/../c` to `/a/c`; never above `/`. Used by the tile for the breadcrumb and by the agent for path joins.
pub fn clean_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// A folder path plus a child name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// The parent of a path (`/` stays `/`).
pub fn parent(p: &str) -> String {
    let c = clean_path(p);
    match c.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => c[..i].to_string(),
    }
}

/// A file name from a server that is safe to use as the name of a local file: no folders, no control characters, not `.` or `..`.
pub fn safe_name(name: &str) -> Option<String> {
    let n: String = name.chars().map(|c| if c.is_control() || c == '/' || c == '\\' { '_' } else { c }).collect();
    let n = n.trim().to_string();
    if n.is_empty() || n == "." || n == ".." || n.len() > 255 {
        return None;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_cleaned_joined_and_never_climb_above_the_root() {
        assert_eq!(clean_path("/a/./b//c/../d"), "/a/b/d");
        assert_eq!(clean_path("/../../x"), "/x");
        assert_eq!(clean_path(""), "/");
        assert_eq!((parent("/a/b"), parent("/a"), parent("/")), ("/a".into(), "/".into(), "/".into()));
        assert_eq!((join("/", "x"), join("/a", "x"), join("/a/", "x")), ("/x".into(), "/a/x".into(), "/a/x".into()));
    }

    #[test]
    fn a_name_from_a_server_cannot_become_a_path() {
        assert_eq!(safe_name("a.txt").as_deref(), Some("a.txt"));
        assert_eq!(safe_name("../../etc/passwd").as_deref(), Some(".._.._etc_passwd"));
        assert_eq!(safe_name("a\\b\nc").as_deref(), Some("a_b_c"));
        assert_eq!((safe_name(".."), safe_name("."), safe_name("  "), safe_name(&"x".repeat(300))), (None, None, None, None));
    }
}

//! The person's messaging accounts and room names. One folder per person (`messaging/<user>/`): an account belongs to exactly one
//! person and nobody else, administrators included, reads it through this store. `accounts.json` holds no secret; the access token is in
//! the person's vault as [`vault_name`]. `state.json` holds the sync position of each account (not a secret).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::gitkeys::{AuditFn, SecretStore};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    /// `matrix`.
    pub kind: String,
    pub label: String,
    /// As the person typed it: `https://matrix.example.org` or `cluster://iyakki-matrix/matrix:8008`.
    pub homeserver: String,
    /// `@name:server`, filled in by the sign-in.
    pub user_id: String,
    #[serde(default)]
    pub device_id: String,
    /// Largest picture to send, bytes (0: the default).
    #[serde(default)]
    pub max_attachment: u64,
    pub created: i64,
}

/// A room the person named: the assistant, the flows and the test-send reach rooms only by these names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomRef {
    pub account: String,
    pub name: String,
    pub room_id: String,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    accounts: Vec<Account>,
    #[serde(default)]
    rooms: Vec<RoomRef>,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    since: std::collections::BTreeMap<String, String>,
}

pub fn vault_name(id: &str) -> String {
    format!("msg.acct.{id}.token")
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && crate::vault::valid_secret_name(&vault_name(id)) && !id.contains('.')
}

pub fn slug(label: &str) -> String {
    let mut s: String = label.trim().to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    if s.is_empty() { "matrix".into() } else { s.chars().take(24).collect() }
}

/// A room name: short, no control characters.
pub fn valid_room_name(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty() && n.chars().count() <= 40 && !n.chars().any(|c| c.is_control())
}

/// `!abc:server`: what the Matrix API calls a room id.
pub fn valid_room_id(id: &str) -> bool {
    id.len() >= 3 && id.len() <= 255 && id.starts_with('!') && id.contains(':') && !id.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '?' | '#' | '\\' | '"' | '\''))
}

#[derive(Clone)]
pub struct Accounts {
    dir: PathBuf,
    store: Arc<dyn SecretStore>,
    audit: AuditFn,
}

impl Accounts {
    pub fn new(dir: &Path, store: Arc<dyn SecretStore>, audit: AuditFn) -> Accounts {
        Accounts { dir: dir.into(), store, audit }
    }

    /// The folder of one person under the data folder.
    pub fn dir_for(data: &Path, user: &str) -> PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        data.join("messaging").join(safe)
    }

    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Accounts {
        Accounts::new(&Accounts::dir_for(svc.dir(), user), Arc::new(crate::gitkeys::ServicesStore::new(svc, user)), audit)
    }

    /// A key that tells one person's folder from another's.
    pub fn dir_key(&self) -> String {
        self.dir.display().to_string()
    }

    fn file_path(&self) -> PathBuf {
        self.dir.join("accounts.json")
    }
    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn audit(&self, action: &str, detail: &str) {
        (self.audit)(&format!("msg.{action}"), detail);
    }

    fn load(&self) -> File {
        std::fs::read(self.file_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write(&self, path: &Path, bytes: Vec<u8>) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    fn save(&self, f: &File) -> Result<(), String> {
        self.write(&self.file_path(), serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?)
    }

    pub fn list(&self) -> Vec<Account> {
        self.load().accounts
    }

    pub fn get(&self, id: &str) -> Option<Account> {
        self.load().accounts.into_iter().find(|a| a.id == id)
    }

    /// The only account, or the one called `id` (empty `id` with several accounts is an error with a plain sentence).
    pub fn pick(&self, id: &str) -> Result<Account, String> {
        let all = self.list();
        if id.trim().is_empty() {
            return match all.len() {
                0 => Err("no messaging account is set up (Settings, Notifications, Matrix account)".into()),
                1 => Ok(all[0].clone()),
                _ => Err("there are several messaging accounts: say which one".into()),
            };
        }
        all.into_iter().find(|a| a.id == id || a.label.eq_ignore_ascii_case(id)).ok_or_else(|| format!("there is no messaging account called `{id}`"))
    }

    /// Add an account and keep its token in the vault. The token is written first: a locked vault stops here and leaves no account behind.
    pub fn add(&self, mut a: Account, token: &str) -> Result<Account, String> {
        if a.label.trim().is_empty() || a.label.chars().count() > 40 || a.label.chars().any(|c| c.is_control()) {
            return Err("give the account a short name".into());
        }
        let mut f = self.load();
        let base = slug(&a.label);
        let mut id = base.clone();
        let mut n = 2;
        while f.accounts.iter().any(|x| x.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        if !valid_id(&id) {
            return Err("that name cannot be used for an account".into());
        }
        a.id = id.clone();
        self.store.set(&vault_name(&id), token)?;
        f.accounts.push(a.clone());
        if let Err(e) = self.save(&f) {
            let _ = self.store.delete(&vault_name(&id));
            return Err(e);
        }
        Ok(a)
    }

    pub fn update(&self, a: &Account) -> Result<(), String> {
        let mut f = self.load();
        match f.accounts.iter_mut().find(|x| x.id == a.id) {
            Some(x) => *x = a.clone(),
            None => return Err("no such account".into()),
        }
        self.save(&f)
    }

    /// The token, for one call. The caller keeps it only as long as it needs it.
    pub fn token(&self, id: &str) -> Result<Zeroizing<String>, String> {
        self.store.get(&vault_name(id)).map_err(|e| if e.contains("locked") { "your vault is locked: sign in again, then try again".to_string() } else { format!("the token of this account is not in your vault ({e})") })
    }

    pub fn set_token(&self, id: &str, token: &str) -> Result<(), String> {
        self.store.set(&vault_name(id), token)
    }

    /// Remove the account, its rooms, its sync position and its token.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut f = self.load();
        let before = f.accounts.len();
        f.accounts.retain(|a| a.id != id);
        if f.accounts.len() == before {
            return Err("no such account".into());
        }
        f.rooms.retain(|r| r.account != id);
        self.save(&f)?;
        let mut s = self.read_state();
        s.since.remove(id);
        let _ = self.write(&self.state_path(), serde_json::to_vec(&s).unwrap_or_default());
        self.store.delete(&vault_name(id))
    }

    pub fn rooms(&self, account: &str) -> Vec<RoomRef> {
        self.load().rooms.into_iter().filter(|r| r.account == account).collect()
    }

    pub fn add_room(&self, account: &str, name: &str, room_id: &str) -> Result<RoomRef, String> {
        if !valid_room_name(name) {
            return Err("give the room a short name".into());
        }
        if !valid_room_id(room_id) {
            return Err("a Matrix room id looks like !abcdef:server.org (Element: room settings, Advanced)".into());
        }
        let mut f = self.load();
        if !f.accounts.iter().any(|a| a.id == account) {
            return Err("no such account".into());
        }
        f.rooms.retain(|r| !(r.account == account && r.name.eq_ignore_ascii_case(name.trim())));
        if f.rooms.iter().filter(|r| r.account == account).count() >= 50 {
            return Err("at most 50 rooms per account".into());
        }
        let r = RoomRef { account: account.into(), name: name.trim().into(), room_id: room_id.trim().into() };
        f.rooms.push(r.clone());
        self.save(&f)?;
        Ok(r)
    }

    pub fn remove_room(&self, account: &str, name: &str) -> Result<(), String> {
        let mut f = self.load();
        let n = f.rooms.len();
        f.rooms.retain(|r| !(r.account == account && r.name.eq_ignore_ascii_case(name.trim())));
        if f.rooms.len() == n {
            return Err("no such room".into());
        }
        self.save(&f)
    }

    fn read_state(&self) -> State {
        std::fs::read(self.state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn since(&self, account: &str) -> Option<String> {
        self.read_state().since.get(account).cloned()
    }

    pub fn set_since(&self, account: &str, since: &str) -> Result<(), String> {
        let mut s = self.read_state();
        s.since.insert(account.into(), since.into());
        self.write(&self.state_path(), serde_json::to_vec(&s).map_err(|e| e.to_string())?)
    }
}

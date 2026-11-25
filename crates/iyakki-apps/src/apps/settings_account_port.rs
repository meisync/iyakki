//! What the account and secret pages of Settings (lane ST-A: Vault, External secret providers, Account, Backup, Encrypted home, Cloud access)
//! ask of the rest of the system. One small trait per page; the real implementation is `impl X for AppCx` here, the one place that reads the
//! session, the vault and the services for those pages. A page names its trait (a `p` parameter, never `cx`); the fake is [`FakeAcct`].
//! docs/development/SETTINGS-EXTRACTION-PLAN.md section 2.

use super::super::settings_port::PagePort;
use super::super::AppCx;
use iyakki_shared::vault::{ProviderConfig as SecretProvider, VaultError};

// ------------------------------------------------------------------------------------------------------------------ Vault

/// The system vault and the person's own vault: lock state, names, lock, and what waits for a locked vault.
pub(crate) trait VaultPort: PagePort {
    /// The names of the secrets of the system vault (`system`) or of the person's own vault.
    fn vault_names(&self, system: bool) -> Result<Vec<String>, VaultError>;
    /// (exists, unlocked) of the system vault.
    fn system_vault_state(&self) -> (bool, bool);
    /// Who holds a key to the system vault.
    fn system_slots(&self) -> Vec<String>;
    /// Lock the system vault until an administrator with access signs in again.
    fn lock_system_vault(&self);
    /// The person's own vault is open (the core's `vault.status`, else the vault itself).
    fn user_vault_unlocked(&self) -> bool;
    /// One sentence naming what waits for the person's locked vault, or none.
    fn vault_waiting_line(&self, unlocked: bool) -> Option<String>;
}

impl VaultPort for AppCx {
    fn vault_names(&self, system: bool) -> Result<Vec<String>, VaultError> {
        if system {
            self.vault().system_names()
        } else {
            self.vault().user_names(&self.session.user)
        }
    }
    fn system_vault_state(&self) -> (bool, bool) {
        self.services.system_vault_state()
    }
    fn system_slots(&self) -> Vec<String> {
        self.services.system_slots()
    }
    fn lock_system_vault(&self) {
        self.services.lock_system();
    }
    fn user_vault_unlocked(&self) -> bool {
        use iyakki_core_api::contract::CallContext;
        let ctx = CallContext::local(&self.session.user, self.session.policy.manage_settings);
        self.services.contract_local().vault().and_then(|v| v.status(&ctx).ok()).map(|v| !v.locked).unwrap_or_else(|| self.services.user_vault_unlocked(&self.session.user))
    }
    fn vault_waiting_line(&self, unlocked: bool) -> Option<String> {
        super::vaultstate_page::waiting_line(self.data_dir(), &self.session.user, unlocked)
    }
}

// ------------------------------------------------------------------------------------------------- External secret providers

/// The list of external secret providers (HashiCorp / OpenBao, 1Password Connect), and what saving or testing one does.
pub(crate) trait SecretProvidersPort: VaultPort {
    fn secret_providers(&self) -> Vec<(String, SecretProvider)>;
    /// The settings would still be valid with this provider added (checked before the token is stored).
    fn check_provider(&self, alias: &str, cfg: &SecretProvider) -> Result<(), String>;
    /// Store the provider's access token in the system vault (audited as `vault.system.set`).
    fn store_system_secret(&self, name: &str, value: &str) -> Result<(), String>;
    /// Save the provider in the settings and tell the secrets service.
    fn save_provider(&self, alias: &str, cfg: SecretProvider) -> Result<(), String>;
    /// A job that asks the provider whether it is reachable and takes the token (run it on a worker).
    fn provider_test_job(&self, alias: &str) -> Box<dyn FnOnce() -> Result<(), String> + Send>;
}

impl SecretProvidersPort for AppCx {
    fn secret_providers(&self) -> Vec<(String, SecretProvider)> {
        self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().secret_providers.clone().into_iter().collect()
    }
    fn check_provider(&self, alias: &str, cfg: &SecretProvider) -> Result<(), String> {
        let mut probe = self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().clone();
        probe.secret_providers.insert(alias.to_string(), cfg.clone());
        probe.validate().map_err(|e| e.to_string())
    }
    fn store_system_secret(&self, name: &str, value: &str) -> Result<(), String> {
        self.vault().system_set(name, value).map_err(|e| match e {
            VaultError::Locked => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            other => other.to_string(),
        })?;
        self.audit.record(&self.session.user, "vault.system.set", name);
        Ok(())
    }
    fn save_provider(&self, alias: &str, cfg: SecretProvider) -> Result<(), String> {
        self.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).upsert_secret_provider(alias, cfg).map_err(|e| e.to_string())?;
        self.services.refresh_secret_providers();
        Ok(())
    }
    fn provider_test_job(&self, alias: &str) -> Box<dyn FnOnce() -> Result<(), String> + Send> {
        let (svc, alias) = (self.services.clone(), alias.to_string());
        Box::new(move || svc.secrets_api().test_provider(&alias).map_err(|e| e.to_string()))
    }
}

// REAL

// ---------------------------------------------------------------------------------------------------------------------- Backup

/// The backup pages: who backs up, the person's home folder, and whether this account may touch the machine's own settings and keys.
pub(crate) trait BackupPort: PagePort {
    /// The signed-in person's name.
    fn backup_user(&self) -> String;
    /// The person's home folder (what a personal backup reads).
    fn backup_home(&self) -> std::path::PathBuf;
    /// This account may back up or restore the machine's own settings and keys.
    fn may_manage_settings(&self) -> bool;
}

impl BackupPort for AppCx {
    fn backup_user(&self) -> String {
        self.session.user.clone()
    }
    fn backup_home(&self) -> std::path::PathBuf {
        let data = self.data_dir().to_path_buf();
        self.core.file_scope().home_root(&self.session.user, &data)
    }
    fn may_manage_settings(&self) -> bool {
        self.session.policy.manage_settings
    }
}

// ------------------------------------------------------------------------------------------------------- Encrypted home

/// The encrypted home: the person's name and the sign-in check (the same password the sign-in uses).
pub(crate) trait EncHomePort: PagePort {
    /// The signed-in person's name.
    fn user_name(&self) -> String;
    /// Is `pass` this person's sign-in password (the store is read; nothing is changed).
    fn verify_sign_in(&self, user: &str, pass: &str) -> bool;
    /// The same check as a value for a background job (it outlives the page).
    fn verifier(&self) -> std::sync::Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
}

impl EncHomePort for AppCx {
    fn user_name(&self) -> String {
        self.session.user.clone()
    }
    fn verify_sign_in(&self, user: &str, pass: &str) -> bool {
        self.store.lock().map(|mut s| s.authenticate(user, pass).is_ok()).unwrap_or(false)
    }
    fn verifier(&self) -> std::sync::Arc<dyn Fn(&str, &str) -> bool + Send + Sync> {
        let store = self.store.clone();
        std::sync::Arc::new(move |user: &str, pass: &str| store.lock().map(|mut s| s.authenticate(user, pass).is_ok()).unwrap_or(false))
    }
}

// ---------------------------------------------------------------------------------------------------------------- Cloud access

/// The roles lists of this machine's cloud tenant, their audit trail, the shared-vault setting and the signed-in person's name.
pub(crate) trait CloudRolesPort: PagePort {
    /// The signed-in person's name.
    fn user(&self) -> String;
    /// The roles lists (None when the machine has none yet).
    fn roles_load(&self) -> Result<Option<iyakki_shared::cloudroles::Roles>, String>;
    /// Save the roles lists.
    fn roles_save(&self, r: &iyakki_shared::cloudroles::Roles) -> Result<(), String>;
    /// Append one line to the roles audit trail.
    fn roles_trail_append(&self, now: u64, c: &iyakki_shared::cloudroles::Change) -> Result<(), String>;
    /// The audit trail as text (empty when there is none).
    fn roles_trail_text(&self) -> String;
    /// One line about which cryptography keeps the shared vault.
    fn sharedvault_line(&self) -> String;
    /// Save a checked shared-vault setting, made by `actor`.
    fn shared_vault_save(&self, s: &iyakki_shared::sharedvault::Setting, actor: &str) -> Result<(), String>;
}

impl CloudRolesPort for AppCx {
    fn user(&self) -> String {
        self.session.user.clone()
    }
    fn roles_load(&self) -> Result<Option<iyakki_shared::cloudroles::Roles>, String> {
        iyakki_shared::cloudroles::load(&iyakki_shared::cloudroles::path(self.data_dir()))
    }
    fn roles_save(&self, r: &iyakki_shared::cloudroles::Roles) -> Result<(), String> {
        iyakki_shared::cloudroles::save(&iyakki_shared::cloudroles::path(self.data_dir()), r)
    }
    fn roles_trail_append(&self, now: u64, c: &iyakki_shared::cloudroles::Change) -> Result<(), String> {
        iyakki_shared::cloudroles::append_audit(&iyakki_shared::cloudroles::audit_path(self.data_dir()), now, c)
    }
    fn roles_trail_text(&self) -> String {
        std::fs::read_to_string(iyakki_shared::cloudroles::audit_path(self.data_dir())).unwrap_or_default()
    }
    fn sharedvault_line(&self) -> String {
        self.agent.sharedvault_page_line(self.data_dir())
    }
    fn shared_vault_save(&self, s: &iyakki_shared::sharedvault::Setting, actor: &str) -> Result<(), String> {
        s.save(self.data_dir(), actor)
    }
}


// --------------------------------------------------------------------------------------------------------------------- Account

/// What the person's own limits are (shown on the Account page): the role, the file quota, the memory limit and the apps allowed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccountFacts {
    pub role: String,
    pub vfs_quota_mb: u64,
    pub max_memory_mb: u64,
    pub apps: Vec<String>,
}

/// The Account page: who is signed in and what the person may use. The text size, theme and wallpaper cards take the device port's
/// `UserPort`, which this trait extends.
pub(crate) trait AccountPort: super::super::settings_device_port::UserPort {
    fn account_facts(&self) -> AccountFacts;
}

impl AccountPort for AppCx {
    fn account_facts(&self) -> AccountFacts {
        let p = &self.session.policy;
        AccountFacts {
            role: self.session.role.name().to_string(),
            vfs_quota_mb: p.vfs_quota_mb as u64,
            max_memory_mb: p.max_memory_mb as u64,
            apps: p.apps.iter().map(|x| x.id().to_string()).collect(),
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------- Fake

/// The fake account port: an administrator switch, an audit list, a manual clock and in-memory state for each page.
#[cfg(test)]
pub(crate) struct FakeAcct {
    pub admin: std::sync::atomic::AtomicBool,
    pub audit_lines: std::sync::Mutex<Vec<(String, String)>>,
    pub clock: iyakki_testkit::ManualClock,
    pub dir: std::path::PathBuf,
    /// Every fake call that can be refused answers this while it is set.
    pub refuse: std::sync::Mutex<Option<String>>,
    pub system_secrets: std::sync::Mutex<Vec<String>>,
    pub user_secrets: std::sync::Mutex<Vec<String>>,
    pub vault_state: std::sync::Mutex<(bool, bool)>,
    pub user_unlocked: std::sync::atomic::AtomicBool,
    pub slots: std::sync::Mutex<Vec<String>>,
    pub waiting: std::sync::Mutex<Option<String>>,
    pub locked_calls: std::sync::atomic::AtomicUsize,
    pub providers: std::sync::Mutex<std::collections::BTreeMap<String, SecretProvider>>,
    pub tested: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub user_name: String,
    pub roles_in_memory: std::sync::Mutex<Result<Option<iyakki_shared::cloudroles::Roles>, String>>,
    pub trail: std::sync::Mutex<Vec<String>>,
    pub shared_saved: std::sync::Mutex<Vec<String>>,
    pub sign_in_password: std::sync::Mutex<String>,
    pub enc_user: String,
    // FIELDS
}

#[cfg(test)]
impl FakeAcct {
    pub(crate) fn new(admin: bool) -> FakeAcct {
        FakeAcct {
            admin: admin.into(),
            audit_lines: Default::default(),
            clock: iyakki_testkit::ManualClock::new(1_000_000),
            dir: std::path::PathBuf::from("fake-data"),
            refuse: Default::default(),
            system_secrets: Default::default(),
            user_secrets: Default::default(),
            vault_state: std::sync::Mutex::new((true, true)),
            user_unlocked: true.into(),
            slots: Default::default(),
            waiting: Default::default(),
            locked_calls: 0.into(),
            providers: Default::default(),
            tested: Default::default(),
            user_name: "owner".into(),
            roles_in_memory: std::sync::Mutex::new(Ok(None)),
            trail: Default::default(),
            shared_saved: Default::default(),
            sign_in_password: std::sync::Mutex::new("correct horse".into()),
            enc_user: "owner".into(),
            // INIT
        }
    }
    pub(crate) fn audited(&self) -> Vec<(String, String)> {
        self.audit_lines.lock().unwrap().clone()
    }
    fn refused(&self) -> Result<(), String> {
        self.refuse.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

#[cfg(test)]
impl PagePort for FakeAcct {
    fn is_admin(&self) -> bool {
        self.admin.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit_lines.lock().unwrap().push((action.to_string(), subject.to_string()));
    }
    fn data_dir(&self) -> std::path::PathBuf {
        self.dir.clone()
    }
    fn now_secs(&self) -> u64 {
        self.clock.now_secs()
    }
}

#[cfg(test)]
impl CloudRolesPort for FakeAcct {
    fn user(&self) -> String {
        self.user_name.clone()
    }
    fn roles_load(&self) -> Result<Option<iyakki_shared::cloudroles::Roles>, String> {
        self.roles_in_memory.lock().unwrap().clone()
    }
    fn roles_save(&self, r: &iyakki_shared::cloudroles::Roles) -> Result<(), String> {
        self.refused()?;
        *self.roles_in_memory.lock().unwrap() = Ok(Some(r.clone()));
        Ok(())
    }
    fn roles_trail_append(&self, now: u64, c: &iyakki_shared::cloudroles::Change) -> Result<(), String> {
        self.trail.lock().unwrap().push(format!("{now} {}", c.0));
        Ok(())
    }
    fn roles_trail_text(&self) -> String {
        self.trail.lock().unwrap().join("\n")
    }
    fn sharedvault_line(&self) -> String {
        "The shared vault is kept by the built-in cryptography (not externally reviewed).".into()
    }
    fn shared_vault_save(&self, s: &iyakki_shared::sharedvault::Setting, actor: &str) -> Result<(), String> {
        self.refused()?;
        self.shared_saved.lock().unwrap().push(format!("{actor}: {}", s.provider.word()));
        Ok(())
    }
}

#[cfg(test)]
impl EncHomePort for FakeAcct {
    fn user_name(&self) -> String {
        self.enc_user.clone()
    }
    fn verify_sign_in(&self, user: &str, pass: &str) -> bool {
        user == self.enc_user && *self.sign_in_password.lock().unwrap() == pass
    }
    fn verifier(&self) -> std::sync::Arc<dyn Fn(&str, &str) -> bool + Send + Sync> {
        let want = self.sign_in_password.lock().unwrap().clone();
        let who = self.enc_user.clone();
        std::sync::Arc::new(move |user: &str, pass: &str| user == who && pass == want)
    }
}

#[cfg(test)]
impl BackupPort for FakeAcct {
    fn backup_user(&self) -> String {
        self.enc_user.clone()
    }
    fn backup_home(&self) -> std::path::PathBuf {
        self.dir.join("home")
    }
    fn may_manage_settings(&self) -> bool {
        self.admin.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl VaultPort for FakeAcct {
    fn vault_names(&self, system: bool) -> Result<Vec<String>, VaultError> {
        if !self.vault_state.lock().unwrap().1 && system {
            return Err(VaultError::Locked);
        }
        Ok(if system { self.system_secrets.lock().unwrap().clone() } else { self.user_secrets.lock().unwrap().clone() })
    }
    fn system_vault_state(&self) -> (bool, bool) {
        *self.vault_state.lock().unwrap()
    }
    fn system_slots(&self) -> Vec<String> {
        self.slots.lock().unwrap().clone()
    }
    fn lock_system_vault(&self) {
        self.locked_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.vault_state.lock().unwrap().1 = false;
    }
    fn user_vault_unlocked(&self) -> bool {
        self.user_unlocked.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn vault_waiting_line(&self, unlocked: bool) -> Option<String> {
        if unlocked {
            None
        } else {
            self.waiting.lock().unwrap().clone()
        }
    }
}

#[cfg(test)]
impl SecretProvidersPort for FakeAcct {
    fn secret_providers(&self) -> Vec<(String, SecretProvider)> {
        self.providers.lock().unwrap().clone().into_iter().collect()
    }
    fn check_provider(&self, alias: &str, cfg: &SecretProvider) -> Result<(), String> {
        let mut probe = iyakki_shared::settings::Settings::default();
        probe.secret_providers = self.providers.lock().unwrap().clone().into_iter().collect();
        probe.secret_providers.insert(alias.to_string(), cfg.clone());
        probe.validate().map_err(|e| e.to_string())
    }
    fn store_system_secret(&self, name: &str, _value: &str) -> Result<(), String> {
        self.refused()?;
        self.system_secrets.lock().unwrap().push(name.to_string());
        self.audit("vault.system.set", name);
        Ok(())
    }
    fn save_provider(&self, alias: &str, cfg: SecretProvider) -> Result<(), String> {
        self.refused()?;
        self.providers.lock().unwrap().insert(alias.to_string(), cfg);
        Ok(())
    }
    fn provider_test_job(&self, alias: &str) -> Box<dyn FnOnce() -> Result<(), String> + Send> {
        let (tested, alias, refuse) = (self.tested.clone(), alias.to_string(), self.refuse.lock().unwrap().clone());
        Box::new(move || {
            tested.lock().unwrap().push(alias);
            refuse.map_or(Ok(()), Err)
        })
    }
}

// FAKEIMPL

#[cfg(test)]
impl super::super::settings_device_port::UserPort for FakeAcct {
    fn user(&self) -> String {
        self.user_name.clone()
    }
}

#[cfg(test)]
impl AccountPort for FakeAcct {
    fn account_facts(&self) -> AccountFacts {
        AccountFacts {
            role: if self.admin.load(std::sync::atomic::Ordering::SeqCst) { "Administrator".into() } else { "User".into() },
            vfs_quota_mb: 512,
            max_memory_mb: 1024,
            apps: vec!["files".into(), "notes".into()],
        }
    }
}

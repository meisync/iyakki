//! What the Settings -> Browser pages ask of the rest of the system (lane ST-C of docs/development/SETTINGS-EXTRACTION-PLAN.md).
//!
//! [`BrowserSettingsPort`] is the group trait of the sixteen Browser sub-pages. It stands on the two things every page has: [`PagePort`] (who is
//! looking, the audit line, the clock) and [`UserData`] (the person, their data folder and policy, the agent's pure helpers: the browser's small
//! stores such as `browser_config`, `browser_block` and `browser_badsites` already take a `&dyn UserData`). What is left is the handful of
//! reads and writes that were `cx.devtools()`, `cx.services`, `cx.web`, `cx.os` and `cx.voice_settings()` in the pages.
//!
//! The real implementation is `impl BrowserSettingsPort for AppCx` below (the one place that reads those handles); [`FakeBrowserPort`] is the
//! fake: a fake chat port for the person, the folder and the audit list, an in-memory dev-tools switch, protection state, page cache, machine
//! numbers and header rules. Nothing is read from the disk by the fake except the person's own small files in its temp folder, and nothing sleeps.

use std::sync::Arc;

use iyakki_agent_api::{DevToolsApi, HeaderRuleStore};
use iyakki_shared::profilestore::Status as ProfileStatus;

use crate::apps::browser_speed::Machine;
use crate::apps::chat_port::UserData;
use crate::apps::settings_port::PagePort;
use crate::apps::AppCx;

/// The Browser pages' view of the shell.
pub(crate) trait BrowserSettingsPort: PagePort + UserData {
    /// The developer tools of the browser (the switch, the summary).
    fn devtools(&self) -> Arc<dyn DevToolsApi>;
    /// The microphone the voice settings name (the browser's device sharing follows it).
    fn voice_mic(&self) -> String;
    /// Whether the person's browser data is protected (encrypted with a key in the vault), and the three changes of it.
    fn profile_status(&self) -> ProfileStatus;
    fn profile_protect(&self) -> Result<String, String>;
    fn profile_unprotect(&self) -> Result<String, String>;
    fn profile_reset(&self) -> String;
    /// How much of the person's page cache is kept, and emptying it.
    fn web_cache_stats(&self) -> Option<iyakki_web::cache::CacheStats>;
    fn web_cache_clear(&self);
    /// The memory, free disk and cores of this machine (what the cache sizes are chosen from).
    fn machine(&self) -> Machine;
    /// The person's custom header rules, and making the browser's transport read them again.
    fn header_store(&self) -> Arc<dyn HeaderRuleStore>;
    fn header_refresh(&self);
}

impl BrowserSettingsPort for AppCx {
    fn devtools(&self) -> Arc<dyn DevToolsApi> {
        AppCx::devtools(self)
    }
    fn voice_mic(&self) -> String {
        self.voice_settings().mic.clone()
    }
    fn profile_status(&self) -> ProfileStatus {
        self.agent.profile_store().status_of(&self.services, &self.session.user)
    }
    fn profile_protect(&self) -> Result<String, String> {
        self.agent.profile_store().protect_for(&self.services, &self.session.user)
    }
    fn profile_unprotect(&self) -> Result<String, String> {
        self.agent.profile_store().unprotect_for(&self.services, &self.session.user)
    }
    fn profile_reset(&self) -> String {
        self.agent.profile_store().reset_for(&self.services, &self.session.user)
    }
    fn web_cache_stats(&self) -> Option<iyakki_web::cache::CacheStats> {
        self.web.cache_stats(&self.session.user)
    }
    fn web_cache_clear(&self) {
        self.web.clear_cache(&self.session.user)
    }
    fn machine(&self) -> Machine {
        crate::apps::browser_speed::machine_os(&*self.os, self.data_dir())
    }
    fn header_store(&self) -> Arc<dyn HeaderRuleStore> {
        crate::apps::browser_headers::store(self)
    }
    fn header_refresh(&self) {
        crate::apps::browser_headers::refresh(self)
    }
}

#[cfg(test)]
pub(crate) use fake::FakeBrowserPort;

#[cfg(test)]
mod fake {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use iyakki_agent_api::AgentApi;
    use iyakki_core_api::Core;
    use iyakki_sec::policy::{Policy, Role};

    use crate::apps::chat_port::{AgentHelpers, FakeChatPort};
    use super::*;

    /// The fake: a signed-in person (administrator or not) with a temp data folder, and in-memory answers for what the real port reads from the shell.
    pub(crate) struct FakeBrowserPort {
        pub chat: FakeChatPort,
        pub devtools: Arc<iyakki_shared::devtools::DevTools>,
        pub mic: Mutex<String>,
        pub profile: Mutex<ProfileStatus>,
        /// The next protect, unprotect or reset answers with this refusal while set.
        pub profile_refuses: AtomicBool,
        pub cache: Mutex<Option<iyakki_web::cache::CacheStats>>,
        pub cache_cleared: AtomicUsize,
        pub machine: Mutex<Machine>,
        pub headers: Arc<MemHeaders>,
        pub refreshed: AtomicUsize,
    }

    impl FakeBrowserPort {
        pub(crate) fn new(tag: &str, admin: bool) -> FakeBrowserPort {
            let dir = crate::apps::tests::temp_dir(tag);
            let store = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
            let role = if admin { Role::Admin } else { Role::User };
            let agent: Arc<dyn AgentApi> = (crate::testkit::kit().expect("install a test kit").agent)();
            let chat = FakeChatPort::new("ada", role.template(), &dir, Arc::new(iyakki_core_fake::FakeCore::new()) as Arc<dyn Core>, iyakki_agent_api::SettingsRef::of_store(&store)).with_agent(agent);
            FakeBrowserPort {
                chat,
                devtools: Arc::new(iyakki_shared::devtools::DevTools::new(&dir)),
                mic: Mutex::new("default".into()),
                profile: Mutex::new(ProfileStatus::Off),
                profile_refuses: false.into(),
                cache: Mutex::new(None),
                cache_cleared: 0.into(),
                machine: Mutex::new(Machine { mem_total: 8 << 30, mem_available: 4 << 30, disk_free: 100 << 30, cores: 4 }),
                headers: Arc::new(MemHeaders::default()),
                refreshed: 0.into(),
            }
        }
        /// The audit lines as `(action, subject)`.
        pub(crate) fn audited(&self) -> Vec<(String, String)> {
            self.chat.audited().into_iter().map(|(_, a, s)| (a, s)).collect()
        }
        pub(crate) fn policy_mut(&mut self) -> &mut Policy {
            &mut self.chat.policy
        }
    }

    impl PagePort for FakeBrowserPort {
        fn is_admin(&self) -> bool {
            self.chat.policy.manage_settings
        }
        fn audit(&self, action: &str, subject: &str) {
            UserData::record_audit(self, action, subject)
        }
        fn data_dir(&self) -> std::path::PathBuf {
            self.chat.dir.clone()
        }
        fn now_secs(&self) -> u64 {
            1_700_000_000
        }
    }

    impl AgentHelpers for FakeBrowserPort {
        fn agent(&self) -> Arc<dyn AgentApi> {
            AgentHelpers::agent(&self.chat)
        }
    }

    impl UserData for FakeBrowserPort {
        fn acting_user(&self) -> &str {
            self.chat.acting_user()
        }
        fn data_root(&self) -> &Path {
            self.chat.data_root()
        }
        fn acting_policy(&self) -> &Policy {
            self.chat.acting_policy()
        }
        fn record_audit(&self, action: &str, subject: &str) {
            self.chat.record_audit(action, subject)
        }
        fn core_handle(&self) -> Arc<dyn Core> {
            self.chat.core_handle()
        }
        fn vault_handle(&self) -> Arc<dyn iyakki_agent_api::VaultRead> {
            self.chat.vault_handle()
        }
        fn web_for_user(&self) -> Arc<dyn iyakki_web::Fetch> {
            self.chat.web_for_user()
        }
    }

    impl BrowserSettingsPort for FakeBrowserPort {
        fn devtools(&self) -> Arc<dyn DevToolsApi> {
            self.devtools.clone()
        }
        fn voice_mic(&self) -> String {
            self.mic.lock().unwrap().clone()
        }
        fn profile_status(&self) -> ProfileStatus {
            *self.profile.lock().unwrap()
        }
        fn profile_protect(&self) -> Result<String, String> {
            if self.profile_refuses.load(Ordering::SeqCst) {
                return Err("The vault is locked.".into());
            }
            *self.profile.lock().unwrap() = ProfileStatus::Open;
            Ok("Browser data is protected.".into())
        }
        fn profile_unprotect(&self) -> Result<String, String> {
            if self.profile_refuses.load(Ordering::SeqCst) {
                return Err("The vault is locked.".into());
            }
            *self.profile.lock().unwrap() = ProfileStatus::Off;
            Ok("Browser data is plain again.".into())
        }
        fn profile_reset(&self) -> String {
            *self.profile.lock().unwrap() = ProfileStatus::Off;
            "Browser data deleted.".into()
        }
        fn web_cache_stats(&self) -> Option<iyakki_web::cache::CacheStats> {
            self.cache.lock().unwrap().clone()
        }
        fn web_cache_clear(&self) {
            self.cache_cleared.fetch_add(1, Ordering::SeqCst);
        }
        fn machine(&self) -> Machine {
            let m = self.machine.lock().unwrap();
            Machine { mem_total: m.mem_total, mem_available: m.mem_available, disk_free: m.disk_free, cores: m.cores }
        }
        fn header_store(&self) -> Arc<dyn HeaderRuleStore> {
            self.headers.clone()
        }
        fn header_refresh(&self) {
            self.refreshed.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Header rules kept in memory (the values too: the fake has no vault).
    #[derive(Default)]
    pub(crate) struct MemHeaders {
        rules: Mutex<Vec<iyakki_shared::browserheaders::HeaderRule>>,
    }

    impl HeaderRuleStore for MemHeaders {
        fn list(&self) -> Vec<iyakki_shared::browserheaders::HeaderRule> {
            self.rules.lock().unwrap().clone()
        }
        fn add(&self, mut r: iyakki_shared::browserheaders::HeaderRule, _value: &str) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
            let mut v = self.rules.lock().unwrap();
            r.id = format!("r{}", v.len() + 1);
            v.push(r.clone());
            Ok(r)
        }
        fn update(&self, id: &str, r: iyakki_shared::browserheaders::HeaderRule, _value: Option<&str>) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
            let mut v = self.rules.lock().unwrap();
            let slot = v.iter_mut().find(|x| x.id == id).ok_or("No such rule.")?;
            let mut r = r;
            r.id = id.to_string();
            *slot = r.clone();
            Ok(r)
        }
        fn set_enabled(&self, id: &str, on: bool) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
            let mut v = self.rules.lock().unwrap();
            let slot = v.iter_mut().find(|x| x.id == id).ok_or("No such rule.")?;
            slot.enabled = on;
            Ok(slot.clone())
        }
        fn delete(&self, id: &str) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
            let mut v = self.rules.lock().unwrap();
            let at = v.iter().position(|x| x.id == id).ok_or("No such rule.")?;
            Ok(v.remove(at))
        }
    }
}

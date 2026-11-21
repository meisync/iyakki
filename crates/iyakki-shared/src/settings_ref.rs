//! The system settings behind a small trait (F-SPLIT-03, UI-RPC-DESIGN.md): a screen reads a snapshot and changes the settings by replacing them when
//! nothing changed since it read them, so the same code runs over the store in this process and over a connection to the core.
//!
//! The old code held `Arc<Mutex<SettingsStore>>` and wrote `settings().lock().unwrap().get().field` and `.update(|s| ...)`. [`SettingsRef`] keeps that
//! shape (`lock()` returns a [`SettingsGuard`] with `get`, `update`, `upsert_provider` ...), but the guard is a snapshot, not a lock: reads are the
//! settings as they were when `lock()` was called, and `update` applies the change to the latest settings and replaces them at the generation it read.
//! If another change got in between (a window of a few microseconds in one process) the update answers an error and nothing is written.
//! A closure never crosses the socket: only whole `Settings` values do.

use std::sync::{Arc, Mutex, PoisonError};

use crate::providers::ProviderCfg;
use crate::settings::{ExtEntry, ModelChoice, Settings, SettingsError, SettingsStore};
use crate::vault::ProviderConfig as SecretProviderCfg;

/// The settings with the count of changes they were read at, and why they cannot be changed (a newer release wrote the file), if so.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SettingsSnap {
    pub generation: u64,
    pub settings: Settings,
    pub read_only: Option<String>,
}

pub trait SettingsSource: Send + Sync {
    fn snapshot(&self) -> SettingsSnap;
    /// Replace the settings when `generation` is still the current count (`Ok(false)`: overtaken, nothing written); validates and saves like `update`.
    fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError>;
}

impl SettingsSource for Arc<Mutex<SettingsStore>> {
    fn snapshot(&self) -> SettingsSnap {
        let g = self.lock().unwrap_or_else(|e| e.into_inner());
        SettingsSnap { generation: g.generation(), settings: g.get().clone(), read_only: g.read_only().map(str::to_string) }
    }
    fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError> {
        self.lock().unwrap_or_else(|e| e.into_inner()).replace_if(generation, settings)
    }
}

/// The settings of a services handle (or of one store in this process, [`SettingsRef::of_store`]), cheap to clone.
#[derive(Clone)]
pub struct SettingsRef(pub Arc<dyn SettingsSource>);

impl SettingsRef {
    pub fn new(api: Arc<dyn SettingsSource>) -> SettingsRef {
        SettingsRef(api)
    }

    /// A store of this process.
    pub fn of_store(store: &Arc<Mutex<SettingsStore>>) -> SettingsRef {
        SettingsRef(Arc::new(store.clone()))
    }

    /// The settings as they are now (the `Result` is the shape of the old `Mutex::lock`; it is never an error).
    pub fn lock(&self) -> Result<SettingsGuard, PoisonError<SettingsGuard>> {
        Ok(SettingsGuard { api: self.0.clone(), snap: self.0.snapshot() })
    }
}

pub struct SettingsGuard {
    api: Arc<dyn SettingsSource>,
    snap: SettingsSnap,
}

impl SettingsGuard {
    pub fn get(&self) -> &Settings {
        &self.snap.settings
    }

    /// Why the settings cannot be changed, if so.
    pub fn read_only(&self) -> Option<&str> {
        self.snap.read_only.as_deref()
    }

    /// Apply a change atomically: it takes effect only if the result validates and saves, and nothing else changed meanwhile.
    pub fn update(&mut self, f: impl FnOnce(&mut Settings)) -> Result<(), SettingsError> {
        let fresh = self.api.snapshot();
        let mut next = fresh.settings.clone();
        f(&mut next);
        match self.api.replace_if(fresh.generation, next)? {
            true => {
                self.snap = self.api.snapshot();
                Ok(())
            }
            false => {
                self.snap = fresh;
                Err(SettingsError::Storage("the settings changed while this change was made; try again".into()))
            }
        }
    }

    pub fn upsert_provider(&mut self, p: ProviderCfg) -> Result<(), SettingsError> {
        self.update(|s| s.put_provider(p))
    }

    pub fn remove_provider(&mut self, id: &str) -> Result<(), SettingsError> {
        if self.get().provider(id).is_none() {
            return Err(SettingsError::Invalid(format!("no provider `{id}`")));
        }
        self.update(|s| s.drop_provider(id))
    }

    pub fn set_chat(&mut self, c: Option<ModelChoice>) -> Result<(), SettingsError> {
        self.update(|s| s.chat = c)
    }

    pub fn set_embedding(&mut self, c: Option<ModelChoice>) -> Result<(), SettingsError> {
        self.update(|s| s.embedding = c)
    }

    pub fn upsert_external(&mut self, e: ExtEntry) -> Result<(), SettingsError> {
        self.update(|s| s.put_external(e))
    }

    pub fn remove_external(&mut self, id: &str) -> Result<(), SettingsError> {
        self.update(|s| s.drop_external(id))
    }

    pub fn upsert_secret_provider(&mut self, alias: &str, cfg: SecretProviderCfg) -> Result<(), SettingsError> {
        self.update(|s| s.put_secret_provider(alias, cfg))
    }

    pub fn remove_secret_provider(&mut self, alias: &str) -> Result<(), SettingsError> {
        self.update(|s| s.drop_secret_provider(alias))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn store(tag: &str) -> Arc<Mutex<SettingsStore>> {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("iyakki-settingsref-{}-{}-{}", std::process::id(), tag, N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Arc::new(Mutex::new(SettingsStore::open(&d.join("settings.json")).unwrap()))
    }

    #[test]
    fn a_guard_reads_a_snapshot_and_an_update_lands_in_the_store_and_in_the_guard() {
        let st = store("a");
        let r = SettingsRef::of_store(&st);
        let mut g = r.lock().unwrap();
        let before = g.get().tool_log_days;
        g.update(|s| s.tool_log_days = before + 5).unwrap();
        assert_eq!(g.get().tool_log_days, before + 5, "the guard shows what it wrote");
        assert_eq!(st.lock().unwrap().get().tool_log_days, before + 5, "the store has it");
        assert_eq!(r.lock().unwrap().get().tool_log_days, before + 5, "a new guard reads it");
    }

    #[test]
    fn a_change_that_does_not_validate_is_refused_and_nothing_is_written() {
        let st = store("b");
        let r = SettingsRef::of_store(&st);
        let gen = st.lock().unwrap().generation();
        let err = r.lock().unwrap().update(|s| s.vault.keyfile = Some("relative/path".into())).unwrap_err();
        assert!(err.to_string().contains("absolute"), "{err}");
        assert_eq!(st.lock().unwrap().generation(), gen);
        assert!(st.lock().unwrap().get().vault.keyfile.is_none());
    }

    #[test]
    fn replace_if_refuses_a_writer_that_was_overtaken_and_says_so() {
        let st = store("c");
        let src: &dyn SettingsSource = &st;
        let a = src.snapshot();
        // somebody else changes the settings after `a` was read
        st.lock().unwrap().update(|s| s.tool_log_days = 11).unwrap();
        let mut mine = a.settings.clone();
        mine.tool_log_days = 99;
        assert_eq!(src.replace_if(a.generation, mine.clone()), Ok(false), "overtaken: nothing written");
        assert_eq!(st.lock().unwrap().get().tool_log_days, 11);
        let b = src.snapshot();
        assert_eq!(src.replace_if(b.generation, mine), Ok(true));
        assert_eq!(st.lock().unwrap().get().tool_log_days, 99);
    }

    #[test]
    fn a_guard_whose_update_is_overtaken_by_the_moment_gets_an_error_not_a_lost_change() {
        // a source that lets another writer in between the guard's read and its write
        struct Racy(Arc<Mutex<SettingsStore>>);
        impl SettingsSource for Racy {
            fn snapshot(&self) -> SettingsSnap {
                SettingsSource::snapshot(&self.0)
            }
            fn replace_if(&self, generation: u64, settings: Settings) -> Result<bool, SettingsError> {
                self.0.lock().unwrap().update(|s| s.tool_log_days = 77).unwrap();
                SettingsSource::replace_if(&self.0, generation, settings)
            }
        }
        let st = store("d");
        let r = SettingsRef::new(Arc::new(Racy(st.clone())));
        let err = r.lock().unwrap().update(|s| s.tool_log_days = 1).unwrap_err();
        assert!(err.to_string().contains("try again"), "{err}");
        assert_eq!(st.lock().unwrap().get().tool_log_days, 77, "the other writer's change stands");
    }

    #[test]
    fn the_provider_helpers_are_the_stores_own_changes() {
        let st = store("e");
        let r = SettingsRef::of_store(&st);
        let mut g = r.lock().unwrap();
        g.upsert_provider(ProviderCfg::new("local", crate::providers::Kind::Ollama)).unwrap();
        g.set_chat(Some(ModelChoice { provider: "local".into(), model: "qwen".into() })).unwrap();
        assert!(g.remove_provider("nope").is_err());
        g.remove_provider("local").unwrap();
        assert!(g.get().providers.is_empty() && g.get().chat.is_none(), "removing a provider clears the choice that pointed at it");
        assert_eq!(st.lock().unwrap().get(), g.get());
    }
}

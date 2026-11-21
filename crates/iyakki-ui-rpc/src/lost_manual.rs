// The quiet value of a lost call for types that have no `Default` (and where adding one to the type would say more than it should).
impl Lost for iyakki_shared::quickinstall::Presence {
    fn lost(_: &str) -> Self {
        iyakki_shared::quickinstall::Presence::Missing
    }
}
impl Lost for iyakki_shared::tasks::AutoAssign {
    /// Off: a lost core never assigns work by itself.
    fn lost(_: &str) -> Self {
        iyakki_shared::tasks::AutoAssign::Off
    }
}
/// A run that is already over: no events, and nobody to send a decision to.
impl Lost for iyakki_shared::agent::RunHandle {
    fn lost(_: &str) -> Self {
        let (_events_tx, events) = std::sync::mpsc::channel();
        let (decisions, _decisions_rx) = std::sync::mpsc::channel();
        iyakki_shared::agent::RunHandle::from_parts(events, decisions)
    }
}
/// A live session that never started: it ended at once.
impl Lost for iyakki_shared::geminilive_host::Running {
    fn lost(why: &str) -> Self {
        iyakki_shared::geminilive_host::Running::new(
            Arc::new(Mutex::new(iyakki_shared::geminilive_host::RunState::Ended(why.to_string()))),
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            None,
            Arc::new(|| {}),
        )
    }
}
/// A voice assistant that is idle and silent.
impl Lost for iyakki_shared::voiceassist::Status {
    fn lost(_: &str) -> Self {
        iyakki_shared::voiceassist::Status {
            state: iyakki_shared::voiceassist::State::Idle,
            muted: false,
            mic_open: false,
            level: 0.0,
            heard: String::new(),
            said: String::new(),
            note: String::new(),
            approval: None,
            confirm: None,
        }
    }
}
/// A removal plan with no step: nothing would be removed.
impl Lost for iyakki_shared::quickinstall_uninstall::Plan {
    fn lost(_: &str) -> Self {
        iyakki_shared::quickinstall_uninstall::Plan {
            item: iyakki_shared::quickinstall::Item::Node,
            steps: Vec::new(),
            data_requires: false,
            always_stays: Vec::new(),
            claim_gb: 0,
            data_bytes: 0,
            notes: Vec::new(),
        }
    }
}
/// The kubectl of the model hosts, not served: the program was not found.
struct NoKube;
impl iyakki_shared::modelhost::Kube for NoKube {
    fn kubectl(&self, _args: &[&str]) -> iyakki_shared::wsexec::Out {
        iyakki_shared::wsexec::Out { stderr: "not served over the UI socket".to_string(), missing: true, ..Default::default() }
    }
}
impl Lost for Arc<dyn iyakki_shared::modelhost::Kube + Send + Sync> {
    fn lost(_: &str) -> Self {
        Arc::new(NoKube)
    }
}
impl Lost for iyakki_shared::dictfix::Target {
    fn lost(_: &str) -> Self {
        iyakki_shared::dictfix::Target::NoModel
    }
}
impl Lost for iyakki_shared::gitjobs::GitCheck {
    fn lost(why: &str) -> Self {
        iyakki_shared::gitjobs::GitCheck::Missing { message: why.to_string() }
    }
}
/// A host-key probe that is not served: it finds no key.
impl Lost for iyakki_shared::gitjobs::Probe {
    fn lost(_: &str) -> Self {
        Arc::new(|_, _| Err("not served over the UI socket".to_string()))
    }
}

/// A service whose state was not asked: the cluster does not answer, so nothing is claimed to run.
impl Lost for iyakki_shared::platform_types::Status {
    fn lost(why: &str) -> Self {
        iyakki_shared::platform_types::Status { state: iyakki_shared::platform_types::SvcState::Unreachable, ready: 0, desired: 0, pods: Vec::new(), live_images: Vec::new(), notes: vec![why.to_string()], reachable: None }
    }
}

/// A review nobody could make: refused with the reason, so nothing is ever installed from it.
impl Lost for iyakki_shared::store_install::Review {
    fn lost(why: &str) -> Self {
        use iyakki_shared::store_trust::{Level, Verdict};
        let refusal = format!("not served over the UI socket: {why}");
        iyakki_shared::store_install::Review {
            kind: String::new(),
            name: String::new(),
            version: String::new(),
            title: String::new(),
            meta: Default::default(),
            verdict: Verdict { level: Level::Unsigned, signer: None, publisher: None, label: String::new(), chain: Vec::new(), warnings: Vec::new(), refusal: Some(refusal.clone()) },
            steps: Vec::new(),
            permissions: Vec::new(),
            core_requests: Vec::new(),
            core_lines: Vec::new(),
            members: Vec::new(),
            needs_newer_iyakki: false,
            warnings: Vec::new(),
            scope: iyakki_shared::skills::Scope::Shared,
            refusal: Some(refusal),
            locked: false,
            installed: None,
            preview: Vec::new(),
            sha256: String::new(),
            size: 0,
            member_install: Vec::new(),
            input: iyakki_shared::store_install::Input { bytes: Vec::new(), entry: None, source: String::new(), builtin: false, catalog_expired: false, known: Vec::new(), rollback: false },
        }
    }
}

/// A removal plan of nothing: it removes nothing and keeps nothing.
impl Lost for iyakki_shared::store_lifecycle::RemovePlan {
    fn lost(_: &str) -> Self {
        iyakki_shared::store_lifecycle::RemovePlan { key: String::new(), title: String::new(), ty: iyakki_shared::store_installed::Type::App, removes: Vec::new(), data: Vec::new(), vault: Vec::new(), vault_note: String::new(), dependents: Vec::new(), cached: 0 }
    }
}

impl crate::LostErr for iyakki_shared::store_install::InstallError {
    fn lost(why: &str) -> Self {
        iyakki_shared::store_install::InstallError::Failed(why.to_string())
    }
}
impl crate::LostErr for iyakki_shared::store_lifecycle::LifeError {
    fn lost(why: &str) -> Self {
        iyakki_shared::store_lifecycle::LifeError::Failed(why.to_string())
    }
}
impl crate::LostErr for iyakki_shared::store_publishflow::Stop {
    fn lost(why: &str) -> Self {
        iyakki_shared::store_publishflow::Stop::Refused(why.to_string())
    }
}

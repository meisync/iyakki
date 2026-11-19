//! `iyakki_agent_api::facets_stores` over the agent's own stores.

use std::path::Path;
use std::time::Duration;

use std::sync::Arc;
use iyakki_proxy::Streamer;
use iyakki_tile_api::radio::{CertProblem, RadioState, Station, BANDS};
use iyakki_shared::jobs::*;
use iyakki_shared::mcp::*;
use iyakki_shared::pkg::*;
use iyakki_shared::runs::*;
use iyakki_shared::sessions::*;
use iyakki_shared::skills::*;

impl iyakki_agent_api::SessionApi for crate::sessions::SessionStore {
    fn create(&self, user: &str) -> Result<Session, String> {
        crate::sessions::SessionStore::create(self, user)
    }
    fn list(&self, user: &str) -> Vec<SessionMeta> {
        crate::sessions::SessionStore::list(self, user)
    }
    fn load(&self, user: &str, id: &str) -> Option<Session> {
        crate::sessions::SessionStore::load(self, user, id)
    }
    fn save(&self, s: &mut Session) -> Result<(), String> {
        crate::sessions::SessionStore::save(self, s)
    }
    fn remember_turn(&self, s: &Session, question: &str, answer: &str) {
        crate::sessions::SessionStore::remember_turn(self, s, question, answer)
    }
}

impl iyakki_agent_api::SkillApi for crate::skills::SkillStore {
    fn add(&self, scope: Scope, markdown: &str, can_shared: bool, can_own: bool) -> Result<Skill, SkillError> {
        crate::skills::SkillStore::add(self, scope, markdown, can_shared, can_own)
    }
    fn get(&self, scope: &Scope, name: &str) -> Option<Skill> {
        crate::skills::SkillStore::get(self, scope, name)
    }
    fn list(&self, user: &str) -> Vec<Skill> {
        crate::skills::SkillStore::list(self, user)
    }
    fn list_scope(&self, scope: &Scope) -> Vec<Skill> {
        crate::skills::SkillStore::list_scope(self, scope)
    }
    fn remove(&self, scope: &Scope, name: &str, can_shared: bool, can_own: bool) -> Result<(), SkillError> {
        crate::skills::SkillStore::remove(self, scope, name, can_shared, can_own)
    }
}

impl iyakki_agent_api::RunApi for crate::runs::RunStore {
    fn delete(&self, user: &str, id: &str) -> Result<(), String> {
        crate::runs::RunStore::delete(self, user, id)
    }
    fn list(&self, user: &str) -> Vec<RunMeta> {
        crate::runs::RunStore::list(self, user)
    }
    fn live(&self) -> Vec<LiveRun> {
        crate::runs::RunStore::live(self)
    }
    fn load(&self, user: &str, id: &str) -> Result<(RunLog, Integrity), String> {
        crate::runs::RunStore::load(self, user, id)
    }
    fn save(&self, log: &RunLog) -> Result<(), String> {
        crate::runs::RunStore::save(self, log)
    }
    fn stop_all_live(&self, user: Option<&str>) -> usize {
        crate::runs::RunStore::stop_all_live(self, user)
    }
    fn stop_live(&self, id: &str) -> bool {
        crate::runs::RunStore::stop_live(self, id)
    }
}

impl iyakki_agent_api::McpApi for crate::mcp::McpStore {
    fn add(&self, s: StoredServer) -> Result<(), String> {
        crate::mcp::McpStore::add(self, s)
    }
    fn list(&self) -> Vec<StoredServer> {
        crate::mcp::McpStore::list(self)
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        crate::mcp::McpStore::remove(self, name)
    }
}

impl iyakki_agent_api::PackageApi for crate::pkg::Packages {
    fn install(&self, review: &Review, approved: bool, can_shared: bool, can_own: bool) -> Result<Skill, PkgError> {
        crate::pkg::Packages::install(self, review, approved, can_shared, can_own)
    }
    fn list(&self, scope: &Scope) -> Vec<(String, Installed)> {
        crate::pkg::Packages::list(self, scope)
    }
    fn remove(&self, scope: &Scope, name: &str, can_shared: bool, can_own: bool) -> Result<(), PkgError> {
        crate::pkg::Packages::remove(self, scope, name, can_shared, can_own)
    }
    fn review(&self, text: &str, scope: &Scope) -> Result<Review, PkgError> {
        crate::pkg::Packages::review(self, text, scope)
    }
    fn trusted_keys(&self) -> Vec<(String, String)> {
        crate::pkg::Packages::trusted_keys(self)
    }
    fn trust_key(&self, key_hex: &str, label: &str) -> Result<(), PkgError> {
        crate::pkg::Packages::trust_key(self, key_hex, label)
    }
}

impl iyakki_agent_api::JobApi for crate::jobs::Jobs {
    fn dismiss(&self, owner: &str, id: &str) -> Result<(), String> {
        crate::jobs::Jobs::dismiss(self, owner, id)
    }
    fn get(&self, owner: &str, id: &str) -> Result<Job, String> {
        crate::jobs::Jobs::get(self, owner, id)
    }
    fn list(&self, owner: &str) -> Vec<Job> {
        crate::jobs::Jobs::list(self, owner)
    }
    fn tail(&self, owner: &str, id: &str, lines: usize) -> Result<String, String> {
        crate::jobs::Jobs::tail(self, owner, id, lines)
    }
    fn log_len(&self, owner: &str, id: &str) -> u64 {
        crate::jobs::Jobs::log_len(self, owner, id)
    }
    fn log_text(&self, owner: &str, id: &str, max: usize) -> Result<String, String> {
        crate::jobs::Jobs::log_text(self, owner, id, max)
    }
    fn running_count(&self, owner: &str) -> usize {
        crate::jobs::Jobs::running_count(self, owner)
    }
    fn start(&self, owner: &str, spec: &Spec, scope: &dyn Fn(&Path) -> Result<(), String>) -> Result<Job, String> {
        crate::jobs::Jobs::start(self, owner, spec, scope)
    }
    fn stop(&self, owner: &str, id: &str, grace: Duration) -> Result<Job, String> {
        crate::jobs::Jobs::stop(self, owner, id, grace)
    }
    fn take_finished(&self, owner: &str, workspace: &str, who: Channel) -> Vec<Job> {
        crate::jobs::Jobs::take_finished(self, owner, workspace, who)
    }
    fn take_finished_all(&self, owner: &str, who: Channel) -> Vec<Job> {
        crate::jobs::Jobs::take_finished_all(self, owner, who)
    }
    fn visible(&self, owner: &str, workspace: &str) -> Vec<Job> {
        crate::jobs::Jobs::visible(self, owner, workspace)
    }
    fn visible_all(&self, owner: &str) -> Vec<Job> {
        crate::jobs::Jobs::visible_all(self, owner)
    }
    fn wait(&self, owner: &str, id: &str, timeout: Duration) -> Result<Job, String> {
        crate::jobs::Jobs::wait(self, owner, id, timeout)
    }
}


impl iyakki_agent_api::RadioApi for crate::radio::RadioPlayer {
    fn cert_problem(&self) -> Option<CertProblem> {
        crate::radio::RadioPlayer::cert_problem(self)
    }
    fn is_active(&self) -> bool {
        crate::radio::RadioPlayer::is_active(self)
    }
    fn is_paused(&self) -> bool {
        crate::radio::RadioPlayer::is_paused(self)
    }
    fn levels(&self) -> [f32; BANDS] {
        crate::radio::RadioPlayer::levels(self)
    }
    fn pause(&self) -> bool {
        crate::radio::RadioPlayer::pause(self)
    }
    fn play(&self, station: &Station, allow: Arc<dyn Fn(&str) -> bool + Send + Sync>, play_cmd: &str) {
        crate::radio::RadioPlayer::play(self, station, allow, play_cmd)
    }
    fn resume(&self) -> bool {
        crate::radio::RadioPlayer::resume(self)
    }
    fn set_retry_wait(&self, d: Duration) {
        crate::radio::RadioPlayer::set_retry_wait(self, d)
    }
    fn set_streamer(&self, s: Arc<dyn Streamer>) {
        crate::radio::RadioPlayer::set_streamer(self, s)
    }
    fn set_volume(&self, v: u8) {
        crate::radio::RadioPlayer::set_volume(self, v)
    }
    fn state(&self) -> RadioState {
        crate::radio::RadioPlayer::state(self)
    }
    fn stop(&self) {
        crate::radio::RadioPlayer::stop(self)
    }
    fn volume(&self) -> u8 {
        crate::radio::RadioPlayer::volume(self)
    }
}

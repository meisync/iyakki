//! The real implementation of the facets of `iyakki_agent_api::facets_b1` (F-SPLIT-03, bucket b1): each method delegates to the agent module.

use std::path::Path;
use std::sync::{Arc, Mutex};

use iyakki_agent_api::{DirectVoice, HostEnginesFacet, StoryFacet, StoryReader, LinkedPicture, SceneFacet, KubeFeed, KubeFeedFacet, MatrixAccount, MatrixAuth, MatrixRoom, MatrixService, ServicesB1, BrowserHeadersFacet, HeaderRuleStore, HeaderValues, ContextDone, ContextIntentFacet, ContextUndo, AgentFacetsB1, AiMachine, AiProfileFacet, AvBuildFacet, HwAccepted, HwFirmwareFacet, HwOffer, HwView};
use iyakki_shared::aiprofile::Vis;

use crate::agentapi_gen::AgentApiImpl;

struct RealB1;

impl AiMachine for crate::aiprofile::AiProfile {
    fn summary(&self) -> String {
        crate::aiprofile::AiProfile::summary(self)
    }
    fn capable(&self) -> bool {
        crate::aiprofile::AiProfile::capable(self)
    }
    fn unknown(&self) -> bool {
        crate::aiprofile::AiProfile::unknown(self)
    }
    fn fits_line(&self) -> String {
        crate::aiprofile::AiProfile::fits_line(self)
    }
    fn visibility(&self, item_id: &str, show_all: bool) -> Vis {
        match crate::quickinstall::Item::parse(item_id) {
            Some(item) => crate::aiprofile::visibility(self, item, show_all),
            None => Vis::Show,
        }
    }
    fn hidden_items(&self) -> Vec<(String, String)> {
        crate::aiprofile::hidden_items(self).into_iter().map(|(i, r)| (i.id().to_string(), r)).collect()
    }
    fn tier_words(&self) -> String {
        self.tier.words().to_string()
    }
}

impl AiProfileFacet for RealB1 {
    fn machine(&self, data: &Path) -> Arc<dyn AiMachine> {
        Arc::new(crate::aiprofile::this_machine(crate::modelhost::Hosts::new(data).list().len()))
    }
    fn local_capable(&self) -> bool {
        crate::aiprofile::this_machine(0).capable()
    }
    fn this_machine(&self) -> Arc<dyn AiMachine> {
        Arc::new(crate::aiprofile::this_machine(0))
    }
}

impl AvBuildFacet for RealB1 {
    fn consent(&self, map_text: &str) -> Result<Vec<String>, String> {
        let entry = crate::avbuild::lookup(map_text, crate::avbuild::IMAGE_VERSION)?;
        Ok(crate::avbuild::consent_text(&entry, std::env::consts::ARCH))
    }
    fn codecs_policy(&self, user: &str) -> Option<bool> {
        let dir = crate::tenantconfig::enforce::data_dir_from_env()?;
        crate::tenantconfig::enforce::media_codecs_for(&dir, None, user)
    }
}

impl HwFirmwareFacet for RealB1 {
    fn plan_is_empty(&self, data: &Path) -> bool {
        crate::fwextra_cli::plan_here(data).is_empty()
    }
    fn accept(&self, data: &Path, with: &[String], user: &str, now: u64) -> Result<HwAccepted, String> {
        let sel = crate::fwextra_cli::plan_here(data).select(with);
        if sel.files.is_empty() {
            return Err("No missing firmware file was found in the kernel log: nothing to install.".into());
        }
        crate::fwextra::accept(data, &sel, user, now as i64)?;
        Ok(HwAccepted { files: sel.unique().len(), digest16: sel.digest(crate::fwextra::manifest())[..16].to_string() })
    }
    fn view_now(&self, data: &Path, home: &Path) -> HwView {
        let v = crate::fwextra_cli::view_now(data, home);
        HwView {
            lines: v.lines,
            summary: v.summary,
            vendors: v.vendors,
            files: v.files,
            bytes: v.bytes,
            has_nvidia: v.has_nvidia,
            offers: v.offers.into_iter().map(|o| HwOffer { ready: o.ready, needs_click: o.needs_click, line: o.line }).collect(),
            staging: v.staging,
            stick: v.stick,
            link_iface: v.link_iface,
        }
    }
}

fn done_of(d: crate::contextintent::Done) -> ContextDone {
    ContextDone { reply: d.reply, undo: d.undo.map(|u| ContextUndo(Arc::new(u))), touched: d.touched, choices: d.choices, refused: d.refused }
}

impl ContextIntentFacet for RealB1 {
    fn detect(&self, text: &str) -> Option<iyakki_shared::contextintent::Detected> {
        crate::contextintent::detect(text)
    }
    fn apply(&self, root: &Path, person: &str, d: &iyakki_shared::contextintent::Detected, last: Option<&str>, now: i64) -> ContextDone {
        done_of(crate::contextintent::apply(root, person, d, last, now))
    }
    fn apply_choice(&self, root: &Path, person: &str, d: &iyakki_shared::contextintent::Detected, ids: &[String], pick: usize, now: i64) -> ContextDone {
        done_of(crate::contextintent::apply_choice(root, person, d, ids, pick, now))
    }
    fn undo(&self, root: &Path, u: &ContextUndo) -> Result<String, String> {
        match u.0.downcast_ref::<crate::contextintent::Undo>() {
            Some(u) => crate::contextintent::undo(root, u),
            None => Err("This change cannot be taken back here.".into()),
        }
    }
}

impl HeaderRuleStore for crate::browserheaders::Store {
    fn list(&self) -> Vec<iyakki_shared::browserheaders::HeaderRule> {
        crate::browserheaders::Store::list(self)
    }
    fn add(&self, r: iyakki_shared::browserheaders::HeaderRule, value: &str) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        crate::browserheaders::Store::add(self, r, value)
    }
    fn update(&self, id: &str, r: iyakki_shared::browserheaders::HeaderRule, value: Option<&str>) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        crate::browserheaders::Store::update(self, id, r, value)
    }
    fn set_enabled(&self, id: &str, on: bool) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        crate::browserheaders::Store::set_enabled(self, id, on)
    }
    fn delete(&self, id: &str) -> Result<iyakki_shared::browserheaders::HeaderRule, String> {
        crate::browserheaders::Store::delete(self, id)
    }
    fn activate(&self) -> usize {
        let (rules, skipped) = self.compile();
        crate::browserheaders::activate(rules);
        skipped
    }
}

impl BrowserHeadersFacet for RealB1 {
    fn open(&self, path: &Path, values: Arc<dyn HeaderValues>) -> Arc<dyn HeaderRuleStore> {
        Arc::new(crate::browserheaders::Store::new(path, values))
    }
    fn active_here(&self, url: &iyakki_web::Url) -> Vec<String> {
        crate::browserheaders::active_here(url)
    }
    fn provider(&self) -> iyakki_web::headerfetch::Provider {
        crate::browserheaders::provider()
    }
}

// ------------------------------------------------------------------------------------------------ services-bound

struct RealMatrix {
    ms: crate::msg::MessageService,
    svc: Arc<crate::services::Services>,
    user: String,
}

fn room_of(r: crate::msg::RoomRef) -> MatrixRoom {
    MatrixRoom { name: r.name, mirror: r.mirror }
}

fn account_of(a: crate::msg::Account) -> MatrixAccount {
    MatrixAccount { id: a.id, label: a.label, user_id: a.user_id, homeserver: a.homeserver }
}

impl MatrixService for RealMatrix {
    fn accounts(&self) -> Vec<MatrixAccount> {
        self.ms.accounts().list().into_iter().map(account_of).collect()
    }
    fn rooms(&self, account: &str) -> Vec<MatrixRoom> {
        self.ms.accounts().rooms(account).into_iter().map(room_of).collect()
    }
    fn mirror_on(&self, account: &str) -> bool {
        self.ms.accounts().mirror_on(account)
    }
    fn set_mirror(&self, account: &str, on: bool) -> Result<(), String> {
        self.ms.accounts().set_mirror(account, on)
    }
    fn set_room_mirror(&self, account: &str, room: &str, on: bool) -> Result<MatrixRoom, String> {
        self.ms.accounts().set_room_mirror(account, room, on).map(room_of)
    }
    fn unread_by_room(&self, account: &str) -> Vec<(String, usize)> {
        crate::msg::mirror::Mirror { svc: &self.ms, inbox: &self.svc.inbox, user: &self.user }.unread_by_room(account).unwrap_or_default()
    }
    fn add(&self, label: &str, homeserver: &str, auth: MatrixAuth, room_id: &str) -> Result<MatrixAccount, String> {
        let auth = match auth {
            MatrixAuth::Password { user, password } => crate::msg::Auth::password(&user, password),
            MatrixAuth::Token(t) => crate::msg::Auth::token(t),
        };
        self.ms.add_matrix(label, homeserver, auth, room_id).map(account_of)
    }
    fn test_send(&self, account: &str) -> Result<String, String> {
        self.ms.test_send(account, "").map(|r| r.note)
    }
    fn remove(&self, account: &str) -> Result<(), String> {
        self.ms.remove(account)
    }
}

impl ServicesB1 for crate::services::Services {
    fn av_notify_stale(self: &Arc<Self>, user: &str) {
        crate::avbuild::notify_if_stale(self, user)
    }
    fn msg_mirror_tick(self: &Arc<Self>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> bool {
        // pushes that waited at the person's push service are shown first (a cheap no-op without a saved subscription)
        let pushed = crate::browserpush::poll_tick(self, user, audit.clone());
        crate::msg::mirror::tick(self, user, audit) || pushed
    }
    fn rag_refresh_tick(self: &Arc<Self>, user: &str, policy: &iyakki_sec::policy::Policy, audit: iyakki_shared::gitkeys::AuditFn) -> bool {
        crate::ragrefresh::tick(self, user, policy, audit)
    }
    fn msg_offer_tools(self: &Arc<Self>, tools: Arc<dyn iyakki_shared::agent::Tools>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools> {
        crate::msg::tools::offer(tools, self, user, audit)
    }
    fn page_passkeys(self: &Arc<Self>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Option<Arc<Mutex<dyn iyakki_agent_api::PagePasskeyHost>>> {
        Some(crate::passkeys::page_host(self, user, audit))
    }
    fn page_push(self: &Arc<Self>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Option<Arc<Mutex<dyn iyakki_agent_api::PagePushHost>>> {
        Some(crate::browserpush::page_host(self, user, audit))
    }
    fn msg_prompt_rules(self: &Arc<Self>, user: &str) -> String {
        crate::msg::tools::prompt_rules(self, user)
    }
    fn user_tools(self: &Arc<Self>, user: &str, policy: iyakki_sec::policy::Policy, host: Arc<dyn iyakki_core_api::HostApi>, mcp: Vec<iyakki_shared::mcp::ServerSpec>, request: Option<&str>, cwd: Option<&Path>) -> Arc<dyn iyakki_shared::agent::Tools> {
        let mut t = crate::agent::UserTools::new(user, policy, self.clone(), host, mcp);
        if let Some(r) = request {
            t = t.with_request(r);
        }
        if let Some(c) = cwd {
            t = t.with_cwd(c);
        }
        Arc::new(t)
    }
    fn system_speaker(self: &Arc<Self>) -> Box<dyn iyakki_shared::voiceassist::Speaker> {
        Box::new(crate::voiceassist_ports::SystemSpeaker::new(self.clone()))
    }
    fn voice_turn_log(self: &Arc<Self>, user: &str) -> Box<dyn iyakki_shared::voiceassist::TurnLog> {
        Box::new(crate::voiceassist_ports::SessionLog::new(self.clone(), user))
    }
    fn voice_stt(self: &Arc<Self>) -> Box<dyn iyakki_shared::voiceassist::Stt> {
        Box::new(crate::voiceassist_ports::ServicesStt { svc: self.clone() })
    }
    fn ssh_host(self: &Arc<Self>, user: &str) -> Arc<dyn iyakki_agent_api::SshHost> {
        Arc::new(RealSsh(crate::ssh::Env::for_services(self, user)))
    }
    fn ssh_session_tools(self: &Arc<Self>, tools: Arc<dyn iyakki_shared::agent::Tools>, user: &str, allows_host: Arc<dyn Fn(&str) -> bool + Send + Sync>, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn iyakki_shared::agent::Tools> {
        let (svc, env_user) = (self.clone(), user.to_string());
        Arc::new(crate::sshsessiontools::SshSessionTools::new(tools, Arc::new(move || crate::ssh::Env::for_services(&svc, &env_user)), self.dir(), user, true, allows_host, audit))
    }
    fn wake_set_opt_in(self: &Arc<Self>, on: bool) -> Result<bool, String> {
        crate::wakelisten::set_opt_in(self, on)
    }
    fn wake_set_sensitivity(self: &Arc<Self>, sens: u8) -> Result<(), String> {
        crate::wakelisten::set_sensitivity(self, sens)
    }
    fn wake_no_model_text(self: &Arc<Self>) -> Option<String> {
        crate::wakelisten::no_model_text(&crate::wakelisten::load_model(self.dir()))
    }
    fn wake_spawn(self: &Arc<Self>, listener: Option<iyakki_agent_api::WakeListenerBox>) -> Result<Box<dyn iyakki_agent_api::WakeHandle>, String> {
        use crate::wakelisten as wl;
        match wl::load_model(self.dir()) {
            wl::Model::Found { net, header } => {
                let listener = match listener.and_then(|b| b.0.downcast::<wl::Listener>().ok()) {
                    Some(l) => *l,
                    // the experimental second check and the counters are attached by the agent (`with_services`)
                    None => wl::Listener::for_model(Box::new(wl::CaptureMic::new(Arc::new(crate::voicedesk::SystemRecorders), self.clone())), net, &header).with_services(self.clone()),
                };
                let svc = self.clone();
                Ok(Box::new(wl::spawn(listener, Box::new(move || wl::live_from(&svc)), 20)))
            }
            other => Err(wl::no_model_text(&other).unwrap_or_default()),
        }
    }
    fn wake_capture_mic(self: &Arc<Self>) -> Box<dyn iyakki_shared::wakelisten::Mic> {
        // the wake listener gives the microphone up while a page or the Voice tile has it (F-VOICE-60)
        Box::new(iyakki_shared::wakeshare::YieldingMic::new(Box::new(crate::wakelisten::CaptureMic::new(Arc::new(crate::voicedesk::SystemRecorders), self.clone()))))
    }
    fn matrix(self: &Arc<Self>, user: &str, audit: iyakki_shared::gitkeys::AuditFn) -> Arc<dyn MatrixService> {
        Arc::new(RealMatrix { ms: crate::msg::MessageService::for_services(self, user, audit), svc: self.clone(), user: user.to_string() })
    }
}


impl KubeFeedFacet for RealB1 {
    fn start(&self, kube: Arc<dyn iyakki_shared::k8s::Kube>, target: iyakki_shared::k8s::Target, interval: std::time::Duration, log_interval: std::time::Duration) -> Box<dyn KubeFeed> {
        Box::new(crate::k8s::Feed::start(kube, target, interval, log_interval))
    }
}

impl SceneFacet for RealB1 {
    fn asks_for_link(&self, text: &str, scenes: usize) -> Option<(usize, String)> {
        crate::scene::reply::asks_for_link(text, scenes)
    }
    fn host_of(&self, url: &str) -> String {
        crate::scene::reply::host_of(url)
    }
    fn fetch_link(&self, url: &str, web: &dyn iyakki_web::Fetch, allow: &dyn Fn(&str) -> bool, allow_video: bool) -> Result<LinkedPicture, String> {
        crate::scene::reply::fetch_link(url, web, allow, allow_video).map(|f| LinkedPicture { ext: f.ext.to_string(), bytes: f.bytes })
    }
}

impl StoryReader for crate::storykit::Reader {
    fn start(&self, chunks: Vec<iyakki_shared::story::Chunk>, from: usize, open: bool, voice_key: &str, synth: Arc<dyn iyakki_shared::story::Synth>, sink: Arc<dyn iyakki_shared::story::Sink>) {
        crate::storykit::Reader::start(self, chunks, from, open, voice_key, synth, sink)
    }
    fn append(&self, more: Vec<iyakki_shared::story::Chunk>) {
        crate::storykit::Reader::append(self, more)
    }
    fn close_input(&self) {
        crate::storykit::Reader::close_input(self)
    }
    fn pause(&self) {
        crate::storykit::Reader::pause(self)
    }
    fn resume(&self) {
        crate::storykit::Reader::resume(self)
    }
    fn stop(&self) {
        crate::storykit::Reader::stop(self)
    }
    fn seek(&self, chunk: usize) {
        crate::storykit::Reader::seek(self, chunk)
    }
    fn is_active(&self) -> bool {
        crate::storykit::Reader::is_active(self)
    }
    fn announce_holder(&self) {
        crate::storykit::Reader::announce_holder(self)
    }
    fn status(&self) -> iyakki_shared::story::Status {
        crate::storykit::Reader::status(self)
    }
    fn snapshot(&self) -> iyakki_shared::story::Snapshot {
        crate::storykit::Reader::snapshot(self)
    }
    fn timeline_note(&self) -> String {
        crate::storykit::Reader::timeline_note(self)
    }
    fn now_ms(&self) -> f32 {
        crate::storykit::Reader::now_ms(self)
    }
    fn set_time_source(&self, time: Arc<dyn Fn() -> f32 + Send + Sync>) {
        crate::storykit::Reader::set_time_source(self, time)
    }
    fn set_words(&self, on: bool) {
        crate::storykit::Reader::set_words(self, on)
    }
    fn set_speed(&self, speed: f32) {
        crate::storykit::Reader::set_speed(self, speed)
    }
    fn level(&self) -> f32 {
        crate::storykit::Reader::level(self)
    }
    fn sounds(&self) -> (Vec<iyakki_shared::story::Chunk>, Vec<Option<Arc<Vec<u8>>>>) {
        crate::storykit::Reader::sounds(self)
    }
}

impl StoryFacet for RealB1 {
    fn new_reader(&self) -> Arc<dyn StoryReader> {
        Arc::new(crate::storykit::Reader::new())
    }
}

struct RealDirect(crate::hostengines::Spec);

impl DirectVoice for RealDirect {
    fn synth(&self) -> Arc<dyn iyakki_shared::story::Synth> {
        Arc::new(crate::hostengines::DirectSynth)
    }
    fn sink(&self, voice: Option<&str>, lang: Option<&str>, speed: f32) -> Arc<dyn iyakki_shared::story::Sink> {
        let mut spec = self.0.clone();
        if let Some(v) = voice.filter(|v| !v.is_empty()) {
            spec.voice = Some(v.to_string());
        }
        Arc::new(crate::hostengines::DirectSink { spec, lang: lang.map(str::to_string), speed })
    }
}

impl HostEnginesFacet for RealB1 {
    fn direct(&self, engine: &str, program: Option<&str>, voice: Option<&str>) -> Option<Arc<dyn DirectVoice>> {
        if !crate::hostengines::Kind::from_id(engine).is_some_and(crate::hostengines::Kind::can_speak) {
            return None;
        }
        crate::hostengines::Spec::from_parts(engine, program, voice).ok().map(|s| Arc::new(RealDirect(s)) as Arc<dyn DirectVoice>)
    }
    fn test_speak(&self, engine: &str, program: Option<&str>, voice: Option<&str>, lang: &str, speed: f32, play_cmd: &str) -> Result<String, String> {
        let spec = crate::hostengines::Spec::from_parts(engine, program, voice)?;
        crate::hostengines::test_speak(&spec, lang, speed, play_cmd)
    }
}

struct RealSsh(crate::ssh::Env);

impl iyakki_agent_api::SshHost for RealSsh {
    fn connections(&self) -> Vec<iyakki_shared::ssh::Connection> {
        self.0.connections().list()
    }
    fn save_connection(&self, c: iyakki_shared::ssh::Connection, replacing: Option<&str>) -> Result<(), String> {
        self.0.connections().save(c, replacing)
    }
    fn remove_connection(&self, name: &str) -> Result<(), String> {
        self.0.connections().remove(name)
    }
    fn trusted_keys(&self, host: &str, port: u16) -> Vec<iyakki_shared::ssh::HostKey> {
        self.0.known_hosts().for_host(host, port)
    }
    fn trust(&self, host: &str, port: u16, keys: &[iyakki_shared::ssh::HostKey]) -> Result<(), String> {
        self.0.known_hosts().trust(host, port, keys)
    }
    fn forget(&self, host: &str, port: u16) -> Result<usize, String> {
        self.0.known_hosts().forget(host, port)
    }
    fn has_client(&self) -> bool {
        self.0.client().is_some()
    }
    fn probe(&self, host: &str, port: u16) -> Result<Vec<iyakki_shared::ssh::HostKey>, String> {
        let c = self.0.client().ok_or_else(|| format!("No SSH client was found on this machine. {}", crate::ssh::install_hint(std::env::consts::OS)))?;
        crate::ssh::probe(&c, host, port)
    }
    fn run(&self, args: &[String], cwd: &Path, seconds: u64, sink: Option<iyakki_shared::exec::Sink>) -> Result<String, String> {
        crate::ssh::run(&self.0, iyakki_shared::ssh::Mode::Person, "ssh", args, cwd, seconds, &|_, _| Err("this connection names no local files".into()), sink)
    }
    fn interactive(&self, args: &[String], cwd: &Path, cols: usize, rows: usize) -> Result<(Box<dyn iyakki_vt::Link>, Box<dyn std::any::Any + Send>), String> {
        let (link, keep) = crate::ssh::interactive(&self.0, args, cwd, &|_, _| Err("a login names no local files".into()), cols, rows)?;
        Ok((Box::new(link), Box::new(keep)))
    }
    fn generate_key(&self, name: &str, comment: &str, store: &dyn Fn(&str, &str) -> Result<(), String>) -> Result<String, String> {
        crate::ssh::generate_key(&self.0, name, comment, store)
    }
    fn public_key(&self, name: &str) -> Option<String> {
        self.0.public_key(name)
    }
}

impl iyakki_agent_api::WakeHandle for crate::wakelisten::WakeHandle {
    fn indicator(&self) -> bool {
        crate::wakelisten::WakeHandle::indicator(self)
    }
    fn take_wake(&self) -> bool {
        crate::wakelisten::WakeHandle::take_wake(self)
    }
    fn set_paused(&self, paused: bool) {
        crate::wakelisten::WakeHandle::set_paused(self, paused)
    }
    fn note(&self) -> String {
        crate::wakelisten::WakeHandle::note(self)
    }
}


impl AgentFacetsB1 for AgentApiImpl {
    fn browser_media_for_user(&self, user: &str) -> Option<Arc<dyn iyakki_agent_api::BrowserMediaHandle>> {
        crate::browsermedia::for_user(user).map(|m| m as Arc<dyn iyakki_agent_api::BrowserMediaHandle>)
    }
    fn wake_request_session(&self) {
        crate::wakelisten::request_session()
    }
    fn sound_hold_registry(&self) -> Arc<dyn iyakki_shared::soundhold::HoldRegistry> {
        crate::soundhold::global()
    }
    fn host_engines(&self) -> &dyn HostEnginesFacet {
        &RealB1
    }
    fn story(&self) -> &dyn StoryFacet {
        &RealB1
    }
    fn scene(&self) -> &dyn SceneFacet {
        &RealB1
    }
    fn mic_glyph(&self) -> iyakki_shared::voiceassist::MicGlyph {
        crate::voiceassist::mic_glyph()
    }
    fn kube_feed(&self) -> &dyn KubeFeedFacet {
        &RealB1
    }
    fn matrix_check_homeserver(&self, addr: &str) -> Result<String, String> {
        crate::msg::matrix::check_homeserver(addr)
    }
    fn browser_headers(&self) -> &dyn BrowserHeadersFacet {
        &RealB1
    }
    fn context_intent(&self) -> &dyn ContextIntentFacet {
        &RealB1
    }
    fn ai_profile(&self) -> &dyn AiProfileFacet {
        &RealB1
    }
    fn av_build(&self) -> &dyn AvBuildFacet {
        &RealB1
    }
    fn hw_firmware(&self) -> &dyn HwFirmwareFacet {
        &RealB1
    }
}

#[cfg(test)]
#[path = "facets_b1_tests.rs"]
mod tests;

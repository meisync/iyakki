//! The registry of the Settings pages: for each page, which code handles its keys, draws it and gives its buttons. One row per page, grouped
//! by lane (docs/development/SETTINGS-EXTRACTION-PLAN.md); a lane edits only the rows under its own banner.

use super::*;

impl SettingsApp {
    /// A key on the page: the page that is open takes it.
    pub(super) fn page_key(&mut self, other: &Key, cx: &AppCx) {
        match self.tab() {
            // ---- lane ST-A: Account, people, system
                Tab::Overview => self.handle_overview(other, cx),
                Tab::Vault => self.handle_vault(other, cx),
                Tab::Backup => self.handle_backup(other, cx),
                Tab::Storage => self.handle_storage(other, cx),
                Tab::Account => self.handle_account(other, cx),
                Tab::Memory => self.handle_memory(other, cx),
                Tab::Time => self.handle_time(other, cx),
                Tab::Notifications => self.handle_notifs(other, cx),
                Tab::Cloud => self.handle_cloud(other, cx),
                Tab::CloudRoles => self.handle_cloudroles(other, cx),
                Tab::Battery => self.handle_battery(other, cx),
                Tab::Power => self.handle_power(other, cx),
                Tab::Radio => self.handle_radio(other, cx),
                Tab::EncHome => self.handle_ench(other, cx),
                Tab::Computer => self.handle_computer(other, cx),
                Tab::HostName => self.handle_host_name(other, cx),
                Tab::SessionLog => self.handle_crash(other, cx),
                Tab::Startup => self.handle_startup(other, cx),
                Tab::Install => self.handle_install(other, cx),
                Tab::Updates => self.handle_updates(other, cx),
            // ---- lane ST-B: Apps and AI
                Tab::Models => self.handle_models(other, cx),
                Tab::Agents => self.handle_agents(other, cx),
                Tab::Rag => self.handle_rag(other, cx),
                Tab::Assistant => self.handle_assistant(other, cx),
                Tab::ModelRouter => self.handle_router(other, cx),
                Tab::GeminiVoice => self.handle_geminivoice(other, cx),
                Tab::Alexa => self.handle_alexa(other, cx),
                Tab::Skills => self.handle_skills(other, cx),
                Tab::Voice => self.handle_voice(other, cx),
                Tab::SecretProviders => self.handle_secret_providers(other, cx),
                Tab::HostAccess => self.handle_host(other, cx),
                Tab::AppAccess => self.handle_appaccess(other, cx),
                Tab::AppEvents => self.handle_appevents(other, cx),
            // ---- lane ST-C: Browser, network and sharing
                Tab::Fleet => self.handle_fleet(other, cx),
                Tab::Network => self.handle_hw(other, cx),
                Tab::Sound => self.handle_sound(other, cx),
                Tab::Browser => self.handle_browser(other, cx),
                Tab::Shares => self.handle_shares(other, cx),
                Tab::Firewall => self.handle_firewall(other, cx),
                Tab::SshTunnels => self.handle_tun(other, cx),
                Tab::Domains => self.handle_dom(other, cx),
                Tab::Vpn => self.handle_vpn(other, cx),
                Tab::Remote => self.handle_remote(other, cx),
            // ---- lane ST-D: Display, sound, devices, developer
                Tab::Bluetooth => {}
                Tab::Display => self.handle_display(other, cx),
                Tab::HomeLayout => self.handle_homelayout(other, cx),
                Tab::Views => self.handle_views(other, cx),
                Tab::DevCluster => self.handle_devcluster(other, cx),
                Tab::Displays => self.handle_displays(other, cx),
                Tab::ScreenSaver => self.handle_screensaver(other, cx),
                Tab::Keyboard => self.handle_keyboard(other, cx),
                Tab::SystemSounds => self.handle_sysnd(other, cx),
                Tab::Accessibility => self.handle_access(other, cx),
                Tab::Appearance => self.handle_appearance(other, cx),
                Tab::Developer => self.handle_developer(other, cx),
                Tab::GitKeys => self.handle_git(other, cx),
        }
    }

    /// Draw the open page into `body`.
    pub(super) fn render_page(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx) {
        match self.tab() {
            // ---- lane ST-A: Account, people, system
            Tab::Overview => self.render_overview(g, body, s, cx),
            Tab::Vault => self.render_vault(g, body, s, cx),
            Tab::Backup => self.render_backup(g, body, s, cx),
            Tab::Storage => self.render_storage(g, body, s, cx),
            Tab::SecretProviders => self.render_secret_providers(g, body, s, cx),
            Tab::Account => self.render_account(g, body, s, cx),
            Tab::Memory => self.render_memory(g, body, s, cx),
            Tab::Time => self.render_time(g, body, s, cx),
            Tab::Notifications => self.render_notifs(g, body, s, cx),
            Tab::Cloud => self.render_cloud(g, body, s, cx),
            Tab::CloudRoles => self.render_cloudroles(g, body, s, cx),
            Tab::Battery => self.render_battery(g, body, s, cx),
            Tab::Power => self.render_power(g, body, s, cx),
            Tab::Radio => self.render_radio(g, body, s, cx),
            Tab::EncHome => self.render_ench(g, body, s, cx),
            Tab::Computer => self.render_computer(g, body, s, cx),
            Tab::HostName => self.render_host_name(g, body, s, cx),
            Tab::SessionLog => self.render_crash(g, body, s, cx),
            Tab::Startup => self.render_startup(g, body, s, cx),
            Tab::Install => self.render_install(g, body, s),
            Tab::Updates => self.render_updates(g, body, s, cx),
            // ---- lane ST-B: Apps and AI
            Tab::Models => self.render_models(g, body, s, cx),
            Tab::Agents => self.render_agents(g, body, s, cx),
            Tab::Rag => self.render_rag(g, body, s, cx),
            Tab::Assistant => self.render_assistant(g, body, s, cx),
            Tab::ModelRouter => self.render_router(g, body, s, cx),
            Tab::GeminiVoice => self.render_geminivoice(g, body, s, cx),
            Tab::Alexa => self.render_alexa(g, body, s, cx),
            Tab::Skills => self.render_skills(g, body, s, cx),
            Tab::Voice => self.render_voice(g, body, s, cx),
            Tab::HostAccess => self.render_host(g, body, s, cx),
            Tab::AppAccess => self.render_appaccess(g, body, s, cx),
            Tab::AppEvents => self.render_appevents(g, body, s, cx),
            // ---- lane ST-C: Browser, network and sharing
            Tab::Fleet => self.render_fleet(g, body, s, cx),
            Tab::Network => self.render_hw(g, body, s, cx),
            Tab::Sound => self.render_sound(g, body, s, cx),
            Tab::Browser => self.render_browser(g, body, s, cx),
            Tab::Shares => self.render_shares(g, body, s, cx),
            Tab::Firewall => self.render_firewall(g, body, s, cx),
            Tab::SshTunnels => self.render_tun(g, body, s, cx),
            Tab::Domains => self.render_dom(g, body, s, cx),
            Tab::Vpn => self.render_vpn(g, body, s, cx),
            Tab::Remote => self.render_remote(g, body, s, cx),
            // ---- lane ST-D: Display, sound, devices, developer
            Tab::Display => self.render_display(g, body, s, cx),
            Tab::Bluetooth => self.bt.render(g, body, s),
            Tab::HomeLayout => self.render_homelayout(g, body, s, cx),
            Tab::Views => self.render_views(g, body, s, cx),
            Tab::DevCluster => self.render_devcluster(g, body, s, cx),
            Tab::Displays => self.render_displays(g, body, s, cx),
            Tab::ScreenSaver => self.render_screensaver(g, body, s, cx),
            Tab::Keyboard => self.render_keyboard(g, body, s, cx),
            Tab::SystemSounds => self.render_sysnd(g, body, s, cx),
            Tab::Accessibility => self.render_access(g, body, s, cx),
            Tab::Appearance => self.render_appearance(g, body, s, cx),
            Tab::Developer => self.render_developer(g, body, s, cx),
            Tab::GitKeys => self.render_git(g, body, s, cx),
        }
    }

    /// The action buttons under the open page.
    pub(super) fn page_buttons(&self, admin: bool) -> Vec<Btn> {
        let (acc, mut_, err) = (ACCENT, TEXT, ERR);
        match (self.tab(), admin) {
            // ---- lane ST-A: Account, people, system
            (Tab::Overview, _) => vec![],
            (Tab::Vault, true) if self.section == 0 => vec![("New secret", acc, 'n'), ("Delete", err, 'd'), ("Lock now", mut_, 'l'), ("Key file", mut_, 'f')],
            (Tab::Vault, _) => vec![("New secret", acc, 'n'), ("Delete", err, 'd'), ("Reset vault", mut_, 'R')],
            (Tab::Storage, _) if self.storage.virtual_disks.is_some() => vec![("New virtual disk", acc, 'n'), ("Delete it", err, 'x'), ("Make data disk", ERR, 'm'), ("Rescan", TEXT, 'r')],
            (Tab::Storage, _) => vec![("Make data disk", ERR, 'm'), ("Rescan", TEXT, 'r'), ("Reboot", TEXT, 'b')],
            (Tab::SecretProviders, _) => vec![("Add provider", acc, 'a'), ("Test", mut_, 't'), ("Delete", err, 'd')],
            (Tab::Account, _) => vec![("Change password", acc, 'p'), ("Bigger text", mut_, 'u'), ("Smaller text", mut_, 'U')],
            (Tab::Memory, _) => self.memory_buttons(),
            (Tab::Time, _) => self.time_buttons(),
            (Tab::Notifications, _) => self.notifs_buttons(),
            (Tab::Cloud, _) => vec![("Change mode", ACCENT, 'c'), ("Show all AI tools", mut_, 'a')],
            (Tab::CloudRoles, _) => self.roles_buttons(),
            (Tab::Battery, _) => self.battery_buttons(),
            (Tab::Power, _) => self.power_buttons(),
            (Tab::Radio, _) => self.radio_buttons(),
            (Tab::EncHome, _) => self.ench_buttons(),
            (Tab::Computer, _) => self.computer_buttons(),
            (Tab::HostName, _) => self.host_name_buttons(),
            (Tab::SessionLog, _) => self.crash_buttons(),
            (Tab::Startup, _) => self.startup_buttons(),
            (Tab::Backup, _) => self.backup_buttons(),
            (Tab::Install, _) => self.install_buttons(),
            (Tab::Updates, _) => self.updates_buttons(),
            // ---- lane ST-B: Apps and AI
            (Tab::Models, true) => vec![("Add", acc, 'a'), ("NVIDIA NIM", acc, 'N'), ("Google Gemini", acc, 'g'), ("Gemini consent", mut_, 'C'), ("Detect", acc, 'f'), ("Mac / GPU host", acc, 'H'), ("Edit", mut_, 'e'), ("Delete", err, 'd'), ("Test", mut_, 't'), ("Lemonade GPU/CPU", mut_, 'G'), ("Chat model", mut_, 'm'), ("Vision model", mut_, 'v'), ("Embeddings", mut_, 'b'), ("Set key", mut_, 'k'), ("On/Off", mut_, 'x'), ("Agent price", mut_, 'p'), ("Tool routing", mut_, 'T'), ("Test chat", mut_, 'c')],
            (Tab::Models, false) => vec![("Test", mut_, 't'), ("Test chat", mut_, 'c'), ("Gemini consent", mut_, 'C')],
            (Tab::Agents, _) => vec![("Detect", acc, 'd'), ("Install", mut_, 'i'), ("Login", OK, 'l'), ("Logout", mut_, 'o'), ("API key", mut_, 'k'), ("Model", mut_, 'm'), ("Auto", mut_, 'a'), ("Use for new chats", mut_, 'u'), ("Test", mut_, 't'), ("Hide/show", mut_, 'x'), ("Add custom", mut_, 'n'), ("Edit", mut_, 'e'), ("Remove", err, 'R')],
            (Tab::Rag, true) => vec![("Add backend", acc, 'a'), ("Edit", mut_, 'e'), ("Delete", err, 'd'), ("Test", mut_, 't'), ("On/Off", mut_, 'x'), ("Reindex", mut_, 'r'), ("Ingest docs", mut_, 'i')],
            (Tab::Rag, false) => vec![("Reindex", mut_, 'r'), ("Ingest my docs", acc, 'i'), ("Test backend", mut_, 't')],
            (Tab::Voice, true) => vec![
                ("Speech-to-text", acc, 's'), ("Text-to-speech", acc, 'v'), ("Commands", mut_, 'c'), ("Auto-send", mut_, 'a'), ("Speak replies", mut_, 'r'), ("Dictation pause", mut_, 'u'), ("Pause length", mut_, 'U'), ("Keep listening", mut_, 'K'), ("Correct wording", mut_, 'W'), ("Clear STT", err, 'S'), ("Clear TTS", err, 'V'),
                ("Test mic", mut_, 'm'), ("Test speaker", mut_, 'p'), ("Test STT", mut_, 't'),
            ],
            (Tab::Voice, false) => vec![("Test mic", mut_, 'm'), ("Test speaker", mut_, 'p'), ("Test STT", mut_, 't')],
            (Tab::Skills, _) => {
                let mut v = vec![("From clipboard", acc, 'c'), ("Quick create", mut_, 'a'), ("Import file", mut_, 'i'), ("Delete", err, 'd')];
                if self.share_skills {
                    v.insert(1, ("Share clipboard", mut_, 'C'));
                }
                v
            }
            (Tab::HostAccess, _) if hf_on() => Self::host_buttons(),
            (Tab::HostAccess, _) => vec![],
            (Tab::AppAccess, _) => self.appaccess_buttons(),
            (Tab::AppEvents, _) => self.appevents_buttons(),
            (Tab::Assistant, _) => self.assistant_buttons(),
            (Tab::ModelRouter, _) => self.router_buttons(),
            (Tab::GeminiVoice, _) => self.geminivoice_buttons(),
            (Tab::Alexa, _) => self.alexa_buttons(),
            // ---- lane ST-C: Browser, network and sharing
            (Tab::Network, _) if self.share.open => self.share_buttons(),
            (Tab::Network, _) => vec![("Share internet", mut_, 'i'), ("Turn on/off", acc, 'e'), ("Join WiFi", acc, 'w'), ("Sign-in page", mut_, 'o'), ("Other adapter", mut_, 'v'), ("Private MAC", mut_, 'm'), ("Apply MAC now", mut_, 'a'), ("Device name", mut_, 'n'), ("Look again", mut_, 'r')],
            // the Bluetooth page draws its own buttons on its rows
            (Tab::Browser, _) if self.bh.open => self.bh_buttons(),
            (Tab::Browser, _) => vec![("Custom headers", acc, 'H'), ("Forget sites", err, 'f'), ("Protect data", acc, 'P'), ("Reset data", err, 'X'), ("Known-bad", acc, 'I'), ("Video quality", acc, 'q'), ("Decoders", acc, 'y'), ("Add bad site", acc, 'J'), ("Remove bad site", err, 'O')],
            (Tab::Fleet, _) => self.fleet_buttons(),
            (Tab::Shares, _) => self.shares_buttons(),
            (Tab::Firewall, _) => self.fw_buttons(),
            (Tab::SshTunnels, _) => self.tun_buttons(),
            (Tab::Domains, _) => self.dom_buttons(),
            (Tab::Vpn, _) => self.vpn_buttons(),
            (Tab::Remote, _) => self.remote_buttons(),
            // ---- lane ST-D: Display, sound, devices, developer
            (Tab::Bluetooth, _) => vec![],
            (Tab::Display, _) => vec![("Brighter", acc, '+'), ("Dimmer", mut_, '-'), ("Look again", mut_, 'r')],
            (Tab::Sound, _) => vec![("Output", acc, 'o'), ("Test sound", acc, 't'), ("Louder", mut_, '+'), ("Quieter", mut_, '-'), ("Mute", mut_, 'm'), ("Mix sounds", mut_, 'M'), ("Advanced", mut_, 'A'), ("Look again", mut_, 'r')],
            (Tab::HomeLayout, _) => self.homelayout_buttons(),
            (Tab::Views, _) => self.views_buttons(),
            (Tab::DevCluster, _) => vec![("Test", acc, 't'), ("My default", mut_, 'd'), ("Workspace default", mut_, 'w'), ("Next workspace", mut_, 'k'), ("Hide/Show", mut_, 'h'), ("Disconnect", err, 'x'), ("Registry", mut_, 'g'), ("Platform services", acc, 'P'), ("Clusters", acc, 'C'), ("Add vault profile", mut_, 'v'), ("Remove it", err, 'V'), ("Look again", mut_, 'r')],
            (Tab::Displays, _) => self.displays_buttons(),
            (Tab::ScreenSaver, _) => self.screensaver_buttons(),
            (Tab::Keyboard, _) => self.keyboard_buttons(),
            (Tab::SystemSounds, _) => self.sysnd_buttons(),
            (Tab::Accessibility, _) => self.access_buttons(),
            (Tab::Appearance, _) => self.appearance_buttons(),
            (Tab::Developer, _) => self.developer_buttons(),
            (Tab::GitKeys, _) => self.git_buttons(),
        }
    }
}

impl SettingsApp {
    pub(super) fn tab_for_name(name: &str) -> Option<Tab> {
        match name {
            "overview" => Some(Tab::Overview),
            "network" => Some(Tab::Network),
            "bluetooth" => Some(Tab::Bluetooth),
            "display" => Some(Tab::Display),
            "sound" => Some(Tab::Sound),
            "keyboard" => Some(Tab::Keyboard),
            "systemsounds" => Some(Tab::SystemSounds),
            "storage" => Some(Tab::Storage),
            "time" => Some(Tab::Time),
            "battery" => Some(Tab::Battery),
            "assistant" | "history" | "forget" | "stall" => Some(Tab::Assistant),
            "router" | "modelrouter" | "models-router" => Some(Tab::ModelRouter),
            "fleet" | "pair" | "pairgroup" => Some(Tab::Fleet),
            "geminivoice" | "gemini-voice" | "live" => Some(Tab::GeminiVoice),
            "alexa" | "amazon" => Some(Tab::Alexa),
            "power" => Some(Tab::Power),
            "radio" => Some(Tab::Radio),
            "accessibility" | "screenreader" | "access" => Some(Tab::Accessibility),
            "appearance" | "look" | "looks" | "wallpaper" | "dock" | "icons" => Some(Tab::Appearance),
            "encryptedhome" | "encrypted" | "encryption" | "enchome" => Some(Tab::EncHome),
            "computer" | "thiscomputer" | "computers" => Some(Tab::Computer),
            "hostname" => Some(Tab::HostName),
            "crashlog" | "sessionlog" => Some(Tab::SessionLog),
            "startup" => Some(Tab::Startup),
            "updates" | "update" => Some(Tab::Updates),
            "models" => Some(Tab::Models),
            "homelayout" => Some(Tab::HomeLayout),
            "views" | "desktopviews" => Some(Tab::Views),
            "cloud" => Some(Tab::Cloud),
            "cloudaccess" | "roles" => Some(Tab::CloudRoles),
            "developer" => Some(Tab::Developer),
            "git" => Some(Tab::GitKeys),
            "shares" => Some(Tab::Shares),
            "backup" | "restore" => Some(Tab::Backup),
            "firewall" => Some(Tab::Firewall),
            "vpn" => Some(Tab::Vpn),
            "remote" | "webaccess" => Some(Tab::Remote),
            "vault" => Some(Tab::Vault),
            "browser" => Some(Tab::Browser),
            "devcluster" => Some(Tab::DevCluster),
            "notifications" => Some(Tab::Notifications),
            "appaccess" | "permissions" => Some(Tab::AppAccess),
            "appevents" | "events" => Some(Tab::AppEvents),
            _ => None,
        }
    }

    /// Go to the page a `tab:<name>` item names (the part of `open_item` that needs no shell context, so a page can ask for it from a port).
    pub(super) fn open_tab_item(&mut self, item: &str) -> bool {
        let Some(name) = item.strip_prefix("tab:") else { return false };
        match Self::tab_for_name(name).and_then(|t| self.tabs.iter().position(|x| *x == t)) {
            Some(i) => {
                self.tab = i;
                true
            }
            None => false,
        }
    }
}

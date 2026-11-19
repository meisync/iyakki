//! The account's tool chain, shared by the chat and the voice session (F-GEMVOICE-15).
//!
//! Every wrapper that belongs to the ACCOUNT (the Settings tools, SSH, VPN, Bluetooth, cameras, documents, the model router, the fleet and so on) is
//! put on top of a base tool set here, once. The chat calls [`account_chain`] with its own base (which may already carry the open project, the dashboard
//! and the flows); the voice session calls it with the plain account base. Both therefore offer the same tools, with the same grants and the
//! same approval cards. What stays chat-only is only what needs a chat's own open state (the open project, flow and dashboard wrappers and the
//! chat's pending-work ledger): see [`CHAT_ONLY`].

use std::sync::Arc;

use iyakki_shared::agent::Tools;

use super::AppCx;

/// What the chain needs to know that is not the account: the open workspace (name, root) and the external agent chosen in a chat.
#[derive(Clone, Debug, Default)]
pub struct ChainScope {
    pub project: Option<(String, std::path::PathBuf)>,
    pub ext: Option<String>,
}

/// The wrappers that only a chat can build, because they need the chat's own open state. A voice session does not have them.
pub const CHAT_ONLY: [&str; 4] = ["the open workspace's board tools", "the dashboard tools", "the flow tools", "the task tools"];

/// The job tools and the pending-work ledger, outermost on a chat and on a voice session alike (F-GEMVOICE-40): `wait_for`, `pending_list`
/// and the jobs a spoken request may start, with the grants and approvals of the chat. `workspace` is the open project's name (a voice
/// session has none: its work belongs to no workspace) and `tasks` the chat's task list (none in a voice session).
pub fn session_state_tools(cx: &AppCx, tools_arc: Arc<dyn Tools>, workdir: Option<std::path::PathBuf>, workspace: Option<(&str, &std::path::Path)>, tasks: Option<iyakki_agent_api::pending::TaskList>) -> Arc<dyn Tools> {
    let cwd = workdir.unwrap_or_else(|| std::path::PathBuf::from(format!("/home/{}", cx.session.user)));
    let cwd = if cwd.is_dir() { cwd } else { std::path::PathBuf::from("/") };
    let tools_arc: Arc<dyn Tools> = if cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full {
        cx.b4().wrap_job_tools(tools_arc, &cx.session.user, cx.session.policy.clone(), &cwd, workspace)
    } else {
        tools_arc
    };
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    let ws = workspace.map(|w| w.0.to_string()).unwrap_or_default();
    cx.pendingapi().wrap_pending_tools(tools_arc, &cx.session.user, &ws, tasks, cx.session.policy.clone(), &cwd, Arc::new(move |k, d| audit.record(&user, k, d)))
}

/// Put every account-level wrapper on `tools_arc`.
pub fn account_chain(cx: &AppCx, tools_arc: Arc<dyn Tools>, scope: &ChainScope) -> Arc<dyn Tools> {
    let env = {
        let audit = cx.audit.clone();
        let audit2 = cx.audit.clone();
        iyakki_agent_api::ChainEnv {
            user: cx.session.user.clone(),
            role: cx.session.role,
            policy: cx.session.policy.clone(),
            audit: Arc::new(move |u, a, d| audit.record(u, a, d)),
            audit_recent: Arc::new(move |n| audit2.recent(n)),
        }
    };
        // the interactive SSH login of the CLI tile: the list is free, a login asks every time; the person types in the session (full terminal only)
        let tools_arc: Arc<dyn Tools> = if cx.session.policy.cli == iyakki_sec::policy::CliLevel::Full {
            let (audit, user, svc, policy) = (cx.audit.clone(), cx.session.user.clone(), cx.services.clone(), cx.session.policy.clone());
            svc.ssh_session_tools(tools_arc, &cx.session.user, Arc::new(move |h| policy.allows_host(h)), Arc::new(move |a, d| audit.record(&user, a, d)))
        } else {
            tools_arc
        };
        // SSH tunnels and domain names (administrators), the address bar's short words, the browser's graphics mode (administrators)
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_net_browser_tools(tools_arc, &env);
        // the SFTP server: the status is free, every change asks (administrators only: it is a device setting; the accounts are the shell's own store)
        let tools_arc: Arc<dyn Tools> = {
            let (s1, s2) = (cx.store.clone(), cx.store.clone());
            cx.b4().wrap_sftp_tools(
                tools_arc,
                &env,
                iyakki_agent_api::SftpAccounts {
                    login: Arc::new(move |n, p| s1.lock().unwrap_or_else(|e| e.into_inner()).authenticate(n, p).map(|s| s.policy).map_err(|e| e.to_string())),
                    lookup: Arc::new(move |n| {
                        let mut s = s2.lock().unwrap_or_else(|e| e.into_inner());
                        s.refresh();
                        s.get(n).map(|u| u.policy())
                    }),
                },
            )
        };
        // remote files (SFTP / FTPS): listing is free, connecting, moving and changing ask; the person's own manager, shared with the REMOTE FILES tile
        let tools_arc: Arc<dyn Tools> = if cx.session.policy.allows_app(iyakki_sec::policy::App::Remote) {
            crate::apps::remotefiles::manager_for(cx).wrap_tools(tools_arc)
        } else {
            tools_arc
        };
        // live cameras: listing is free, a snapshot, adding, removing and sharing ask; the password goes through a masked form to the vault
        let tools_arc: Arc<dyn Tools> = if cx.session.policy.allows_app(iyakki_sec::policy::App::Cameras) {
            crate::apps::cameras::tools_for(cx, tools_arc)
        } else {
            tools_arc
        };
        // shared folders: the list is free, every change asks; the manager enforces the folder rule (administrator any folder of the disks, others their home)
        let tools_arc: Arc<dyn Tools> = {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            let admin = cx.session.role == iyakki_sec::policy::Role::Admin;
            let a2 = user.clone();
            cx.b0().share_tools(tools_arc, admin, cx.session.policy.clone(), &user, Arc::new(move |e, d| audit.record(&a2, e, d)))
        };
        // updates, building an image app, the cluster explorer's custom resources (read-only), Linux sandboxes
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_run_a_tools(tools_arc, &env);
        // an archived sandbox root as an image app in the dev registry: asks every time, administrators only
        let tools_arc: Arc<dyn Tools> = {
            let audit = cx.audit.clone();
            let user = cx.session.user.clone();
            cx.agent.tool_wraps().linux_appimage_tools(tools_arc, cx.services.clone().as_any_arc(), cx.session.policy.clone(), cx.session.user.clone(), Arc::new(move |a, d| audit.record(&user, a, d)))
        };
        // skills, the MCP servers of the MCP tile, a Linux app root as a pod in the dev cluster: lists are free, changes ask every time
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_run_b_tools(tools_arc, &env, cx.mcp_servers.clone());
        // setup wizards of installed apps: listing and answering are free, running the checks and migrations asks every time
        let tools_arc: Arc<dyn Tools> = {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            cx.agent.tool_wraps_b3().wrap_wizard_tools(tools_arc, cx.b3().wizard_backend(&cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d))))
        };
        // the model router (experimental): reads and router_cancel are free, starting a run and every change ask every time (hard asks)
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_router_tools(
            tools_arc,
            &env,
            iyakki_agent_api::RouterParts {
                project_root: scope.project.as_ref().map(|(_, root)| root.clone()),
                scope: scope.project.as_ref().map(|(name, _)| format!("workspace:{name}")).unwrap_or_default(),
                camera_hub: crate::apps::cameras::hub_for(cx),
                camera_who: Box::new(crate::apps::cameras::who_for(cx)),
                os: cx.os.clone(),
            },
        );
        // fleet (experimental): fleet_status is free, pairing, leaving, sending work and accepting work ask every time (hard asks)
        let tools_arc = {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            cx.services.wrap_fleet_tools(tools_arc, cx.os.clone(), cx.os.fleet(), Arc::new(move |a, d| audit.record(&user, a, d)))
        };
        // DOCTOR, the log of the last session, the lock screen
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_status_tools(tools_arc, &env);
        // the Settings pages that every feature must also reach by chat (docs/development/CHAT-REACHABILITY.md): views, notifications, screen saver,
        // Apps access, Connect a client, keyboard, time, browser headers, Home layout; reads are free, every change asks
        let tools_arc: Arc<dyn Tools> = super::chat_settings::wrap(cx, tools_arc);
        // client devices (experimental): the list is free, a permission change or a revoke asks every time; administrators only
        let tools_arc: Arc<dyn Tools> = {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            let admin = cx.session.role == iyakki_sec::policy::Role::Admin;
            cx.agent.tool_wraps_b3().wrap_device_tools(tools_arc, cx.data_dir(), &cx.session.user, admin, Arc::new(move |a, d| audit.record(&user, a, d)))
        };
        // the standing restart grant of developer mode: status is free, a change asks every time; administrators only
        let tools_arc: Arc<dyn Tools> = {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            cx.agent.tool_wraps_b3().wrap_restart_tools(tools_arc, cx.data_dir(), &cx.session.policy, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
        };
        // the encrypted home folder (status and plan are free, everything else asks), accessibility, the Browser's accessibility choices
        let tools_arc: Arc<dyn Tools> = {
            cx.b4().wrap_home_access_tools(tools_arc, &env)
        };
        // encrypted drives (unlock and lock ask every time; not for guests), format and resize a partition (hard asks, administrators)
        let tools_arc: Arc<dyn Tools> = {
            let user = cx.session.user.clone();
            let home = cx.agent.scope_home_root(&user, std::path::Path::new("/var/lib/iyakki")).display().to_string();
            let (uid, gid) = cx.agent.disks_account_owner(&user, "/etc/passwd").unwrap_or((0, 0));
            cx.b4().wrap_disks_tools(tools_arc, &env, iyakki_shared::disks::Actor { user, role: cx.session.role, uid, gid, home })
        };
        // system components, shared builds, PDF files and the PDF-output switch, WASM tiles, object storage, model providers, Ollama, document
        // retrieval, quick install removal, where Lemonade runs, this computer, dictation wording and My IYAKKI (what is free and what asks is in each tool)
        let tools_arc: Arc<dyn Tools> = {
            let downloads = super::browser::BrowserApp::downloads_dir(&cx).unwrap_or_else(|_| cx.data_dir().join("downloads"));
            cx.b4().wrap_system_tools(tools_arc, &env, iyakki_agent_api::PdfParts { ext: scope.ext.clone(), web: cx.web.clone(), downloads }, cx.chat.clone())
        };
        // messaging (once an account exists), browser Web Push and passkeys: every send, save or use asks on a card, every time
        let tools_arc: Arc<dyn Tools> = {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            cx.services.msg_offer_tools(tools_arc, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
        };
        // the shared vault's cryptography (administrators), the cloud tenant space (free), how tools are offered (switching asks)
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_vault_routing_tools(tools_arc, &env);
        // clone a repository, register an AI IDE workspace: both ask every time (the account needs the Git tile or the AI IDE)
        let tools_arc: Arc<dyn Tools> = if iyakki_shared::gitworktools::offered(&cx.session.policy) {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            cx.b3().wrap_git_work_tools(tools_arc, &cx.session.user, &cx.session.policy, Arc::new(move |a, d| audit.record(&user, a, d)))
        } else {
            tools_arc
        };
        // VPN profiles: the list is free, every change asks (administrators only: routes are device settings)
        let tools_arc: Arc<dyn Tools> = if cx.session.policy.manage_settings {
            let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
            cx.b3().wrap_vpn_tools(tools_arc, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
        } else {
            tools_arc
        };
        // Bluetooth keyboards, mice and game pads: the list is free, connecting, disconnecting and forgetting ask (administrators only)
        let tools_arc: Arc<dyn Tools> = cx.b4().wrap_bluetooth_tools(tools_arc, &env);
    // My IYAKKI on a cloud machine: sending microphone sound off the device. Status is free, the switch is a hard ask (administrators only)
    let tools_arc: Arc<dyn Tools> = {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        cx.services.wrap_cloud_stream_tools(tools_arc, Arc::new(move |a, d| audit.record(&user, a, d)), cx.session.policy.manage_settings)
    };
    tools_arc
}

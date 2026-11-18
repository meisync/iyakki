# Core split, lane B: plans for the remaining stateful handles (alpha.6)

Measured on 2026-11-17 at 274 UI uses. Each piece: the methods the UI really calls (not the module's whole API), the data types that must move
to `iyakki-shared` first, then a trait in `iyakki-agent-api` (`facets_s6b.rs`), the real implementation in `iyakki-agent`, and a
`Services::..._facet()` or `host_probe()` accessor. Done pieces: flows, voicedesk, extagent (run + list), update centre, git trio, authcodes.

## platform (10 uses through the `agent_handles_b2::platform` seam)

UI files: `platform_services.rs` (2518 lines, the panel), `ide_git.rs`, `ide_git_connect.rs`, `ide.rs`, `chat.rs`, `git_wizard.rs`, `settings_devcluster.rs`.
Methods the UI calls on `Platform` (about 21): apply, apply_with, backup, can_apply, common_options, explain, guard_write, list_profiles, plan,
plan_remove, plan_upgrade, plan_waiting, power, preview, preview_backup, preview_power, preview_remove, preview_upgrade, probe, remove,
spec/specs, status, upgrade, validate_options, with_live; plus the free functions `connect::{plan_connect, preview_connect, connect, plan_push,
preview_push, push}`, `installed::{find, load, save, upsert}`, `localbin::{discover, adopt, judge, expand_typed}`, `gitea::{connect_existing, normalize_url}`.
Data types to move first (all in `platform/{mod,engine,connect,installed,localbin}.rs`, none in shared yet): Options, OptionDef, OptKind,
Exposure, Plan, MirrorPlan, Applied, Status, SvcState, UpgradePlan, RemovePlan, BackupPlan, BackupResult, ListedProfile, Installed, ClusterRef,
ConnectOpts, ConnectPlan, Connected, PushPlan, PrPlan, WsInfo, Transport, LocalBinary, Places, Discovery, BuildPlan, BuildState, Arch, Found.
Knot: the engine depends on k8stools, ocipush, tenantconfig, localreg, regsetup, pvcguard, deploylint, store (split-a). `Platform` is built by
the UI with its public fields (runner, vault, http, gen, live) and tests plant fakes there: the facet is `Services::platform_facet(user, audit,
run, env)` returning `Arc<dyn PlatformApi>`, and the fakes become constructors on the real side (`MemVault`, `FakeHttp` stay agent test helpers).
Order: move the data types (about 30, `move_types` does not walk directory modules: a `move_submods` pass per file), then the trait in three
parts (PlatformPlan, PlatformConnect, PlatformLocalBin), then switch `platform_services.rs`.

## domains (4 uses through the seam)

UI: `settings_domains.rs`. Methods: settings, set_listener, status, save_route, routes, make_trial_cert, certs, tunnel_choices, own, namespace_of,
import_cert, cert_state, discover (13) and the pure texts `target_text`, `cert_name_for`, `access_text`.
Types to move: ListenerCfg, CertRow, Status (agent `domains.rs`), and `cloudroute::Route` (agent; four agent deps: cloudlisten_tunnel,
cloudstorage, cloudtenant, sshtunnel, all used by validators, so `Route` and `RouteTable` move with their validators cut out or as a trait).
`k8s_cloud.rs` (split-a) uses the same `Route`/`RouteTable`: move once, coordinate.
Facet: `Services::domains_facet(user, audit) -> Arc<dyn DomainsMgr>`.

## devmode `DevMode` (settings_developer, 1 use) and `dm::` (ide_tile, 1 use)

`DevMode` methods the Settings page calls: about 45 (pairing, start_pairing, cancel_pairing, remove_pairing, paired_computer, state, enable, extend,
disable, is_on, now, status_text, trusted_list/on/off, shell_pending/approve/deny/end/auto_list/auto_on/auto_off, restart_grant_list/on/persist/off/active,
api_scopes/set_api_scope, core_scopes/set_core_scope/core_enabled/set_core_enabled, set_scope, set_pairing_scope, relay_enabled/set_relay_enabled/relay_url/
relay_summary, network_wide/set_network_wide, fw_want, allow_peer, allow_pairing_through_firewall, grant_diagnostic, activity).
Types to move: Pairing, ShellGrant, TrustedView, AutoShell, RestartView, DevKey, State, FwWant, Activity, TrustLevel, TrustFor (in
`devmode/{mod,pairings,grants,trusted,restartgrant}.rs`). Split the facet into three traits by concern: pairing and keys, trust and shell grants,
relay and firewall. `ide_tile.rs` needs `dm::client::{Home, Device, DevClient}`, `ops::{package_bytes, install}`, `sessions`, `stamp`, `is_on`;
`stamp` and `pairings::ago` are pure and move first.

## settings_agents (5) and agent_login (5, plus the shell test)

`settings_agents.rs::test_agent` builds a `RunRequest` from an agent that is not saved yet: the facet takes the agent as plain settings
(`iyakki_extagentx` data) instead of an id (`ExtAgentFacet::test(spec_with_settings)`). `agent_login.rs` holds a `LoginSession` for a long
time: an opaque `Arc<dyn LoginHandle>` (snapshot, cancel, submit-code) with `Method`/`Phase`/`Snapshot` moved to `iyakki-extagentx`
(they are serde-only data); `relay` (231 lines, only `iyakki_sec`) moves to `iyakki-extagentx` whole.

## Other leftovers by module (counts at 274)

chat: story 5, tasks 5, assistrun 3, gemini 2, geminilive 2, lemonade 2, agent 1, context 1, kb 1, modelhost 1, modelopts 1.
settings: scope 3, config 2, power 2, settings 1. vault: sharedvault 3, pendingplus 2, pending 1. network: browserext 1. files: quickinstall 3,
mediadl 1, sharedfolder 1. accounts: sharing 2, cloudtenant 1, cloudtenants 1. Most are one function each behind `Services`; the cheap ones go
into `HostProbeFacet`/`SoundProbeFacet` as before.

## The two timing tests

`chat::assist_tests::the_answer_appears_while_it_is_written_and_the_footer_says_where_the_time_went` and
`starting_a_run_does_not_wait_for_the_context` (file `crates/iyakki-apps/src/apps/chat_assist_tests.rs`) assert `overhead_ms < 200` and a best-of-three
start time. They fail when run in parallel on the shared build host and pass alone with `--test-threads=1`: they are in this lane's files and are
made robust below (a wall-clock budget polled instead of fixed beats).

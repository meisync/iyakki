# Core split, lane split-a: groups (F-SPLIT-03)

Owner rule (2026-11-17): act on a whole group, then compile and test the group once, regenerate the ratchet, commit. Counts are the total of
`docs/development/UI-AGENT-USES.txt` (both lanes) after each group; start of the lane: 509.

| Group | Modules | How | Count after |
|---|---|---|---|
| G1 chat-tool modules | the `*tools` wrappers of `apps/tool_chain.rs` (sshtunneltools, domainstools, browsershort, webgraphicstools, sftp, updatestools, router tools, doctortools, pdftools, ragtools and 30 more) | one method per contiguous run of the account tool chain, `ServicesFacetB4` (`cx.services.b4()`), real impl `iyakki_agent::facets_b4` | 431 |
| G2 services and background work | services, autopilot, sshtunnel_mgr, taskenv, runs, projectstate, corehandle | stateful handles of the b2 seam; facet over the long-lived objects the UI holds | not done: `services` is the composition root's bundle (18 uses of the type through the seam); the rest are single functions |
| G3 store | store (20), linuxapps, dlmanifest, imageapps | `StoreFacets` already exist; move the remaining `store::` types the UI names | planned |
| G4 cluster | devcluster, k8sexp, k3sdev, clusters, k8sretry, k8stools, clusterprobe | pure types to a leaf crate, kube runners behind a facet | planned |
| G5 small helpers (done in part) | settingstools_* (17), providererr, speechtime, touchprobe, touchrecover, vaultpending, voicepack, datatools, upnp_serve, voicehelper | `ServicesFacetB4::settings_entries_a/b`, `HelpersB4`, `HelperState` to iyakki-shared | 398 |
| G5b still open | camsource, browsersource, geminilive_*, lab, notes, rag*, router, store (20), devcluster (11), autopilot (9) | need types moved or handle facets | open |

Each group ends with: `rcargo . test -p iyakki-core --test boundary --test dependency_ratchet --test ui_agent_uses --test layer_split --test feature_matrix`.

## Round 2 (2026-11-17): whole-module merges, count 378

`dist/split/merge_module.py M` appends the agent file to the same-named module of iyakki-shared (which already holds M's moved types) and
leaves `pub use iyakki_shared::M;` behind. It worked for boarddeps, coverage, featureboard, k8sretry, toolblock, rag, browsera11y,
checkup_result and piclog; `webaccess`, `clientlink` and `Locality` were moved by hand.

Knots (a merge pulls the module's whole neighbourhood down, each needs its own plan):

| Module | What blocks it |
|---|---|
| sshtunnel (+ `sshtunnel_mgr`) | `gitkeys::valid_key_id` (split-b is moving GitKeys); the manager holds `Services` |
| clusterprobe, devcluster, k3sdev, clusters, k8sexp, k8scr | `aiprofile_remote`, `k8s` functions (`fetch`, `age_of`, `classify_failure`), `models`, `workspace`, `kubeprofiles` fns; one tree of about 10 modules |
| revise, hostvoice, study, taskenv, gitdiscover, linuxapps_dist | `services::Services`, `wordtime`, `voice::speakable`, `calendar`, `tasks::Task`, `pvcguard::Kube`, `clis::host_ok` |
| browsersource, dlmanifest | `browsermedia`, `voicedesk::SystemRecorders`, `pkg` key functions, `clis`, `netprobe` |
| store (20 uses) | the 32,000-line `store/` tree, `Store` holds the services and the install I/O |
| services (18) | `Services` names 60 agent modules; replace by a handle only with a facet per field the UI reads |

## Round 3: the cluster tree, count 362

Merged into iyakki-shared with `merge_module.py`: wsexec, workspace, k8s, k8sexp, k8scr, devcluster, kubeprofiles, aiprofile_remote, clusterprobe.
Cuts that stay in the agent: `kubeprofiles_svc::resolver_for` (needs `Services`) and `impl WorkReport for Ledger` (the rollout is reported
through a trait now, `iyakki_shared::k8sexp::WorkReport`). Copied plain: `jobs::process_exists`, `webengine::{available, host_session, set_host_mode}`,
`aiprofile::MIN_LLM_GB`. `impl KubeFeed for Feed` moved to iyakki-agent-api. Tests that need agent types stayed in the agent
(`kubeprofiles_tests`, `aiprofile_remote_tests`, `k8sexp_ledger_tests`). Left: `clusters` (uses `clis::CliManager`, `k3sdev`, `sysinfo`) and `k3sdev` (2,100 lines on `clis`, `exposure`, `ntp`, `clockfloor`).

## Round 4: ssh tunnels and Autopilot, count 342

`sshtunnel` merged into iyakki-shared (`valid_key_id` copied as a private plain fn); `Status`, `Row`, `TestReport` moved; the manager is the `SshTunnels`
trait (`TunHandle` wraps the shared `Manager`); `start_for_login` / `on_logout` are `ServicesFacetB4` methods. Autopilot: `Executor` to shared,
`ApStart` (plain data) + `autopilot_start` on `ServicesFacetB4` (builds the `Context` and the history from the services), `ApSaved` + `autopilot_saved`.
`autopilot.rs` itself stays in the agent (it needs `agent::start_with`, `boardtrack`, `projectstate`, `notes`, `bugs`, `assistant_guard`).

`services` (18): no facet attempted. The UI reads about 60 fields of `Services` (`dir()` 860 times, `settings` 340, `jobs` 107, `voice_settings` 104,
`on_login` 103, `browser` 81, `clis` 77, `sound_settings` 68 ...). A per-domain read facet means a trait per field group and rewriting those 2,000+ call sites;
that is its own project, not a slice of this lane.

## The store tree: plan (2026-11-17)

Measured: 20 UI uses of `iyakki_agent::store::` (now 18). The tree is about 50 files and 32,000 lines. `Store` (mod.rs) is an `impl` hub: install, lifecycle,
tilehub, bundle, drivers, approvals, storecmd, privatepub, publishflow and tools hold `impl Store` blocks or call it, and 130 places take `&Services`
(it uses `svc.skills`, `packages`, `mcp`, `clis`, `inbox`, `quick_upstream`, `settings`, `centre`, `journal` and others).

Shape:

1. **Data and rules to iyakki-shared** (`store_<name>`, tool `dist/split/merge_store_sub.py`): manifest, trust, revocation, catalog, license, hosting, sysannounce (step 1,
   done), then compmanifest, buildshare, updatenotice (need `devmode::update`), approvals, builtin, vet, checkout (need tile, skills, clis, pkg
   builders, the install fixtures).
2. **Tile runtime stays in the agent behind a trait**: `tile`, `tilesdk`, `sdkv2*` (they use `usage::UsageLog`, `audiosink::mixing`, `dash`, `flows`); the UI names only
   `tile::{Request, TileRuntime}` and `tilehub`, so a `TileHost` trait with those two plus the hub calls is enough.
3. **Server stays in the agent**: `server*` (`webservice::tls`, `sharedsite`); the UI names it in tests only.
4. **`Store` and its I/O**: see the owner decision below.

Blocking modules, one decision each:

| Module | Used for | Decision |
|---|---|---|
| `scope::explain_io`, `home_root` | error words, home folder | copy the plain fn `explain_io` into shared; `home_root` becomes a parameter of the bundle |
| `platform` (`installed::load`, `spec`, `localbin`) | cluster services of the private store | `platform::installed` data to shared if pure, else a `PlatformHost` trait; privatemanage/privatestore stay in the agent |
| `models::b64_decode` | icons | done: `crate::b64` of shared |
| `wtool` | tool package manifest and caps | done: data part is `iyakki_shared::wtool`, the sandbox stays |
| `sharedsite::statvfs_free` | free-space check | copy the plain fn (libc) into shared |
| `datamigrate::after_update` | after an update | trait method of the host the store gets |
| `bundlesnap` | data snapshots on update | move whole if pure, else host trait |
| `bindelta` | component deltas | direct dependency on the crate `iyakki-bindelta` |
| `usage::UsageLog`, `audiosink::mixing` | tile SDK | stays in the agent with the tile runtime |
| `webservice::tls` | store server TLS | stays in the agent with `server_http` |
| `k8stools::protected_namespace`, `localreg::explain_kubectl`, `bugs::redact` | words and checks | copy the plain fn |
| `qprintln`, `qeprintln` | CLI output | stays with the CLI files (`compcli`, `privatemanage`) in the agent |
| `official-roots.txt` | pinned root keys | stays where it is; the shared module includes it by path (owner's pinning step is unchanged) |

### Owner decision needed: what `Store` becomes for the UI

* **A, full move.** `Store` moves to shared and every `&Services` becomes `&dyn StoreHost` (a trait of about 60 methods over skills, packages, MCP, CLIs, inbox,
  upstream, settings, centre, journal). One big change; the agent implements `StoreHost` for `Services`. Removes about 15 more uses, and later the Store tests could run
  without the agent.
* **B, facade (recommended).** `Store` stays in the agent; the plain data types the UI names (`Item`, `State`, `Filter`, `PriceFilter`, `Refresh`, `Probe`, `Review`,
  `Life`, `Report`, `UpdatePlan`, `RemovePlan`, `Ctx`) move to shared, and the UI calls `StoreApi` (about 45 methods: items, review_item, review_sideload, refresh,
  plan_update, apply_update, rollback, plan_remove, remove_item, update_all, trust_key, sources, pins, licences, revocations ...) through `cx.services.b4().store()`.
  The same result for the ratchet, a smaller blast radius, and the same shape as `SshTunnels` and `VpnManager`.

Step 1 (done): wtool data, manifest, trust, revocation, catalog, license, hosting, sysannounce merged into iyakki-shared; count 342 -> 340. The remaining
steps wait for the decision above.

## Store step 2 (option B, decided by the coordinator): count 284

Moved to iyakki-shared with `dist/split/move_items.py`: `Probe`, `Item`, `State`, `Filter`, `PriceFilter`, `Refresh` (`store_core`); `Review`, `InstallError`, `Perm`,
`Step`, `Input`, `Record`, `scope_from_key` (`store_install`); `Ctx`, `Life`, `Report`, `UpdatePlan`, `LifeError`, `RemovePlan`, `Pin`, `Hist`, `Done` (`store_lifecycle`);
`MemberCard` (`store_bundle`). `StoreApi` (iyakki-agent-api `store_api.rs`, 47 methods incl. `update_check_run`) is the one handle; `StoreHandle` (agent) holds the
`Store` and the `Services`, so the UI no longer passes `&Services`. The Store tile, `store_manage` and `tile.rs` use it. Two methods have no default
(`review_sideload`, `plan_remove`: their results have no neutral value), so `ServicesFacetB4::store()` is `Option` and the UI helper `store_of(cx)` states that the agent core always serves it.

Left (about 10 uses): `ide_tile.rs` (publishflow `pf::*`, `privatepub::*`, `keys::*`, `tile::open_entry/run_tests`, `author::pack_dir`: they take `&Store`, so they need
a `StorePublish` part of the handle), `platform_services.rs` (`privatemanage`, `privatestore`, `Store`), `tile.rs`/`tile_wasm.rs` (`tile::{Request, TileRuntime}`, `tilehub`: the `TileHost` trait).

## Store step 3 (2026-11-17): the AI IDE publish flow, count 282

`StorePublish` (supertrait of `StoreApi`, so one handle): `pf_*` (stores, bumped, apply_bump, add_changelog, remote_version, token_present, save_token, card, publish,
review_published, install_reviewed, yank_note), `pp_*` (offer, deployed_all, option_label, need_admin_text, add_and_target), `keys_*`, `author_pack_dir`, `tile_open_wasm`.
`pf_preflight`, `pf_header_hint` and `tile_run_tests` need no store and sit on `HelpersB4`. Types moved: `Bump`, `Preflight`, `StoreChoice`, `Card`, `Stop`, `Deployed`, `Offer`,
`KeyInfo`, `Published`, `TestReport`, `Request`. Test: `store_api_tests::the_real_core_always_returns_the_store_handle_...` (the real core returns `Some`, a core
without a store returns `None`).

Left in the store domain (12): `platform_services.rs` (`privatemanage` and `privatestore` take the cluster `Platform` handle, a seam of the settings lane: the types `pm::Report`,
`Action`, `JobArgs` can move, the functions need `Platform` as a trait first), `tile.rs` (`TileRuntime`: about 15 methods and the public `page`/`out` fields; a `TileHost` trait
with `pump`, `type_into`, `click_control`, `answer_sdk`, `resolve`, `doc`, the page calls and the outbox), `ide_tile.rs` `deploy_dev` (`devmode::ops::install` takes the agent `Store`
and `DevMode`), and the `store` seam in `store.rs` (`store::{categories, featured, newest, filter, builtin::...}`, now the only `store::` names left there).

## The Services read facets: plan (not started)

Measured on 2026-11-17 over the non-test UI code (`services.`, `svc.`, `self.svc.` receivers): 1,483 uses of 185 distinct members, 34 public fields on `Services`.
The earlier figure of 2,000 counted test code. The 17 ratchet uses are only the places that NAME the type (`Arc<Services>` in signatures and fields); the 1,483 reads never name it.

| Group | Members (uses) | Proposed facet |
|---|---|---|
| Data folder | `dir` (398, in 105 files) | not a facet: `AppCx::data_dir()` (and `dir` on the shell env); codemod `X.services.dir()` -> `X.data_dir()`; `Path` only |
| Settings | `settings` (73, `lock()` of the `SettingsStore`), `voice_settings` (55), `sound_settings`, `agents_cfg` (28), `system_set`, `cloud_setting`, `show_all_ai`, `set_show_all_ai` | `SettingsRead` (get a typed snapshot, `update(|s| ..)` with a closure over a shared settings type); the settings lane's `SettingsStore` must be in shared first |
| Vault and accounts | `user_get` (13), `user_set`, `user_delete`, `user_names`, `user_vault_unlocked`, `secrets` (12), `secret_requests`, `answer_secret_request`, `cancel_secret_request` | `VaultRead` (split-b's vault lane owns the types) |
| AI and voice | `host_tts` (28), `ext_status` (30), `ext_sessions`, `client_for`, `models`, `image_models`, `refresh_lemonade`, `lemonade_load`, `picture_choices`, `provider_is_lemonade`, `transcribe_samples`, `test_sound`, `set_voice_mic`, `usage`, `kb` | `AiRead` |
| Cluster and CLIs | `k3sdev` (48), `clis` (35), `cluster` (11), `devwatch` (13), `quick_upstream`, `ollama`, `seaweedfs`, `rustfs` | `ClusterRead` (after `clusters`/`k3sdev` are in shared) |
| Work and content | `inbox` (40), `jobs` (29), `runs` (13), `sessions` (16), `skills` (17), `mcp`, `packages`, `calendar`, `study`, `radio` (19), `screen`, `camera`, `browser` (34), `devtools` | one small facet per store type, most of them are already shared data types plus an `Arc` of the store: the work is to move the store struct, not to write a trait |
| Facet accessors | `b0`, `b3`, `b4` (63), `store_facets`, `flows_facet`, `git_keys` | already facets; they become methods of the same handle |

Approach, in steps, each with a compile and test pass and a ratchet count:

1. **`data_dir()`** (codemod, no ratchet change, about 400 sites): removes the biggest reason to hold a `Services` at all. After it, 56 of the 105 files that read `dir` read nothing else.
2. **Stop passing the type** (the 17 ratchet uses): functions that take `&Arc<Services>` only to read `dir`, a store or a facet take `&AppCx` or the facet handle instead; structs that hold `Arc<Services>` hold `AppCx`. Each site is a hand edit of a signature (17), the bodies change by the codemod.
3. **Facet per group**, in the order of use: Work and content (stores whose structs move to shared), then Settings (needs the settings lane's `SettingsStore` in shared), then AI and voice, Cluster, Vault. The codemod `dist/split/rewrite_services.py MEMBER FACET` rewrites `X.services.MEMBER` and `svc.MEMBER` to `X.FACET().MEMBER` for one member at a time and leaves a line it cannot parse for hand editing (the same shape as `gen_facade.py --rewrite` plus `revert_failed.py`: compile, revert the lines that do not compile).
4. **Remove the seam**: when no UI file reads a member, drop the field from `AppCx::services` (it becomes `AppCx::host: Arc<dyn Host>`), and `agent_handles_b2::services` has no use left.

Decisions for the owner: (a) whether step 2 may change function signatures across the apps (it touches about 17 files, no behaviour change); (b) whether `SettingsStore` is moved by the settings lane first (it is the largest read: 73 + 55 + 28 uses); (c) whether `AppCx` may grow `data_dir()` (it already holds `services`, `os`, `agent`).

## Services step 2 (2026-11-17): the settings data in iyakki-shared, count 257

Moved: `settings` (merged into the shared module), `hwprofile` (the `Services` functions stay in the agent as `hwprofile_svc`), `extagent/mod.rs` with `claude_code`, `copilot`,
`generic` (`login`, `bridge`, `run`, `relay`, `proc`, `guidance` stay), `extagentcfg::AgentsCfg`, `SystemSounds`, `PowerSettings`, `ExternalRag`; copied: `ntp::valid_server`, `toollog::MAX_DAYS`,
`bridge::copilot_config`. New shared dependencies: `iyakki-battery`, `iyakki-extagentx` (both serde only). Lesson: a type whose inherent methods build agent objects
(`AgentsCfg::agent`) can move only together with the objects (here the three agent classes, which depended on `wsexec` only, already in shared).
The regen script of the ratchet had a bug that deleted `domain settings` when `module settings` had no use left; it now removes only `module` lines.

## Services steps 3 to 6 (2026-11-17): cameras, usage, the voice worker, settings_agents, count 239

Each its own commit. `ServicesFacetB4` gained `camera_tools`, `camera_resume`, `camera_source` (hub and person travel type-erased, as for the router), `usage_source`, `metered_chat`,
`voice_assistant_spawn` (+ `MineWorker`), `voice_ready`; `HelpersB4` gained `camera_sign_names` and `camera_stop_all_signs`. The settings_agents helpers take the shared `StatusCache`
instead of `Services`. `hwprofile_svc.rs` is left as a module (7 functions, one caller each).

Still naming `Services` in the UI (13 uses, the floor is 2-3): `cameras.rs` (`hub_from` builds the agent `Hub` from the services; `client_feed`, `resume_recording`), `checkup.rs` and `cluster_host.rs`
(fields), `browser_headers.rs` (2), `web_agent.rs`, `platform_services.rs`, `samples.rs`, `settings.rs`, `mod.rs` (the field of `AppCx`), `shell.rs` (the composition root), and the `agent_handles_b2` seam.

## Store steps 4 and 5, cameras step 7 (2026-11-17): counts 235 -> 233 -> 232

`camera_hub` (the hub is made in the agent; `cameras.rs`, the shell and `iyakki-remote` pass the facet handle instead of `Services`), `TileRt` + `tile_open` + `TileTake`
(the running tile app: 15 methods, `Outbox` stays in the agent), the pure store functions (`filter`, `featured`, `newest`, `publishers`, `categories`, `score`, `PublisherInfo`) in `store_core`.
Store domain left (4 uses): `platform_services.rs` (`privatemanage`, `privatestore`, `Store`: split-b's `Platform` seam, ask first) and `ide_tile.rs` `deploy_dev` (`devmode::ops::install` takes `Store` and `DevMode`).

## The `other` bucket, by count (2026-11-17, total 141 of 232)

services 11, disksformattools 5, browsersource 5, k8stools 4, taskenv 3, runs 3, projectstate 3, notes 3, lab 3, jobtools 3, geminilive_audio 3, edittest 3, doctor 3, corehandle 3
(composition root, stays), calendar 3, assistant_guard 3, then 2 each: study, regsetup, radio, mediaqueue, mediaindex, linuxapps_net, hostvoice, gitnet, gitjobs, geminilive_host, directlink, devtools,
contract_local, clientmedia, cli, boardtools, avaddon_install, accessbroker, HostApi; and 42 modules with one use each. Order of work: disksformattools, browsersource, k8stools, the `runs`/`taskenv`/`projectstate`/`notes` group
(project state of the AI IDE), then the 2-use modules; one commit each.

## Steps 5 to 7 of the `other` bucket (2026-11-17), count 195

Done: runs (`ChatRun` handle, `chat_run_start`, `chat_run_from_handle`; `RunLog` to shared), notes, assistant_guard, bugs, boardtrack, phrases (merged into shared; `estimate_tokens` copied),
calendar (merged; the alarm sound is `HelpersB4::calendar_ring`), the whole project tool set of an AI IDE chat (`ProjectKit`), disksformattools (iyakki-disksx), browsersource, linuxapps_net.

## What is left of the `other` bucket: the plan (module, uses, why it is not a move, proposed shape, estimate)

Totals: other 107, services 11 (composition root, stays), corehandle 3 (composition root, stays), `AgentApiImpl` 1 (composition root, stays).

| Module | Uses | Why it stays | Proposed shape | Estimate |
|---|---|---|---|---|
| taskenv | 3 | `Task`, `Manager` are the `tasks` seam (split-b); `TaskCluster` takes the cluster | `TaskKit` facet over the task manager: `close_question`, `bring_my_changes`, provision/teardown | 0.5 day, after split-b's `tasks` handle |
| lab (+ lab_pic, lab_ui) | 3 | `Catalogue`, `LabModel`, `PictureBackend`, `VisionBackend` over lemonade and the browser jobs | `LabKit` facet (the backends are built from the services; the UI holds the handle) | 1 day |
| geminilive_audio / _host / _ws | 6 | live voice session: `SpeechSink(svc)`, `Board`, `Running`, a websocket connector | one `GeminiLive` facet (`spawn(parts) -> Running` handle, `Board` data to shared) | 1 day |
| edittest | 3 | spawns the project's test commands, claims the process in `procowner` | `ProcOwner` + `TestRun` facet: `plan(path)`, `run(plan, secs, flag)`; `procowner` stays in the agent | 0.5 day |
| doctor, doctor_gather, enginelog, checkup | 6 | the DOCTOR facts are gathered from `sysinfo`, `enginelog`, `vpn_mgr`, `k3sdev` | `DoctorKit` facet: `gather(live) -> Facts`, `sound_outputs()`; `Facts` data to shared | 1 day |
| study, studylesson, studypdf, scene, storypdf, storyillus | 7 | `StudyStore` has inherent methods in `studylesson` (needs `Services`, `scene`, `story`) | merge `study`+`studylesson`+`scene`+`story*` together after `lemonade` and `Services` reads are facets; or a `StudyKit` facet | 2 days |
| radio, radiotools | 3 | `RadioPlayer`, `Holder`, `StationStore` are stateful and tied to the mixer | `RadioKit` facet over the player (state/stations to shared) | 1.5 days |
| mediaqueue, mediaindex | 4 | need `mediadl`, `embed`, `media::scan` and `Services` | one `MediaKit` facet (`index`, `queue`) | 1.5 days |
| regsetup, localreg, cloudlaunch, cloudcli, router*, remotefiles* | 12 | deploy path (`deploylint`, `netprobe`, `pvcguard`, `ocipush`), cloud launch | `DeployKit` facet wrapping the deploy and registry steps; the router and remotefiles by their own handles (`RouterKit`, `RemoteKit`) | 3 days |
| gitnet, gitjobs, gitdiscover | 5 | the git engine with its HTTP client and jobs | `GitKit` facet (the git tile and wizard take the handle; `NetGit` built inside) | 1 day |
| hostvoice, recordings, revise | 4 | host speech engines and the dictation reviser take `Services` | `SpeechKit` facet (`BrowserSynth`, `BrowserSink`, reviser) | 1 day |
| devtools, clientmedia, contract_local, cli, accessbroker, sysprocs, avaddon_install, clis, dlmanifest, linuxapps(+dist, imageapps, linuxwin) | 20 | each 1 or 2 uses of stateful objects (`DevTools`, `Cli`, `CliManager`, the access broker, the Linux apps engine) | one facet per object as `ProjectKit`; the CLI and Linux apps group first (7 uses) | 3-4 days |
| the 1-use tail (rag*, sftp, settingstools, qaimage, consul, camtools, browserjobs, tasktools, WasmTask/WasmIo ...) | about 25 | single calls | facet methods on `ServicesFacetB4`/`HelpersB4`, one commit per five | 2 days |

Not in the bucket but on the same path: `platform_services` (privatemanage, privatestore) and `deploy_dev` wait for split-b's platform facet; `k8stools` (2) in `ide.rs` and `ide_deploy.rs` needs the `K8sKit` handle (`ProjectKit` already builds the chat's).

Total for the rest of the bucket: about 20 working days of this size of change. The composition-root crate step needs only: services 11, corehandle 3, `AgentApiImpl` 1 and the `agent_handles_b2` seam to stay named.

## The study group: prerequisites and order (2026-11-17, count 91)

UI uses of the group: `study` 2 (`score`), `studypdf` 1 (`export`), `studylesson` 1, `scene` 1 (`CharacterSheet`), `storyillus` 1 (`Coverage`, `Readiness`),
`storypdf` 1 (`story_pdf`), `story` 4 (chat, settings, learn_lesson), `lemonade` 2 (`Model`, `ModelType`).

What each module needs from the agent (found by reading its `crate::` references):

| Module | Needs from the agent | So |
|---|---|---|
| `study` | `clock`, `calendar` (both already in shared) | whole module to shared, first |
| `studypdf` | `pdftools`, `study`, `studylesson::Lesson`/`Question` (types already in shared) | after `study`; `pdftools` call goes through the existing pdf facet or moves |
| `studylesson` | `scene::reply` (3), `story::sanitize_svg`, `Services` (3 functions: `draw_picture`, `generate`, `ask`), `chat` (in shared) | pure parts to shared; the three `Services` functions become one facet method each (`StudyKit`); `sanitize_svg` and `scene::reply` move first |
| `scene` (directory) | `story::sanitize_svg`/`PictureStyle`/`StoryOptions`/`follow_agent_run`/`image_prompt_with`/`base64_encode`/`MAX_SVG_*`, `extagent` (shared), `tenantconfig::catalog` | `CharacterSheet` (type) to shared first; `agentchat`, `compose` stay agent |
| `storyillus` | `scene::CharacterSheet`, `story::PictureStyle`, `settings::ModelChoice` (shared), `lemonade::ModelType`, `sysinfo::Memory` (shared) | types `Coverage`, `Readiness` to shared once `CharacterSheet` and `PictureStyle` are there |
| `storypdf` | `story`, `pdftools` | `story_pdf` becomes a facet method (pdf date already comes from the agent handle) |
| `story` (4493 lines) | `HostVoice` (5), `storyillus::PictureOpts`, `piclog`, `audiosink`, `Services`, `vault::Secrets`, `providers`, `scope` | only the types the UI names (`Hints`, `StoryOptions`, `StoryStore`) move; the engine stays |
| `lemonade` | `voice::SpeechEngine`, `providers` helpers | `Model`, `ModelType`, `ImageDefaults`, `remember`, `api_base` (pure) to shared |

Order: 1 `study` whole; 2 `studypdf` with `Lesson`; 3 `sanitize_svg` and `PictureStyle` and `scene::reply` to shared (small pure items); 4 `CharacterSheet`;
5 `studylesson` pure part and `StudyKit` facet (`facets_b5.rs`); 6 `storyillus` types; 7 `storypdf` facet; 8 `lemonade` types; 9 `story` types.
Each step its own commit and a count report. Tests that need agent types stay in the agent as `*_tests.rs`.

## StoryKit: the TALES engine (2026-11-17, count 63)

The UI still names `iyakki_agent::story` in 4 files: `Hints`, `StoryStore`, the parsers (`parse_story`, `parse_hints`, `chunks_for`, `sentence_chunks`,
`title_from_topic`, `plan_model`, `auto_style`, `slug`), the voice status helpers (`status_text`, `tts_status_auto`, `effective_engine_auto`, `quick_engines`,
`voice_choices`, `NO_ENGINE_HELP`) and the reading pipeline (`Synth`, `Sink`, `Playing`, `ServicesSynth`, `CommandSink`, `PrebuiltSynth`, `render_audio`).
`Synth`, `Sink`, `Playing`, `Chunk`, `chunks_for` and `sentence_chunks` are already in shared (split-b). What `story.rs` still needs from the agent is small:
`Services` (only `ServicesSynth`), `vault::Secrets`, `soundhold`, `audiosink::highlight_offset_ms`, `voice::Player` (the `Playing` impl), `agent::RunHandle`
(`follow_agent_run`). Steps, each a compiling commit:
1. the whole of `story.rs` is merged into `iyakki-shared::story` except what names those five agent items; compile errors list what is left;
2. the leftovers become `StoryKit` (`ServicesFacetB5::story_kit`): `ServicesSynth` as `synth(opts)`, the `Playing` over the player, `follow_agent_run`,
   the sound announcement, and `render_audio` stays shared (it takes `&dyn Synth`);
3. the UI paths change to `iyakki_shared::story` and the facet; tests that need agent types stay in the agent as `story_tests`.

## The Services read facets: plan v2 (split F, 2026-11-17): the UI holds no `Services`

Re-measured on 2026-11-17 over the non-test UI code (`services.` and `svc.` receivers): **1,265 reads of 214 members** (the earlier 1,483/185 was before `dir()`; `dir` is still 132 on non-`cx` receivers such as `self.env.services`, `svc`).
The 17 places that NAME the type are the ratchet's `services 11` (fields and signatures: `AppCx`, `ClusterHost`, `Checkup`, `BrowserHeaders`, `web_agent`, `platform_services`, `samples`, the shell). A read never names the type, so a trait or a type from a crate the UI may see is all a read needs.

### End state

* The UI crates name no `Services`. `AppCx` carries per-domain handles, not one bag. Today every handle is an `AppCx` method whose body reads `self.services`; the day `AppCx::services` is gone only those bodies change, no app does (that is why each piece first adds the method, then rewrites the readers with the codemod).
* Three kinds of member, three treatments:
  1. **Shared data types** (already in `iyakki-shared`/`iyakki-memx`/`iyakki-core-api`): the UI holds the type directly, no trait. `AppCx::settings()`, `inbox()`, `calendar()`, `study()`, `secrets()`, `kb()`, `voice_settings()`, `sound_settings()`. The composition root builds them.
  2. **Facets** (already traits in `iyakki-agent-api`: `b0`, `b3`, `b4`, `b5`, `voicedesk_facet`, `extagent_facet`, `tasks_facet`, `flows_facet`, `git_kit`, `store_facets`, `pending_facet`): `AppCx::b4()` etc.; later `AppCx` holds the `Arc<dyn ...>` itself.
  3. **Agent-typed stores** (struct has behaviour in the agent): a read facet per domain in `iyakki-agent-api`, or the struct moves to shared when it is pure data plus a file (the earlier "store steps" moved `Inbox`, `CalendarStore`, `StudyStore`, `RunLog`, ...).
* **Composition root**: new crate `iyakki-compose` (layer `core`, above shell, apps and agent): the ONLY crate that names both. It builds the agent (`Services`, `AgentApiImpl`, `AgentCore`), installs the injected handles (`ensure_real_agent`, `install_agent`, the host, the file scope, `LocalHost`) and gives the shell what `Shell::new` / `ShellEnv::from_paths` need. `iyakki-core` main calls it.
* Dependency removal: when `Services`, `corehandle`, `AgentApiImpl` and the lane modules are gone from `iyakki_agent::` in the UI, `iyakki-agent` leaves both `Cargo.toml`s and `dependency_ratchet` pins it.

### Member table (non-test reads, 2026-11-17, after piece 1 and 2 below)

| Member (reads) | Type lives in | Treatment | Piece |
|---|---|---|---|
| `settings` 69, `voice_settings` 54, `sound_settings` 6, `agents_cfg` 28 | shared (`SettingsStore`, `VoiceSettings`, `SoundSettings` in core-api, `AgentsCfg`) | shared type held directly: `AppCx` accessors | 2 done |
| `inbox` 40, `calendar` 10, `study` 12, `secrets` 12, `kb` 12 | shared / memx | shared type held directly | 2 done |
| `b0` 25, `b3` 18, `b4` 33, `b5` 15, `voicedesk_facet` 14, `extagent_facet` 8, `tasks_facet` 4, `flows_facet` 3, `git_kit` 15 | agent-api traits | `AppCx` accessors; later fields | 1 done |
| `dir` 132 (non-`cx` receivers) | `Path` | `data_dir()` on the shell env / handle | 3 |
| `k3sdev` 47, `cluster` 10, `devwatch` 13, `ollama`, `seaweedfs`, `rustfs`, `clis` 34 | agent (`K3sDev`, `Cluster`, `CliManager`, ...) | `ClusterRead` facet in agent-api; cluster lane owns `k3sdev`/`clusters` | 4 (after the cluster lane) |
| `browser` 32, `screen` 11, `camera` 7 | agent (`BrowserState`, `ScreenCapture`, `CameraHub`) | one `BrowserRead` facet (jobs the shell serves, consent) | 5 |
| `jobs` 25, `runs` 10, `sessions` 14, `skills` 14, `mcp` 10, `packages` 7, `devtools` 10, `radio` 19 | agent stores | a small facet per store, or the store struct to shared if pure data | 6 |
| `host_tts` 27, `ext_status` 25 (`StatusCache` is shared), `ext_sessions`, `client_for`, `models`, `image_models`, `lemonade_*`, `picture_choices`, `usage` | mixed | `AiRead` facet; `ext_status` direct | 7 |
| `user_get/set/delete/names/vault_unlocked`, `system_set`, `secret_requests`, `answer/cancel_secret_request` (~50) | agent (`VaultError` shared) | `VaultRead` facet (vault lane owns the types) | 8 |
| `clone` 94 | `Arc<Services>` handed on | goes away with the 17 type names (signatures take `&AppCx` or a handle) | 9 |
| the 1-use tail (about 120 members: lemonade, wake, fleet, cloud stream, sftp, system sounds) | agent | `b3`/`b4`/`b5` facet methods or a facet each, by group | 10 |

### Order and checks

One commit per piece, each: compile (`mcargo`, apps + shell with tests), codemod, ratchet regenerated, `FEATURES.md` test paths repointed if a test moved, `cargo test -p iyakki-agent --test facet_defaults` (real-wiring guard) and the sweep `ui_read_facets_real` (a fake default must not be what production returns: the accessors above must reach the real stores).
1. Facet accessors on `AppCx` (this section: 9 facets, ~150 reads moved).
2. Shared-typed reads on `AppCx` (this section: ~270 reads).
3. `iyakki-compose` and the injected handles; the shell constructors take their dependencies from it.
4. to 10. By group as in the table; 9 and the dependency removal last.

### Result of pieces 1 to 3 (2026-11-17, commit "split F ...")

Reads of `services`/`svc` in non-test UI code 1,265 -> 1,017 (about 250 moved to `AppCx` accessors, plus the test code). `iyakki_agent::` uses in the UI 26 -> 22 -> 18 after the merge with the other lanes (left: `services` 11, `clusters` 3, `k3sdev` 2, `scene` 1, `lab` 1).
`iyakki-compose` (layer core) installs the agent handles and the core factory; `iyakki-core` main, `iyakki-gui` and the remote tests call `iyakki_compose::wire()`; shell examples use `examples/common`; shell unit tests build the same wiring under `cfg(test)` (`wiring::test_wire`).
`ui_agent_uses` now also pins the dependency: `STILL_DEPENDING` lists the UI crates that still have `iyakki-agent` in `[dependencies]` (iyakki-apps, iyakki-shell); the list only shrinks.
What blocks the removal: the type `Services` (11 names: `AppCx::services`, `ShellEnv::services`, `ClusterHost`, `Checkup`, `BrowserHeaders`, `web_agent`, `platform_services`, `samples`, `settings`, `agent_handles_b2`), the cluster lane (5), `scene`, `lab`, and the 1,017 reads that go through `AppCx::services`/`ShellEnv::services` to agent-typed stores (table above, pieces 4 to 10). The feature `testing` of iyakki-apps also names the agent (`default_agent`, `default_host`, `default_file_scope`): it becomes an optional dependency or moves to the test helper crate when the dependency goes.

### Result of split G pieces 7 and 8 (2026-11-17, split F continued)

Moved onto accessors (no app reads them through `services` any more, non-test code): `host_tts` (trait `HostTtsApi`, `AppCx::host_tts()` / `ShellEnv::host_tts()`), `ext_sessions`, `client_for` (test and one chat turn), `image_models`, `picture_choices`, `unavailable_models`, the usage recording (trait `AiRead`, `cx.ai()`), the vault group (trait `VaultRead`, `cx.vault()`: `user_get/set/delete/names/vault_unlocked`, `system_set/names/delete`, `reset_user_vault`, `answer/cancel_secret_request`), `usage` (`cx.usage()`, shared `UsageLog`), `secret_requests` (`cx.secret_requests()`, shared `SecretRequests`), `notify`/`notify_once` (`cx.notify`, `env.notify`: they only write the shared inbox), `store_facets` on the shell (`env.storeapi()`), and every `dir` read (`cx.data_dir()`, `env.data_dir()`).
Types moved to `iyakki-shared`: `usagelog` (UsageLog and its charts; `Metered` stays in the agent), `extsessions` (SessionMap), `secretreq`, `hostengines::Env`, `providers::TestReport`. The facets are in `iyakki-agent-api/src/facets_g.rs` and `iyakki-agent/src/facets_g.rs`; the sweep `the_context_read_accessors_are_the_real_stores_not_defaults` also checks that the facets write through to the real stores.
`ShellEnv` has the same accessor block as `AppCx` (`b0..b5`, the facets, `settings`, `inbox`, `calendar`, `study`, `secrets`, `kb`, `voice_settings`, `sound_settings`, `agents_cfg`, `ext_status`, `ai`, `vault`, `host_tts`, `usage`, `secret_requests`, `data_dir`, `notify`).
UI direct uses of the agent: 11 -> 9 (`BrowserHeaders` no longer holds `Services`).

What is left for the type removal (the flip of `AppCx::services` and `ShellEnv::services` to a trait object / the facets):
* the members of split A (about 270 non-test reads: `k3sdev`, `jobs`, `radio`, `browser`, `clis`, `skills`, `mcp`, `sessions`, `runs`, `devwatch`, `cluster`, `ollama`, `devtools`, `packages`, `screen`, `camera` and the tail: `quick_upstream`, `slow_upstream`, `streamer`/`set_streamer`, `update_sound`, `set_voice_mic`, `reading_synth`, `session_workdir`, `web_access_*`, `contract_local`, `git_keys`, `speech_kit`, `cloud_setting`, `refresh_lemonade`, `rag_kit`, `fleet`). Types that still name `Services` only because they read these members: `ClusterHost`, `Checkup`, `web_agent`, `platform_services`, `samples`, `settings::svc`.
* closures that capture `Arc<Services>` as `svc` and read my members (`svc.inbox`, `svc.settings`, `svc.kb`, `svc.secrets`, `svc.voice_settings`, `svc.b0/b3/b4`): about 25 places (chat, rag, settings, settings_agents, store, store_setup, shell, shell_startup). They become `let inbox = cx.inbox()`-style clones taken before the closure once the other members of the same closure are gone.
* the `testing` feature of `iyakki-apps` (`default_agent`, `default_host`, `default_file_scope` build the real agent handles): moves to `[dev-dependencies]` (own tests) and an explicit `iyakki_compose::wire()` / `wiring::test_wire` for the shell and remote tests.

### Result of split H: the flip (2026-11-18, the finish of the core split)

`iyakki-apps` and `iyakki-shell` no longer depend on `iyakki-agent` in `[dependencies]` (a dev-dependency for their own tests); `STILL_DEPENDING` in `crates/iyakki-core/tests/ui_agent_uses.rs` is empty, so the ratchet pins the dependency, and a second test shows that a re-added line is seen. Direct `iyakki_agent::` uses in non-test UI code: 4 -> 0 (`docs/development/UI-AGENT-USES.txt`).
* `trait ServicesHandle` (`iyakki-agent-api/src/facets_h.rs`, real implementation `iyakki-agent/src/facets_h.rs` for `Services`, which keeps a `Weak` to itself so the `self: &Arc<Self>` members work behind `&self`). `AppCx::services` and `ShellEnv::services` are `Arc<dyn ServicesHandle>`. A member the UI still needs is added with `dist/split/add_handle.py NAMES` (it finds the signature in the agent and writes both halves); `dist/split/h_iter.sh` is one compile-and-add round; `fix_handle_uses.py` turns `svc.field` into `svc.field()` and `ServicesBn::m(&svc, ..)` into `svc.m(..)`; `fix_test_real.py` does the same for test code (`.real()` from `iyakki_agent::facets_h::RealServices` gives the concrete `Services` to a test).
* Types that the handle's signatures needed moved to `iyakki-shared`: `PullStatus`, `SessionDeleted`, `LoginReport`, `How`/`Place`/`Pick`/`VisionError`/`Cand` (`visionmodel`), `Review` (`omni`), the fleet views, `Status`, `AssignBody`, `Heartbeat`, `FleetAct` (`fleetview`), `SandboxResult`, `engine_setting`.
* The shell opens its services through the factory the composition root installs (`iyakki_shell::wiring::install_services_factory`, `iyakki_compose::open_services`); `ShellEnv::from_paths` no longer names `Services::open`. Programs that need the concrete type (`iyakki-core`, `iyakki-remote`) use `Services::from_handle`.
* The `testing` feature of `iyakki-apps` no longer builds the real agent: `iyakki_apps::testkit` holds what a test double needs (`TestKit`); this crate's own tests build the real kit from the dev-dependency, the shell's tests install it from a load-time initialiser (`wiring::test_wire`, so the one-shot handles are real before any test reads them), the programs call `iyakki_compose::wire()`, and the remote tests and examples call it first.
* Guards (KI-1010): `Shell::new` asserts `env.services.is_real()`; the sweep `the_context_read_accessors_are_the_real_stores_not_defaults` also checks that the handle is the real `Services` and that the accessors return the objects the concrete type holds; `real_wiring` checks the handle, its type and its data folder.

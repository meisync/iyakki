# Built-in apps: the inventory for the move onto the app standard

Written by `python3 dist/scripts/app-inventory.py --write`; do not edit by hand, run the script again (`--check` says when the file is out of date).
The standard and the way to move one app is [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) section 10 (worked example: section 12, RADIO).

How to read it: **uses** counts, in the non-test code of the app's files, the places that name the services or the shell context (`.services`/`Services`, `cx.host`, `cx.web`, `cx.chat`, `cx.store`, `cx.session`/`cx.audit`); `AppCx` counts the mentions of the context type itself (a migrated tile keeps only the entry points of the `App` trait). **agent paths** counts the `iyakki_agent::` paths (the data types and helpers the app takes from the agent, which an extracted crate cannot); **ratchet** is the sum of the lines the app's files hold in `shell_kernel_access.allow` (direct kernel and hardware access). **chat** says whether the assistant has tools whose names start with the app's name; **settings** whether a Settings page has the app's name. Effort: S up to 15, M up to 60, L above, counting uses plus half the agent paths, one class up for an app of more than 6000 lines. The order is the suggested order for alpha.4: small and quiet first, so that every change is one tile and the procedure is corrected on the easy ones before the hard ones.

| Order | App | Files | Lines | AppCx | services | host | web | chat | store | session/audit | uses | agent paths | ratchet | chat tools | settings page | port to write | effort | state |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | Firewall (`FirewallApp`) | firewall_tile.rs | 106 | 6 | 0 | 0 | 0 | 0 | 0 | 4 | 4 | 0 | 0 | yes | yes | `FirewallPort` | S | to do |
| 2 | Disks (`DisksApp`) | disks_tile.rs, disks_tile_ui.rs | 1951 | 25 | 0 | 0 | 0 | 0 | 0 | 8 | 8 | 0 | 0 | yes | no | `DisksPort` | S | to do |
| 3 | Dash (`DashApp`) | dash.rs, dash_charts.rs, dash_query.rs and 1 more | 5073 | 25 | 1 | 0 | 0 | 0 | 1 | 6 | 8 | 0 | 0 | no | no | `DashPort` | S | to do |
| 4 | DevTools (`DevToolsApp`) | devtools/mod.rs, devtools/net.rs, devtools/page.rs and 2 more | 1224 | 10 | 0 | 0 | 0 | 0 | 0 | 9 | 9 | 0 | 0 | no | no | `DevToolsPort` | S | to do |
| 5 | K8s (`K8sApp`) | k8s_tile.rs, k8s_tile_ui.rs | 2987 | 21 | 0 | 0 | 0 | 0 | 0 | 9 | 9 | 0 | 0 | yes | no | `K8sPort` | S | to do |
| 6 | Learn (`LearnApp`) | learn.rs, learn_lesson.rs, learn_ui.rs | 1539 | 26 | 3 | 0 | 0 | 2 | 0 | 8 | 13 | 0 | 0 | no | no | `LearnPort` | S | to do |
| 7 | Flows (`FlowsApp`) | flows.rs, flows_assist.rs, flows_board.rs and 11 more | 7319 | 21 | 1 | 0 | 0 | 0 | 0 | 6 | 7 | 0 | 0 | yes | no | `FlowsPort` | M | to do |
| 8 | Git (`GitApp`) | git_tile.rs, git_tile_repo.rs, git_tile_repo_ui.rs and 1 more | 3961 | 36 | 1 | 0 | 0 | 0 | 0 | 16 | 17 | 0 | 0 | yes | no | `GitPort` | M | to do |
| 9 | System (`SystemApp`) | system.rs | 979 | 14 | 0 | 1 | 0 | 0 | 0 | 18 | 19 | 0 | 0 | yes | no | `SystemPort` | M | to do |
| 10 | Mesh (`MeshApp`) | mesh.rs | 605 | 4 | 20 | 0 | 0 | 0 | 0 | 0 | 20 | 0 | 0 | no | no | `MeshPort` | M | to do |
| 11 | Rag (`RagApp`) | rag.rs | 620 | 13 | 4 | 0 | 0 | 2 | 0 | 14 | 20 | 0 | 0 | yes | yes | `RagPort` | M | to do |
| 12 | Lab (`LabApp`) | lab.rs, lab_pic.rs, lab_ui.rs | 2350 | 27 | 9 | 0 | 0 | 0 | 0 | 27 | 36 | 0 | 0 | no | no | `LabPort` | M | to do |
| 13 | Inbox (`InboxApp`) | inbox.rs | 1049 | 16 | 2 | 0 | 0 | 0 | 0 | 47 | 49 | 0 | 0 | no | no | `InboxPort` | M | to do |
| 14 | Edit (`EditApp`) | edit.rs, edit_preview.rs | 2326 | 27 | 1 | 1 | 0 | 1 | 0 | 47 | 50 | 0 | 0 | yes | no | `EditPort` | M | to do |
| 15 | Cli (`CliApp`) | cli.rs, cli_conn.rs, cli_term.rs | 1701 | 25 | 3 | 3 | 0 | 0 | 0 | 53 | 59 | 0 | 0 | yes | no | `CliPort` | M | to do |
| 16 | Tales (`StoryApp`) | story.rs | 5772 | 98 | 11 | 0 | 1 | 1 | 0 | 46 | 59 | 0 | 0 | no | no | `StoryPort` | M | to do |
| 17 | Voice (`VoiceApp`) | voice.rs, voice_assist.rs, voice_fix.rs and 4 more | 3192 | 65 | 16 | 1 | 0 | 1 | 0 | 57 | 75 | 0 | 0 | yes | yes | `VoicePort` | L | to do |
| 18 | Store (`StoreApp`) | store.rs, store_drivers.rs, store_manage.rs and 4 more | 8333 | 107 | 15 | 1 | 0 | 0 | 0 | 137 | 153 | 0 | 0 | yes | no | `StorePort` | L | to do |
| 19 | Chat (`ChatApp`) | chat.rs, chat_appevents.rs, chat_browser_data.rs and 15 more | 8271 | 117 | 8 | 1 | 0 | 2 | 0 | 149 | 160 | 0 | 0 | yes | no | `ChatPort` | L | to do |
| 20 | Ide (`IdeApp`) | ide.rs, ide_bottom.rs, ide_browser.rs and 15 more | 13939 | 144 | 29 | 1 | 0 | 1 | 0 | 169 | 200 | 0 | 0 | yes | no | `IdePort` | L | to do |
| 21 | Browser (`BrowserApp`) | browser.rs, browser/pdfhost.rs, browser_a11y.rs and 38 more | 17235 | 211 | 2 | 0 | 13 | 1 | 0 | 201 | 217 | 0 | 0 | yes | yes | `BrowserPort` | L | to do |
| 22 | Settings (`SettingsApp`) | settings.rs, settings_access.rs, settings_agents.rs and 74 more | 20419 | 534 | 20 | 0 | 2 | 0 | 5 | 714 | 741 | 0 | 0 | yes | no | `SettingsPort` | L | to do |
| done | Auth (`AuthApp`) | auth/mod.rs, auth/page.rs, auth/port.rs | 1040 | 6 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | yes | no | `AuthPort` | - | migrated (port) |
| done | Calendar (`CalendarApp`) | calendar.rs | 151 | 5 | 0 | 0 | 0 | 0 | 0 | 3 | 3 | 0 | 0 | yes | no | `CalendarPort` | - | extracted crate (adapter in the shell) |
| done | Camera (`CameraApp`) | camera.rs | 1345 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `CameraPort` | - | migrated (port) |
| done | Cameras (`CamerasApp`) | cameras.rs | 228 | 9 | 0 | 0 | 0 | 0 | 1 | 5 | 6 | 0 | 0 | yes | no | `CamerasPort` | - | extracted crate (adapter in the shell) |
| done | Checkup (`CheckupApp`) | checkup.rs, checkup_ui.rs | 814 | 8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `CheckupPort` | - | migrated (port) |
| done | Data (`DataApp`) | data.rs | 1492 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `DataPort` | - | migrated (port) |
| done | Doctor (`DoctorApp`) | doctor.rs, doctor_facts.rs, doctor_ui.rs | 620 | 14 | 0 | 0 | 0 | 0 | 0 | 4 | 4 | 0 | 0 | yes | no | `DoctorPort` | - | migrated (port) |
| done | LinuxWin (`LinuxWinApp`) | linuxwin.rs | 1045 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `LinuxWinPort` | - | migrated (port) |
| done | Mcp (`McpApp`) | mcp.rs | 447 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `McpPort` | - | migrated (port) |
| done | Media (`MediaApp`) | media.rs | 2198 | 7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `MediaPort` | - | migrated (port) |
| done | Pdf (`PdfApp`) | pdfview.rs | 157 | 10 | 0 | 0 | 0 | 0 | 0 | 5 | 5 | 0 | 0 | yes | no | `PdfPort` | - | extracted crate (adapter in the shell) |
| done | Radio (`RadioApp`) | radio.rs | 70 | 5 | 0 | 0 | 0 | 0 | 0 | 2 | 2 | 0 | 0 | yes | yes | `RadioPort` | - | extracted crate (adapter in the shell) |
| done | Remote (`RemoteFilesApp`) | remotefiles.rs | 79 | 6 | 1 | 0 | 0 | 0 | 0 | 5 | 6 | 0 | 0 | yes | yes | `RemoteFilesPort` | - | extracted crate (adapter in the shell) |
| done | Runs (`RunsApp`) | runs.rs | 487 | 7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `RunsPort` | - | migrated (port) |
| done | Scan (`ScanApp`) | scan.rs | 675 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `ScanPort` | - | migrated (port) |
| done | Skills (`SkillsApp`) | skills.rs | 500 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | yes | `SkillsPort` | - | migrated (port) |
| done | Usage (`UsageApp`) | usage.rs | 71 | 6 | 0 | 0 | 0 | 0 | 0 | 2 | 2 | 0 | 0 | no | no | `UsagePort` | - | extracted crate (adapter in the shell) |
| done | Users (`UsersApp`) | users.rs | 433 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `UsersPort` | - | migrated (port) |

Totals: 40 apps, 18 migrated or extracted, 22 to do (S 6, M 10, L 6); 1933 uses of the services and the context in the apps still to do.

Notes: the count is by occurrence and the file list is by name prefix, so a helper module shared by two apps is counted for the app whose name is the longer prefix; read the numbers as a size class, not as an exact work estimate. An app whose crate was already extracted (`iyakki-tile-*`) shows `an extracted crate`: its source traits are in `iyakki-tile-api` already.

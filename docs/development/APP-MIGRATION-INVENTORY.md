# Built-in apps: the inventory for the move onto the app standard

Written by `python3 dist/scripts/app-inventory.py --write`; do not edit by hand, run the script again (`--check` says when the file is out of date).
The standard and the way to move one app is [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) section 10 (worked example: section 12, RADIO).

How to read it: **uses** counts, in the non-test code of the app's files, the places that name the services or the shell context (`.services`/`Services`, `cx.host`, `cx.web`, `cx.chat`, `cx.store`, `cx.session`/`cx.audit`); `AppCx` counts the mentions of the context type itself (a migrated tile keeps only the entry points of the `App` trait). **agent paths** counts the `iyakki_agent::` paths (the data types and helpers the app takes from the agent, which an extracted crate cannot); **ratchet** is the sum of the lines the app's files hold in `shell_kernel_access.allow` (direct kernel and hardware access). **chat** says whether the assistant has tools whose names start with the app's name; **settings** whether a Settings page has the app's name. Effort: S up to 15, M up to 60, L above, counting uses plus half the agent paths, one class up for an app of more than 6000 lines. The order is the suggested order for alpha.4: small and quiet first, so that every change is one tile and the procedure is corrected on the easy ones before the hard ones.

| Order | App | Files | Lines | AppCx | services | host | web | chat | store | session/audit | uses | agent paths | ratchet | chat tools | settings page | port to write | effort | state |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | Runs (`RunsApp`) | runs.rs | 475 | 10 | 3 | 0 | 0 | 0 | 0 | 7 | 10 | 2 | 0 | yes | no | `RunsPort` | S | to do |
| 2 | LinuxWin (`LinuxWinApp`) | linuxwin.rs | 1051 | 11 | 4 | 0 | 0 | 0 | 0 | 8 | 12 | 5 | 0 | no | no | `LinuxWinPort` | S | to do |
| 3 | DevTools (`DevToolsApp`) | devtools/b64.rs, devtools/convert.rs, devtools/diff.rs and 13 more | 3637 | 10 | 0 | 0 | 0 | 0 | 0 | 9 | 9 | 10 | 0 | no | no | `DevToolsPort` | S | to do |
| 4 | Checkup (`CheckupApp`) | checkup.rs, checkup_ui.rs | 856 | 19 | 8 | 0 | 0 | 0 | 0 | 7 | 15 | 18 | 0 | yes | no | `CheckupPort` | M | to do |
| 5 | Rag (`RagApp`) | rag.rs | 621 | 13 | 7 | 0 | 0 | 2 | 0 | 14 | 23 | 5 | 0 | yes | yes | `RagPort` | M | to do |
| 6 | Dash (`DashApp`) | dash.rs, dash_charts.rs, dash_query.rs and 1 more | 5071 | 25 | 1 | 0 | 0 | 0 | 1 | 5 | 7 | 38 | 0 | yes | no | `DashPort` | M | to do |
| 7 | Disks (`DisksApp`) | disks_tile.rs, disks_tile_ui.rs | 1957 | 25 | 6 | 0 | 0 | 0 | 0 | 8 | 14 | 29 | 0 | yes | no | `DisksPort` | M | to do |
| 8 | K8s (`K8sApp`) | k8s_tile.rs, k8s_tile_ui.rs | 2911 | 20 | 3 | 0 | 0 | 0 | 0 | 9 | 12 | 45 | 0 | yes | no | `K8sPort` | M | to do |
| 9 | Git (`GitApp`) | git_tile.rs, git_tile_repo.rs, git_tile_repo_ui.rs and 1 more | 3957 | 36 | 5 | 0 | 0 | 0 | 0 | 16 | 21 | 29 | 0 | yes | no | `GitPort` | M | to do |
| 10 | Mesh (`MeshApp`) | mesh.rs | 615 | 16 | 25 | 0 | 0 | 0 | 0 | 12 | 37 | 1 | 0 | no | no | `MeshPort` | M | to do |
| 11 | System (`SystemApp`) | system.rs | 973 | 14 | 10 | 1 | 0 | 0 | 0 | 18 | 29 | 21 | 0 | yes | no | `SystemPort` | M | to do |
| 12 | Doctor (`DoctorApp`) | doctor.rs, doctor_facts.rs, doctor_ui.rs | 640 | 23 | 11 | 0 | 0 | 0 | 0 | 18 | 29 | 24 | 0 | yes | no | `DoctorPort` | M | to do |
| 13 | Learn (`LearnApp`) | learn.rs, learn_lesson.rs, learn_ui.rs | 1535 | 26 | 24 | 0 | 0 | 2 | 0 | 8 | 34 | 14 | 0 | no | no | `LearnPort` | M | to do |
| 14 | Lab (`LabApp`) | lab.rs, lab_pic.rs, lab_ui.rs | 2347 | 27 | 10 | 0 | 0 | 0 | 0 | 26 | 36 | 11 | 0 | no | no | `LabPort` | M | to do |
| 15 | Scan (`ScanApp`) | scan.rs | 731 | 11 | 24 | 0 | 0 | 0 | 0 | 25 | 49 | 8 | 2 | no | no | `ScanPort` | M | to do |
| 16 | Flows (`FlowsApp`) | flows.rs, flows_assist.rs, flows_board.rs and 11 more | 7324 | 21 | 2 | 0 | 0 | 0 | 0 | 8 | 10 | 21 | 0 | yes | no | `FlowsPort` | L | to do |
| 17 | Edit (`EditApp`) | edit.rs, edit_preview.rs | 2286 | 26 | 5 | 1 | 0 | 1 | 0 | 43 | 50 | 24 | 0 | yes | no | `EditPort` | L | to do |
| 18 | Inbox (`InboxApp`) | inbox.rs | 1033 | 16 | 36 | 0 | 0 | 0 | 0 | 47 | 83 | 11 | 0 | no | no | `InboxPort` | L | to do |
| 19 | Cli (`CliApp`) | cli.rs, cli_conn.rs, cli_term.rs | 1698 | 25 | 43 | 3 | 0 | 0 | 0 | 53 | 99 | 24 | 0 | yes | no | `CliPort` | L | to do |
| 20 | Tales (`StoryApp`) | story.rs | 5724 | 98 | 54 | 0 | 1 | 1 | 0 | 46 | 102 | 38 | 0 | no | no | `StoryPort` | L | to do |
| 21 | Voice (`VoiceApp`) | voice.rs, voice_assist.rs, voice_fix.rs and 4 more | 3153 | 65 | 52 | 1 | 0 | 1 | 0 | 55 | 109 | 45 | 0 | yes | yes | `VoicePort` | L | to do |
| 22 | Store (`StoreApp`) | store.rs, store_drivers.rs, store_manage.rs and 4 more | 8170 | 100 | 74 | 1 | 0 | 0 | 0 | 129 | 204 | 167 | 3 | yes | no | `StorePort` | L | to do |
| 23 | Browser (`BrowserApp`) | browser.rs, browser/pdfhost.rs, browser_a11y.rs and 32 more | 16092 | 195 | 80 | 0 | 13 | 1 | 0 | 170 | 264 | 123 | 0 | yes | yes | `BrowserPort` | L | to do |
| 24 | Ide (`IdeApp`) | ide.rs, ide_bottom.rs, ide_browser.rs and 14 more | 13796 | 142 | 94 | 1 | 0 | 1 | 0 | 164 | 260 | 192 | 0 | yes | no | `IdePort` | L | to do |
| 25 | Chat (`ChatApp`) | chat.rs, chat_browser_data.rs, chat_camvision.rs and 9 more | 7661 | 102 | 180 | 1 | 1 | 4 | 2 | 311 | 499 | 462 | 0 | yes | no | `ChatPort` | L | to do |
| 26 | Settings (`SettingsApp`) | settings.rs, settings_access.rs, settings_agents.rs and 63 more | 17423 | 452 | 149 | 0 | 2 | 0 | 5 | 551 | 707 | 305 | 2 | yes | no | `SettingsPort` | L | to do |
| done | Auth (`AuthApp`) | auth/mod.rs, auth/page.rs, auth/port.rs | 1039 | 6 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 6 | 0 | yes | no | `AuthPort` | - | migrated (port) |
| done | Calendar (`CalendarApp`) | calendar.rs | 150 | 5 | 2 | 0 | 0 | 0 | 0 | 3 | 5 | 2 | 0 | yes | no | `CalendarPort` | - | extracted crate (adapter in the shell) |
| done | Camera (`CameraApp`) | camera.rs | 1337 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `CameraPort` | - | migrated (port) |
| done | Cameras (`CamerasApp`) | cameras.rs | 450 | 11 | 10 | 0 | 0 | 0 | 1 | 5 | 16 | 54 | 0 | yes | no | `CamerasPort` | - | extracted crate (adapter in the shell) |
| done | Data (`DataApp`) | data.rs | 1492 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 0 | yes | no | `DataPort` | - | migrated (port) |
| done | Mcp (`McpApp`) | mcp.rs | 447 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | yes | no | `McpPort` | - | migrated (port) |
| done | Media (`MediaApp`) | media.rs | 2175 | 7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 14 | 0 | yes | no | `MediaPort` | - | migrated (port) |
| done | Pdf (`PdfApp`) | pdfview.rs | 157 | 10 | 1 | 0 | 0 | 0 | 0 | 5 | 6 | 2 | 0 | yes | no | `PdfPort` | - | extracted crate (adapter in the shell) |
| done | Radio (`RadioApp`) | radio.rs | 70 | 5 | 0 | 0 | 0 | 0 | 0 | 2 | 2 | 0 | 0 | yes | no | `RadioPort` | - | extracted crate (adapter in the shell) |
| done | Remote (`RemoteFilesApp`) | remotefiles.rs | 193 | 6 | 3 | 0 | 0 | 0 | 0 | 5 | 8 | 3 | 0 | yes | yes | `RemoteFilesPort` | - | extracted crate (adapter in the shell) |
| done | Skills (`SkillsApp`) | skills.rs | 499 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | yes | yes | `SkillsPort` | - | migrated (port) |
| done | Usage (`UsageApp`) | usage.rs | 88 | 5 | 2 | 0 | 0 | 0 | 0 | 2 | 4 | 4 | 0 | no | no | `UsagePort` | - | extracted crate (adapter in the shell) |
| done | Users (`UsersApp`) | users.rs | 387 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `UsersPort` | - | migrated (port) |

Totals: 39 apps, 13 migrated or extracted, 26 to do (S 3, M 12, L 11); 2724 uses of the services and the context in the apps still to do.

Notes: the count is by occurrence and the file list is by name prefix, so a helper module shared by two apps is counted for the app whose name is the longer prefix; read the numbers as a size class, not as an exact work estimate. An app whose crate was already extracted (`iyakki-tile-*`) shows `an extracted crate`: its source traits are in `iyakki-tile-api` already.

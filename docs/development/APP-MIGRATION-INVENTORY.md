# Built-in apps: the inventory for the move onto the app standard

Written by `python3 dist/scripts/app-inventory.py --write`; do not edit by hand, run the script again (`--check` says when the file is out of date).
The standard and the way to move one app is [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) section 10 (worked example: section 12, RADIO).

How to read it: **uses** counts, in the non-test code of the app's files, the places that name the services or the shell context (`.services`/`Services`, `cx.host`, `cx.web`, `cx.chat`, `cx.store`, `cx.session`/`cx.audit`); `AppCx` counts the mentions of the context type itself (a migrated tile keeps only the entry points of the `App` trait). **agent paths** counts the `iyakki_agent::` paths (the data types and helpers the app takes from the agent, which an extracted crate cannot); **ratchet** is the sum of the lines the app's files hold in `shell_kernel_access.allow` (direct kernel and hardware access). **chat** says whether the assistant has tools whose names start with the app's name; **settings** whether a Settings page has the app's name. Effort: S up to 15, M up to 60, L above, counting uses plus half the agent paths, one class up for an app of more than 6000 lines. The order is the suggested order for alpha.4: small and quiet first, so that every change is one tile and the procedure is corrected on the easy ones before the hard ones.

| Order | App | Files | Lines | AppCx | services | host | web | chat | store | session/audit | uses | agent paths | ratchet | chat tools | settings page | port to write | effort | state |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | Chat (`ChatApp`) | chat.rs, chat_appevents.rs, chat_browser_data.rs and 15 more | 6497 | 95 | 8 | 1 | 0 | 2 | 0 | 0 | 11 | 0 | 0 | yes | no | `ChatPort` | M | to do |
| 2 | Edit (`EditApp`) | edit.rs, edit_preview.rs | 2326 | 27 | 1 | 1 | 0 | 1 | 0 | 47 | 50 | 0 | 0 | yes | no | `EditPort` | M | to do |
| 3 | Ide (`IdeApp`) | ide.rs, ide_bottom.rs, ide_browser.rs and 13 more | 13669 | 144 | 29 | 1 | 0 | 1 | 0 | 0 | 31 | 0 | 0 | yes | no | `IdePort` | L | to do |
| done | Auth (`AuthApp`) | auth/mod.rs, auth/page.rs, auth/port.rs | 1040 | 6 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | yes | no | `AuthPort` | - | migrated (port) |
| done | Browser (`BrowserApp`) | browser.rs, browser/pdfhost.rs, browser_a11y.rs and 38 more | 17191 | 161 | 2 | 0 | 9 | 1 | 0 | 185 | 197 | 0 | 0 | yes | yes | `BrowserPort` | - | migrated (port) |
| done | Calendar (`CalendarApp`) | calendar.rs | 198 | 5 | 0 | 0 | 0 | 0 | 0 | 3 | 3 | 0 | 0 | yes | no | `CalendarPort` | - | extracted crate (adapter in the shell) |
| done | Camera (`CameraApp`) | camera.rs | 1345 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `CameraPort` | - | migrated (port) |
| done | Cameras (`CamerasApp`) | cameras.rs | 228 | 9 | 0 | 0 | 0 | 0 | 1 | 5 | 6 | 0 | 0 | yes | no | `CamerasPort` | - | extracted crate (adapter in the shell) |
| done | Checkup (`CheckupApp`) | checkup.rs | 152 | 6 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `CheckupPort` | - | migrated (port) |
| done | Cli (`CliApp`) | cli.rs, cli_conn.rs, cli_term.rs | 1717 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `CliPort` | - | migrated (port) |
| done | Dash (`DashApp`) | dash.rs | 186 | 9 | 1 | 0 | 0 | 0 | 1 | 6 | 8 | 0 | 0 | no | no | `DashPort` | - | extracted crate (adapter in the shell) |
| done | Data (`DataApp`) | data.rs | 1492 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `DataPort` | - | migrated (port) |
| done | DevTools (`DevToolsApp`) | devtools/mod.rs, devtools/net.rs, devtools/qrcode.rs and 1 more | 636 | 6 | 0 | 0 | 0 | 0 | 0 | 4 | 4 | 0 | 0 | no | no | `DevToolsPort` | - | extracted crate (adapter in the shell) |
| done | Disks (`DisksApp`) | disks_tile.rs | 129 | 8 | 0 | 0 | 0 | 0 | 0 | 6 | 6 | 0 | 0 | yes | no | `DisksPort` | - | extracted crate (adapter in the shell) |
| done | Doctor (`DoctorApp`) | doctor.rs, doctor_facts.rs, doctor_manual.rs and 1 more | 1056 | 13 | 0 | 0 | 0 | 0 | 0 | 6 | 6 | 0 | 0 | yes | no | `DoctorPort` | - | migrated (port) |
| done | Firewall (`FirewallApp`) | firewall_tile.rs | 106 | 6 | 0 | 0 | 0 | 0 | 0 | 4 | 4 | 0 | 0 | yes | yes | `FirewallPort` | - | extracted crate (adapter in the shell) |
| done | Flows (`FlowsApp`) | flows.rs | 224 | 11 | 1 | 0 | 0 | 0 | 0 | 4 | 5 | 0 | 0 | yes | no | `FlowsPort` | - | extracted crate (adapter in the shell) |
| done | Git (`GitApp`) | git_tile.rs | 81 | 6 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `GitPort` | - | extracted crate (adapter in the shell) |
| done | Inbox (`InboxApp`) | inbox.rs | 71 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `InboxPort` | - | extracted crate (adapter in the shell) |
| done | K8s (`K8sApp`) | k8s_tile.rs | 85 | 6 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `K8sPort` | - | migrated (port) |
| done | Lab (`LabApp`) | lab.rs | 68 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `LabPort` | - | extracted crate (adapter in the shell) |
| done | Learn (`LearnApp`) | learn.rs | 89 | 7 | 0 | 0 | 0 | 0 | 0 | 1 | 1 | 0 | 0 | no | no | `LearnPort` | - | extracted crate (adapter in the shell) |
| done | LinuxWin (`LinuxWinApp`) | linuxwin.rs | 105 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `LinuxWinPort` | - | migrated (port) |
| done | Mcp (`McpApp`) | mcp.rs | 447 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `McpPort` | - | migrated (port) |
| done | Media (`MediaApp`) | media.rs | 2198 | 7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `MediaPort` | - | migrated (port) |
| done | Mesh (`MeshApp`) | mesh.rs | 75 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `MeshPort` | - | migrated (port) |
| done | Pdf (`PdfApp`) | pdfview.rs | 157 | 10 | 0 | 0 | 0 | 0 | 0 | 5 | 5 | 0 | 0 | yes | no | `PdfPort` | - | extracted crate (adapter in the shell) |
| done | Radio (`RadioApp`) | radio.rs | 70 | 5 | 0 | 0 | 0 | 0 | 0 | 2 | 2 | 0 | 0 | yes | yes | `RadioPort` | - | extracted crate (adapter in the shell) |
| done | Rag (`RagApp`) | rag.rs | 64 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | yes | `RagPort` | - | migrated (port) |
| done | Remote (`RemoteFilesApp`) | remotefiles.rs | 79 | 6 | 1 | 0 | 0 | 0 | 0 | 5 | 6 | 0 | 0 | yes | yes | `RemoteFilesPort` | - | extracted crate (adapter in the shell) |
| done | Runs (`RunsApp`) | runs.rs | 82 | 6 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `RunsPort` | - | migrated (port) |
| done | Scan (`ScanApp`) | scan.rs, scan_models.rs | 204 | 6 | 1 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | no | no | `ScanPort` | - | migrated (port) |
| done | Settings (`SettingsApp`) | settings.rs, settings_access.rs, settings_account.rs and 93 more | 23914 | 18 | 2 | 1 | 0 | 0 | 1 | 37 | 41 | 0 | 3 | yes | no | `SettingsPort` | - | migrated (port) |
| done | Skills (`SkillsApp`) | skills.rs | 500 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | yes | `SkillsPort` | - | migrated (port) |
| done | Store (`StoreApp`) | store.rs, store_drivers.rs, store_manage.rs and 4 more | 8374 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `StorePort` | - | migrated (port) |
| done | System (`SystemApp`) | system.rs | 101 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | no | `SystemPort` | - | extracted crate (adapter in the shell) |
| done | Tales (`StoryApp`) | story.rs | 126 | 4 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `StoryPort` | - | extracted crate (adapter in the shell) |
| done | Usage (`UsageApp`) | usage.rs | 71 | 6 | 0 | 0 | 0 | 0 | 0 | 2 | 2 | 0 | 0 | no | no | `UsagePort` | - | extracted crate (adapter in the shell) |
| done | Users (`UsersApp`) | users.rs | 433 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | no | no | `UsersPort` | - | migrated (port) |
| done | Voice (`VoiceApp`) | voice.rs | 112 | 5 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | yes | yes | `VoicePort` | - | extracted crate (adapter in the shell) |

Totals: 40 apps, 37 migrated or extracted, 3 to do (S 0, M 2, L 1); 92 uses of the services and the context in the apps still to do.

Notes: the count is by occurrence and the file list is by name prefix, so a helper module shared by two apps is counted for the app whose name is the longer prefix; read the numbers as a size class, not as an exact work estimate. An app whose crate was already extracted (`iyakki-tile-*`) shows `an extracted crate`: its source traits are in `iyakki-tile-api` already.

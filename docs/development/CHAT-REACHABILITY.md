# Chat reachability: every Settings page has an assistant tool

Owner rule (2026-11-06): every feature is reachable by chat. A change is an assistant tool that asks the person every time; a read is free. A tool calls the same service code the page calls (one shared path), never exposes a secret, and writes an audit line.

This file is read by a test (`chat_reachability_gate` in `crates/iyakki-shell/src/apps/chat_reachability_tests.rs`). Rules the test enforces:

* every row names a Settings `Tab` in the first column; every `Tab` has a row;
* a row with tools names tools that exist in the source; a row without tools (`MISSING`) must be listed in `chat-reachability-exempt.txt` with a reason;
* an exempt page that gets a tool must leave the exemption list (the list only shrinks).

Risk classes: `free` = read only, no card; `ask` = changes something, a card every time, no session grant (`grant_blocker`), never "always allow"; `hard` = destructive or secret-adjacent, the card is the full plan.

How a tool is built: `crates/iyakki-agent/src/settingstools.rs` is the frame (`Entry`, `SettingsTools`); a page contributes `Entry` values (`settingstools_<page>.rs`, or `apps/chat_settings.rs` when the code lives in the shell). Each has unit tests in the same module and is classified in the system prompt rule (`PROMPT_RULE`) that the chat adds for the person's role.

## The table

| Tab | Page | Tools | Risk and approval | Status |
|---|---|---|---|---|
| `Overview` | Overview | none: the start page only links to other pages | n/a | exempt (permanent) |
| `Account` | Account | MISSING | n/a | todo |
| `Notifications` | Notifications | `notification_filters`, `notification_filter_set`, `message_mirror_set`, `message_room_mirror_set`, `message_unread`, `message_inbox_sync`, `message_read` | free; ask (the copy of messages into the INBOX asks every time; what is read is labelled as coming from outside) | added |
| `Vault` | Vault | `vault_request` | ask (secrets are typed by the person into a masked form, never by the model) | covered |
| `Backup` | Backup and restore | `backup_status`, `backup_inspect`, `backup_create`, `backup_restore`, `backup_schedule`, `checkup_result_verify`, `edit_run_stop`, `edit_run_input` | free; ask (create and restore name the folder or file and the areas; a passphrase never goes through the chat, so a chat backup is unencrypted by the person's explicit word; paths stay inside the person's home) | added |
| `EncHome` | Encrypted home | `encrypted_home_status`, `encrypted_home_plan_move`, `encrypted_home_create`, `encrypted_home_unlock`, `encrypted_home_lock`, `encrypted_home_move_files`, `encrypted_home_new_recovery_key`, `encrypted_home_set_admin_slot` | free; ask every time | covered |
| `SecretProviders` | Secret providers | `shared_vault_provider`, `shared_vault_provider_set` | free; ask | covered |
| `Models` | Models | `pdf_output_status`, `pdf_output_set`, `models_list`, `model_options_get`, `model_options_set`, `provider_models_fetch`, `lemonade_compute_status`, `lemonade_compute_set`, `ollama_status`, `ollama_install`, `gemini_status`, `gemini_set`, `gemini_consent_revoke` | free; ask (Google Gemini, experimental: every change and taking the consent back ask every time; the key is typed by the person, the consent card is answered on the page, the assistant can do neither) | covered |
| `Agents` | Agents | `list_clis`, `install_cli`, `remove_cli`, `pdf_output_status`, `pdf_output_set` | free; ask | covered |
| `Skills` | Skills | `skills_list`, `skill_add`, `skill_remove` | free; ask (adding and removing show the text; never covered by a standing grant) | added |
| `Rag` | Document collections | `rag_sources_add_folder`, `search_documents`, `rag_rerank_status`, `rag_rerank_set` | ask; free | covered |
| `Voice` | Voice and speech | `voice_assistant_status`, `voice_assistant_set`, `voice_wake_status`, `dictation_wording_status`, `dictation_wording_set` | free; ask (the wake phrase is experimental) | covered |
| `Browser` | Browser | `browser_graphics_get`, `browser_graphics_set`, `browser_accessibility_status`, `browser_a11y_set`, `browser_headers_list`, `browser_headers_enable`, `browser_headers_delete`, `browser_block_status`, `browser_block_set`, `browser_block_download`, `browser_block_update`, `browser_block_remove`, `browser_block_allow`, `browser_badsites_status`, `browser_badsites_set`, `browser_badsites_add`, `browser_badsites_remove`, `browser_badsites_download`, `browser_certificate_view`, `browser_data_export`, `browser_data_import`, `browser_protection_choices`, `browser_protection_choices_set`, `browser_bookmarks`, `browser_bookmark_folder`, `browser_history_search`, `browser_zoom_status`, `browser_zoom_set`, `browser_profile_status`, `browser_profile_clear`, `browser_thumbnails_status`, `browser_thumbnails_set`, `browser_downloads_list`, `browser_download_open`, `browser_reports_list`, `browser_reports_clear`, `browser_media_sites`, `browser_media_site_forget`, `browser_av_addon_status` | free; ask. A header value is never shown or accepted by chat: the person types it on the page into the vault. The saved name and address are never shown or set by chat (status lists field names only); filling a form from them is Ctrl+Y on the page, not a chat tool; uploads of the browser have no chat tool; saved downloads are listed and opened in EDIT, DATA or MEDIA by chat; saving a site report is the Report site button on the page (reports are listed and deleted by chat) | covered + added |
| `HostAccess` | Host access | MISSING | n/a | todo |
| `AppAccess` | Apps access | `apps_access_list`, `apps_access_revoke`, `app_sdk_status`, `app_money_limit_status`, `app_money_limit_set` | free (administrators); ask; free; free; ask every time and hard (the money limit of an app: it starts at nothing, only the person raises it, a spoken yes never approves it) | added |
| `Network` | Network | `wifi_status`, `wifi_scan`, `wifi_join`, `wifi_forget`, `hotspot_status`, `hotspot_set` | free; ask. A change asks every time and never rides on a session grant. A passphrase, enterprise password or WEP key is read from a vault entry (`secret_entry`, made with `vault_request`), never typed in the chat; WEP needs `accept_wep_risk` after the warning; the hotspot password is never shown. Ethernet status is not covered | covered |
| `SshTunnels` | SSH tunnels | `ssh_tunnel_list` | free | covered (read only) |
| `Vpn` | VPN | `vpn_list` | free | covered (read only) |
| `Remote` | Remote access and Connect a client (experimental) | `connect_client_status`, `connect_client_enable`, `client_devices`, `client_device_permission`, `client_device_revoke`, `sftp_status`, `sftp_set` | free; ask (administrators). A one-time link is shown only on the page (bearer token) | added + covered |
| `Domains` | Domains | `domains_list` | free | covered (read only) |
| `Firewall` | Firewall | `firewall_vpn_local_status`, `firewall_vpn_local_set`, `firewall_counters` | free; ask (administrators). The VPN switch and the read-only counters (blocked total, what each rule let in) so far: rules, presets and ping are still not reachable by chat | partial |
| `Shares` | Shared folders | `share_list`, `sftp_status`, `sftp_set`, `media_server_status`, `media_server_set` | free; ask (the media server switch lists nothing new: it offers audio and video of the shared folders to the local network). `share_list` says how to get a folder as one zip (`?zip`); the zip needs no switch of its own | covered |
| `Display` | Display | `displays_status`, `displays_look_again` | free; free (Look again only asks the kernel to read the displays again; brightness, text size and scale are still page-only) | added (partly) |
| `Displays` | Displays | MISSING | n/a | todo |
| `ScreenSaver` | Screen saver | `screensaver_status`, `screensaver_set`, `screensaver_preview`, `lock_screen_status`, `lock_screen_set_auto`, `lock_screen` | free; ask; ask (lock now asks nothing: it only protects) | added + covered |
| `HomeLayout` | Home layout | `home_layout_list`, `home_layout_group` | free; ask | added |
| `Views` | Desktop views | `views_list`, `views_templates_list`, `views_make`, `views_rename`, `views_delete`, `views_lock`, `views_keys`, `views_switch`, `views_template_save`, `views_template_delete` | free; ask (a locked view still asks the person for the password at the lock screen) | added |
| `Sound` | Sound | `sound_output_status`, `sound_output_set` (output only; volume, mute and the sound card owner still have no tool; `bluetooth_audio_status` reports the Bluetooth sound path) | free (everyone: the output in use and why, whether a hand-chosen output is set aside because its HDMI/DP pin has no monitor, every output with its key and monitor present); ask (administrators; same path as the page's output list, audited as `settings.sound_output`) | added (partly) |
| `SystemSounds` | System sounds | `system_sounds_status`, `system_sounds_set` | free; ask (administrators) | added |
| `Accessibility` | Accessibility | `accessibility_status`, `accessibility_set` | free; ask | covered |
| `Bluetooth` | Bluetooth | `bluetooth_list`, `bluetooth_tether_status`, `bluetooth_audio_status`, `bluetooth_pair_status`, `bluetooth_scan`, `bluetooth_pair`, `bluetooth_pair_answer` | free (list and status); ask (scan, pair and the yes or no of the number comparison; the digits a device shows for typing are typed in Settings, never through the chat) | covered |
| `Keyboard` | Keyboard | `keyboard_status`, `keyboard_set`, `pointer_speed_status`, `pointer_speed_set` | free; ask (administrators); applied at once through the page's own `apply_keyboard`; the pointer speed tools change only the mouse and touchpad speed (25 to 300 percent, audit action `settings.pointer_speed`) | added |
| `Time` | Time | `time_status`, `time_set`, `clock_face_set` | free; ask (zone and network time: administrators; the clock face is the person's own) | added |
| `Assistant` | Assistant memory and limits | `assistant_memory_status`, `assistant_memory_set`, `assistant_memory_forget`, `assistant_limits_status`, `assistant_limits_set`, `history_forget` | free; ask (forgetting always asks, never granted; limits administrators) | added |
| `Battery` | Battery | `battery_settings_status`, `battery_settings_set` | free; ask (administrators) | added |
| `Power` | Power | `power_settings_status`, `power_settings_set` | free; ask (administrators) | added |
| `Computer` | This computer | `this_computer_status`, `this_computer_set_sharing`, `this_computer_copy_from`, `this_computer_reset`, `startup_disk_status`, `startup_disk_register`, `startup_disk_set` | free; ask. The startup disk tools write firmware boot variables (IYAKKI's own entry, the boot order, the one-time next start): administrators only, `startup_disk_register` and `startup_disk_set` are hard asks | covered |
| `HostName` | Computer name | `host_name_status`, `host_name_set` | free; ask (administrators; a name fixed by the boot option iyakki.hostname is not changed) | added |
| `SessionLog` | Last session log | `last_session_status`, `last_session_log_copy` | free; ask (administrators; the copy goes into the person's own home, the log text is never given to the model) | added |
| `Startup` | Startup | `startup_status` | free (administrators); retry is MISSING | added (read only) |
| `Memory` | Memory | `compressed_memory_status`, `compressed_memory_set`, `memory_settings_status`, `memory_settings_set`, `boot_speed_status`, `boot_speed_set` | free; ask (administrators). The experimental compressed-memory switch, and zram size, compressor, swappiness, swap file and zswap (used from the next start; a choice a boot option fixes is refused; Apply now stays on the page) | covered |
| `Storage` | Storage | `disks_status`, `disks_unlock`, `disks_lock`, `disks_format_plan`, `disks_format`, `disks_resize` | free; ask; hard (format and resize: the typed confirmation is on the page only) | covered |
| `Install` | Install to disk | `quick_install_status`, `hardware_firmware_install` | free; ask | covered (partly) |
| `Updates` | Update centre | `updates_status`, `updates_check_now`, `updates_settings_status`, `updates_settings_set`, `components_status`, `components_apply`, `components_channel_set`, `updates_bundle_open` | free; ask; hard (apply: the card is the plan; opening a signed update file asks every time) | covered |
| `Cloud` | Cloud mode; TLS link to a pod | `cloud_mode_status`, `cloud_mode_set`, `pod_tls_status`, `pod_tls_create`, `pod_tls_rotate` | free; ask (administrators). `pod_tls_create`/`pod_tls_rotate` are hard asks (they decide who may talk to a pod) and never show a certificate or a key. The team local-model chat backend (`TeamLocalChat`) has no switch of its own: a team host is an entry of the tenant configuration (`models.local`, the tenant config API) and the backend is not yet wired to a run, so it has no chat tool | added |
| `CloudRoles` | Cloud access | MISSING | n/a | todo |
| `Developer` | Developer mode | `restart_grant_status`, `restart_grant`, `devmode_api_scopes`, `devmode_api_scope_set`, `core_contract_status`, `core_contract_switch`, `core_contract_scope_set`, `wasm_tiles_status`, `wasm_tiles_set` | free; ask (administrators; the WASM tiles switch is experimental and off by default). Pairing itself and the other scopes are not reachable by chat | partial |
| `DevCluster` | Developer cluster | `cluster_status`, `cluster_start`, `cluster_stop` | free; ask | covered |
| `GitKeys` | Git and SSH keys | `git_clone_repo`, `git_clones_list` | free; ask. Keys stay in the vault: private keys are never shown to the model | covered (partly) |

## Not on a page

Sign-out is a power-menu action (`shell_signout.rs`, not edited in this slice): a `sign_out` tool needs a request file that the shell takes, as `lock_screen` does. It is listed in `chat-reachability-exempt.txt` as a todo for the owner of that file.

## The PDF output switch

`native_pdf_output` (a provider or an external agent writes PDF files itself, so `pdf_create` is not offered to it) is a key on the Models page (`P`) and the Agents page (`p`), and has the assistant tools `pdf_output_status` (free) and `pdf_output_set` (ask every time, with a card that names the old and the new state; administrators only). Covered; they are listed with the `Models` and `Agents` rows above.

## Tiles that write files (not Settings pages)

* PDF (docs/PDF.md): `pdf_info`, `pdf_text` (free), `page_save_pdf`, `pdf_form_fill` (fills form fields into a new file) and `pdf_create` (ask; `pdf_create` is offered to every model that does not write PDFs itself; a provider or external agent with `native_pdf_output` set in its settings is not offered it).
* LEARN: `study_export_pdf` (ask) makes a PDF of a plan, a quiz (answer key page only with `answer_key`) or a day's lesson, through the same function as the tile's Export PDF buttons. Covered.
* TALES: the Save PDF button, the `d` key and a message like "make it a PDF" in its conversation write `Stories/<title>/story.pdf` with the same writer; the general `pdf_create` can also make a PDF of a story from its `story.md`. A web link to a picture (the writer's reply linked to one, or a message like "use https://... for picture 2") is asked about in the tile and fetched only after a yes (F-TALES-18). Covered by the conversation request; there is no separate assistant tool for the TALES tile.
## Linux apps (Store, Setup, Linux apps)

Not a Settings page, so it has no row in the table above; its tools are `settingstools_linux.rs`, wired next to the Settings tools in `apps/chat_settings.rs`,
explained in the system prompt, and checked by `the_linux_apps_tools_are_in_the_prompt_the_doc_and_the_chat_wiring`.

| Page | Tools | Risk and approval |
|---|---|---|
| Linux apps | `linux_sandboxes`, `linux_network_set`, `linux_sandbox_create`, `linux_sandbox_delete`, `linuxapp_image`, `linuxapp_pods`, `linuxapp_pod_start`, `linuxapp_pod_stop` | free; ask. A change asks every time and never rides on a session grant. The same host check as the page and `linux net`: shared with the computer or open outbound needs an account allowed every host, a list of hosts needs each one allowed; the role needs the Linux apps (full command-line) permission |

Creating and deleting sandboxes, installing packages, mounting folders and running programs in a sandbox have no chat tool yet (todo); they are on the page and in `linux ...` commands.

## Counting

The summary of the last audit (covered before this slice / added by this slice / still missing) is kept in the commit that last changed this file and in the report of the lane.
| Cluster explorer (K8S tile) | `k8s_custom_resources` | free (read-only; the output is marked untrusted). Lists custom resource definitions, or the instances of one, of a cluster the account sees in the tile. Screen: the Instances tab of a definition. |
